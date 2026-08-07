//! The render list: a compact copy of what this frame draws.
//!
//! ```text
//! RenderWorld  ->  Culler  ->  RenderList  ->  command recording  ->  D3D12
//!  (the world)   (visibility)  (this frame)
//! ```
//!
//! `RenderWorld` describes the world and outlives the frame. A `RenderList` describes one
//! frame's intent and is rebuilt from scratch every frame. Keeping them separate is the
//! point of Milestone 16: until now the recording loop walked a list of *world slot indices*
//! and reached back into `RenderWorld` for each one, which made the visible set an index
//! into someone else's memory rather than a thing in its own right.
//!
//! # Why a compact copy, and not indices
//!
//! Three shapes were possible. The list could hold handles back into `RenderWorld`, an
//! array of per-object structs, or parallel arrays of exactly the fields the frame needs.
//! The third was chosen:
//!
//! * **Handles into `RenderWorld`** keep the list tiny — four bytes an entry — but every
//!   consumer then dereferences into five separate arrays in whatever order the list
//!   happens to be in. That is tolerable while the order matches the world's, and it stops
//!   being tolerable the moment the list is sorted (Milestone 17), because a sorted index
//!   array turns every recorded draw into a random access across the whole world.
//! * **An array of structs** keeps one object's fields together, which sounds right until
//!   you notice that no consumer reads all of them at once. A future sort would move whole
//!   64-byte elements; a future instancing pass wants the transforms contiguous and would
//!   get them interleaved with handles it does not care about.
//! * **Parallel arrays** cost one sequential copy per visible object at build time and give
//!   every consumer afterwards a dense, linear read of exactly the field it wants.
//!
//! The copy is not free and is not pretended to be: at 17,000 visible objects it writes
//! about 1.1 MB per frame. It is measured against the frame it replaces — see below — and
//! it sits next to the 4.4 MB of constant-buffer uploads the same frame already performs.
//!
//! # Why these fields and no others
//!
//! Only what the frame consumes. `RenderWorld` also stores world-space bounds, a visibility
//! flag and a generation array; none of the three reaches recording:
//!
//! * **bounds** are read by the culler, which reads them straight out of the world. Copying
//!   them into the list would be 16 bytes per visible object written and never read.
//! * **the visibility flag** has already been answered — an entry existing in the list *is*
//!   the answer.
//! * **generations** exist to detect a stale handle, and a list built and consumed inside
//!   one frame cannot hold one.
//!
//! And nothing from gameplay: no health, no AI state, no ability state, no network
//! identity. The list is what the renderer needs and stops there.
//!
//! # The transform, not the matrix — and this was measured, not reasoned
//!
//! The obvious design stores the composed [`Mat4`](engine_core::math::Mat4): 64 bytes, the
//! exact thing a constant buffer wants, and a contiguous array a future instancing pass
//! could upload in one memcpy. It was built that way first, and it cost **14%** of the
//! frame at 10,000 objects — six runs each, medians 0.830 ms against a 0.727 ms baseline,
//! with no overlap between the two sets.
//!
//! The reason is where the work sits rather than how much of it there is. The matrix has to
//! be composed once per visible object per frame either way. In the recording loop it is
//! surrounded by five driver calls per object, which are latency-bound — the composition
//! disappears into the shadow of work the CPU is already waiting on. Hoisted into the
//! gather loop it has nothing to hide behind, and a tight loop of quaternion-to-matrix
//! conversions becomes visible serial time.
//!
//! So the list carries the 40-byte `Transform` and recording composes the matrix exactly
//! where it always did. Storing it is measured at parity with the pre-Milestone-16 frame.
//! The list is also smaller — 64 bytes an entry rather than 88 — which makes a future sort
//! cheaper, and `translation` is a better depth key than digging a matrix column out.
//!
//! This will want revisiting at instancing. Once a batched draw replaces the per-object
//! driver calls there is nothing left to hide the composition behind, and composing a run
//! of transforms straight into the upload ring becomes the right shape. That is a change to
//! one loop, not to this structure.
//!
//! # Capacity
//!
//! [`RenderList::clear`] keeps the allocation. After the first few frames every array has
//! reached the scene's high-water mark and the build path allocates nothing at all — which
//! is asserted by a test rather than assumed, and measured by `cull_bench`.

use engine_core::transform::Transform;
use engine_render_data::{MaterialHandle, MeshHandle, RenderEntity};

/// One entry, gathered from the parallel arrays.
///
/// A view for tests and diagnostics, not the storage layout: reading an entry is a rare
/// debugging operation, and the arrays exist because the *common* operations read one field
/// across many entries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderListEntry {
    /// Which `RenderWorld` entity this came from.
    pub entity: RenderEntity,
    /// Where it is, in world space. Composed into a matrix at recording time.
    pub transform: Transform,
    /// What to draw.
    pub mesh: MeshHandle,
    /// How it looks.
    pub material: MaterialHandle,
}

/// What one frame draws, as parallel arrays.
///
/// Rebuilt every frame by [`crate::culling::Culler`]. Owned by the renderer, because it is
/// a transient rendering artefact rather than something the game authors — which is also
/// why it lives here and not in `engine-render-data`.
#[derive(Debug, Default, Clone)]
pub struct RenderList {
    entities: Vec<RenderEntity>,
    transforms: Vec<Transform>,
    meshes: Vec<MeshHandle>,
    materials: Vec<MaterialHandle>,
}

impl RenderList {
    /// An empty list with no reserved capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty list sized for `capacity` visible objects.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entities: Vec::with_capacity(capacity),
            transforms: Vec::with_capacity(capacity),
            meshes: Vec::with_capacity(capacity),
            materials: Vec::with_capacity(capacity),
        }
    }

    /// Empties the list, keeping every allocation.
    ///
    /// The whole reason the renderer owns one list across frames rather than building one
    /// per frame: after the scene reaches its high-water mark this is four length stores.
    pub fn clear(&mut self) {
        self.entities.clear();
        self.transforms.clear();
        self.meshes.clear();
        self.materials.clear();
    }

    /// Appends one drawable.
    ///
    /// The arrays are only ever pushed to together, which is what keeps index `i` meaning
    /// the same object in all four.
    pub fn push(
        &mut self,
        entity: RenderEntity,
        transform: Transform,
        mesh: MeshHandle,
        material: MaterialHandle,
    ) {
        self.entities.push(entity);
        self.transforms.push(transform);
        self.meshes.push(mesh);
        self.materials.push(material);
    }

    /// Entries in the list.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether the list draws nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Entries the list can hold before it has to grow.
    ///
    /// Reported so a test can prove the build path stops allocating, and so a caller can
    /// size the list from a previous frame.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.entities.capacity()
    }

    /// The source entity of each entry.
    #[must_use]
    pub fn entities(&self) -> &[RenderEntity] {
        &self.entities
    }

    /// The world transform of each entry.
    #[must_use]
    pub fn transforms(&self) -> &[Transform] {
        &self.transforms
    }

    /// The mesh of each entry.
    #[must_use]
    pub fn meshes(&self) -> &[MeshHandle] {
        &self.meshes
    }

    /// The material of each entry.
    #[must_use]
    pub fn materials(&self) -> &[MaterialHandle] {
        &self.materials
    }

    /// One entry, gathered. `None` past the end.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<RenderListEntry> {
        Some(RenderListEntry {
            entity: *self.entities.get(index)?,
            transform: *self.transforms.get(index)?,
            mesh: *self.meshes.get(index)?,
            material: *self.materials.get(index)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;
    use engine_core::transform::Transform;

    fn entry(index: u32) -> (RenderEntity, Transform, MeshHandle, MaterialHandle) {
        let transform = Transform::from_translation(Vec3::new(index as f32, 0.0, 0.0));
        (
            RenderEntity::new(index, 1),
            transform,
            MeshHandle::new(index % 3, 1),
            MaterialHandle::new(index % 5, 1),
        )
    }

    fn filled(count: u32) -> RenderList {
        let mut list = RenderList::new();
        for index in 0..count {
            let (entity, transform, mesh, material) = entry(index);
            list.push(entity, transform, mesh, material);
        }
        list
    }

    #[test]
    fn a_new_list_draws_nothing() {
        let list = RenderList::new();
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
        assert!(list.entities().is_empty());
        assert!(list.transforms().is_empty());
        assert!(list.entry(0).is_none());
    }

    #[test]
    fn every_array_stays_the_same_length() {
        // Index `i` has to mean the same object in all four, which is only true because
        // they are pushed together. A test rather than a comment because nothing in the
        // type system enforces it.
        let list = filled(37);
        assert_eq!(list.len(), 37);
        assert_eq!(list.entities().len(), 37);
        assert_eq!(list.transforms().len(), 37);
        assert_eq!(list.meshes().len(), 37);
        assert_eq!(list.materials().len(), 37);
    }

    #[test]
    fn an_entry_reads_back_exactly_what_was_pushed() {
        let list = filled(8);
        for index in 0..8_u32 {
            let (entity, transform, mesh, material) = entry(index);
            let Some(read) = list.entry(index as usize) else {
                panic!("entry {index} must exist in a list of eight");
            };
            assert_eq!(read.entity, entity);
            assert_eq!(read.transform, transform);
            assert_eq!(read.mesh, mesh);
            assert_eq!(read.material, material);
        }
    }

    #[test]
    fn a_stored_transform_still_composes_the_matrix_recording_needs() {
        // The list carries the transform and recording composes the matrix from it. If the
        // stored value ever stopped round-tripping, objects would render somewhere other
        // than where the world says they are.
        let transform = Transform::from_translation(Vec3::new(3.0, 4.0, 5.0))
            .with_scale(Vec3::splat(2.0))
            .with_rotation(engine_core::math::Quat::from_axis_angle(Vec3::Y, 0.7));
        let mut list = RenderList::new();
        list.push(
            RenderEntity::new(0, 1),
            transform,
            MeshHandle::new(0, 1),
            MaterialHandle::new(0, 1),
        );

        let Some(read) = list.entry(0) else {
            panic!("the list holds one entry");
        };
        assert_eq!(read.transform, transform);
        assert_eq!(read.transform.matrix(), transform.matrix());
        // `translation` is what a future depth sort reads, so it has to survive the copy
        // exactly rather than approximately.
        assert_eq!(read.transform.translation, Vec3::new(3.0, 4.0, 5.0));
    }

    #[test]
    fn an_entry_is_the_size_the_layout_reasoning_assumes() {
        // 8 + 40 + 8 + 8. The module prices a frame's copy from this number; if the layout
        // grows, the reasoning needs revisiting rather than silently becoming wrong.
        let bytes = size_of::<RenderEntity>()
            + size_of::<Transform>()
            + size_of::<MeshHandle>()
            + size_of::<MaterialHandle>();
        assert_eq!(bytes, 64, "one render list entry across the four arrays");
    }

    #[test]
    fn clearing_leaves_nothing_readable() {
        // Not just `len() == 0`: a stale entry still reachable through an accessor would be
        // last frame's object drawn this frame.
        let mut list = filled(16);
        list.clear();

        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
        assert!(list.entry(0).is_none());
        assert!(list.entities().is_empty());
        assert!(list.transforms().is_empty());
        assert!(list.meshes().is_empty());
        assert!(list.materials().is_empty());
    }

    #[test]
    fn a_cleared_list_refills_with_only_the_new_entries() {
        let mut list = filled(20);
        list.clear();
        for index in 100..105_u32 {
            let (entity, transform, mesh, material) = entry(index);
            list.push(entity, transform, mesh, material);
        }

        assert_eq!(list.len(), 5, "the first twenty must not survive");
        let Some(first) = list.entry(0) else {
            panic!("the refilled list holds five entries");
        };
        assert_eq!(first.entity, RenderEntity::new(100, 1));
    }

    #[test]
    fn reuse_stops_allocating_once_the_high_water_mark_is_reached() {
        // The reason the renderer owns one list across frames instead of building one per
        // frame. If this regresses, every frame allocates four vectors.
        let mut list = RenderList::new();
        for _ in 0..64 {
            let (entity, transform, mesh, material) = entry(0);
            list.push(entity, transform, mesh, material);
        }
        let capacity = list.capacity();
        assert!(capacity >= 64);

        for _ in 0..10 {
            list.clear();
            for index in 0..64 {
                let (entity, transform, mesh, material) = entry(index);
                list.push(entity, transform, mesh, material);
            }
        }
        assert_eq!(
            list.capacity(),
            capacity,
            "refilling to the same size must reuse the allocation"
        );
    }

    #[test]
    fn a_sized_list_starts_with_room() {
        let list = RenderList::with_capacity(1024);
        assert!(list.capacity() >= 1024);
        assert!(list.is_empty(), "capacity is not content");
    }
}
