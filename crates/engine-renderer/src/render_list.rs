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
use engine_render_data::{InstanceStyle, MaterialHandle, MeshHandle, RenderEntity};

/// A run of entries sharing one mesh and one material.
///
/// What grouping produces and what recording consumes: bind the mesh and material once,
/// then draw `count` entries starting at `first` in [`RenderList::order`]. Instancing
/// replaces the inner loop with a single draw; the batch is the same either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderBatch {
    /// The mesh every entry in the run uses.
    pub mesh: MeshHandle,
    /// The material every entry in the run uses.
    pub material: MaterialHandle,
    /// Index into [`RenderList::order`] where the run starts.
    pub first: u32,
    /// Entries in the run. Never zero.
    pub count: u32,
}

/// Largest histogram the counting sort will build.
///
/// `material_count * mesh_count` above this would allocate a bucket array larger than the
/// list it is sorting, so the grouping is skipped instead. 64Ki buckets is 256 KiB of
/// counters, which is already generous for a scene with one pipeline.
const MAX_GROUPING_BUCKETS: usize = 64 * 1024;

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
    /// Per-entity visual state, carried alongside the material rather than baked into it.
    pub style: InstanceStyle,
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
    styles: Vec<InstanceStyle>,
    /// Whether each entry is darkened where a shadow falls on it.
    ///
    /// A `bool` per entry rather than a field on [`InstanceStyle`]: the style is written
    /// every frame by extraction and this is a property of the *object*, set when it is
    /// spawned and almost never changed. Recording folds it into the padding word the
    /// per-object constants and the per-instance stream both already carry, so it costs no
    /// bytes on the GPU at all.
    receives_shadow: Vec<bool>,
    /// The order recording walks, filled by `group_by_material_and_mesh`.
    order: Vec<u32>,
    /// Runs of entries sharing a mesh and a material.
    batches: Vec<RenderBatch>,
    /// Histogram scratch for the counting sort, reused across frames.
    counts: Vec<u32>,
    /// Where each bucket started before the scatter, reused across frames.
    ///
    /// The scatter consumes `counts` by advancing each bucket's cursor, so the batch pass
    /// needs the values it had beforehand. This used to be a `to_vec()` inside the pass —
    /// one heap allocation of `material_count * mesh_count` words **every frame**, which
    /// went unnoticed while the scene had nine materials and four meshes and showed up the
    /// moment the environment brought forty of each. `env_bench` counts it.
    starts: Vec<u32>,
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
            styles: Vec::with_capacity(capacity),
            receives_shadow: Vec::with_capacity(capacity),
            order: Vec::with_capacity(capacity),
            batches: Vec::new(),
            counts: Vec::new(),
            starts: Vec::new(),
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
        self.styles.clear();
        self.receives_shadow.clear();
        self.order.clear();
        self.batches.clear();
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
        style: InstanceStyle,
        receives_shadow: bool,
    ) {
        self.entities.push(entity);
        self.transforms.push(transform);
        self.meshes.push(mesh);
        self.materials.push(material);
        self.styles.push(style);
        self.receives_shadow.push(receives_shadow);
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

    /// Whether each entry receives shadows.
    #[must_use]
    pub fn receives_shadow(&self) -> &[bool] {
        &self.receives_shadow
    }

    /// The per-entity visual state of each entry.
    ///
    /// A parallel array rather than part of the material, because it is the one field that
    /// changes every frame while the other three do not — and because it must **not**
    /// participate in grouping: two entities with different tints still share a batch.
    #[must_use]
    pub fn styles(&self) -> &[InstanceStyle] {
        &self.styles
    }

    /// Appends every entry of `other`, leaving it untouched.
    ///
    /// The merge step of a chunked cull: each worker fills its own list, and the chunks are
    /// concatenated in chunk order afterwards. One `extend_from_slice` per array, which is
    /// one memcpy each — the arrays being separate is what makes that true, and is one more
    /// reason the layout is parallel arrays rather than an array of structs.
    ///
    /// Chunk order is world order, so the concatenated list is identical to what a serial
    /// cull would have produced. That is asserted by a test rather than assumed, because a
    /// parallel cull that reordered the scene would be a rendering difference disguised as
    /// an optimization.
    pub fn append(&mut self, other: &Self) {
        self.entities.extend_from_slice(&other.entities);
        self.transforms.extend_from_slice(&other.transforms);
        self.meshes.extend_from_slice(&other.meshes);
        self.materials.extend_from_slice(&other.materials);
        self.styles.extend_from_slice(&other.styles);
        self.receives_shadow
            .extend_from_slice(&other.receives_shadow);
    }

    /// Reserves room for `additional` more entries in every array.
    ///
    /// Used before a merge, so the concatenation grows each array once instead of on the
    /// way through every chunk.
    pub fn reserve(&mut self, additional: usize) {
        self.entities.reserve(additional);
        self.transforms.reserve(additional);
        self.meshes.reserve(additional);
        self.materials.reserve(additional);
        self.styles.reserve(additional);
        self.receives_shadow.reserve(additional);
    }

    /// Groups the list so entries sharing a material and a mesh are adjacent.
    ///
    /// Does not move the entries. It fills [`RenderList::order`] with a permutation and
    /// [`RenderList::batches`] with the runs that permutation creates — the payload arrays
    /// stay in gather order. Permuting 64-byte entries would cost a second full copy of the
    /// list for a recording loop that reads two 8-byte handles per *batch* and the
    /// transforms in bulk; an index array costs four bytes an entry.
    ///
    /// # A counting sort, not a comparison sort
    ///
    /// The key is `(material, mesh)`, and both are indices into arrays the renderer owns,
    /// so the number of distinct keys is bounded by `material_count * mesh_count` and known
    /// before the pass starts. That makes this a histogram, a prefix sum and a scatter —
    /// three linear passes — rather than `n log n` comparisons.
    ///
    /// It matters at this size. A comparison sort of 17,000 entries is a few hundred
    /// thousand comparisons, which is the same order as the ~0.3 ms of driver calls the
    /// grouping is trying to save; it would have spent the win on the sort. Nine materials
    /// and four meshes is 36 buckets.
    ///
    /// Above [`MAX_GROUPING_BUCKETS`] the histogram would be larger than the list, so the
    /// list is left in gather order and the batches become whatever runs it already had.
    /// That is a correct answer, just a worse one, and it keeps a scene with thousands of
    /// materials from allocating a bucket array it cannot use.
    ///
    /// # Depth and pipeline are deliberately absent from the key
    ///
    /// Both were measured rather than assumed. Pipeline: the frame makes **two** pipeline
    /// changes for 17,299 draws, because there is one mesh pipeline — ordering by it can
    /// remove nothing. Depth: front-to-back ordering pays for itself through early-Z, and
    /// this renderer is not pixel-bound — dropping from 1920x1080 to 480x270 changes GPU
    /// time by nothing measurable. Adding either field would be sorting by something with
    /// no demonstrated cost.
    pub fn group_by_material_and_mesh(&mut self, material_count: usize, mesh_count: usize) {
        self.order.clear();
        self.batches.clear();
        if self.is_empty() {
            return;
        }

        let buckets = material_count.saturating_mul(mesh_count);
        if buckets == 0 || buckets > MAX_GROUPING_BUCKETS {
            self.batch_in_gather_order();
            return;
        }

        // Histogram. `counts` is reused across frames, so this is a fill rather than an
        // allocation once the scene has been seen.
        self.counts.clear();
        self.counts.resize(buckets + 1, 0);
        let key_of = |mesh: MeshHandle, material: MaterialHandle| -> Option<usize> {
            let (mesh, material) = (mesh.index(), material.index());
            (mesh < mesh_count && material < material_count).then_some(material * mesh_count + mesh)
        };

        for (mesh, material) in self.meshes.iter().zip(&self.materials) {
            let Some(key) = key_of(*mesh, *material) else {
                // A handle outside the renderer's arrays cannot be bucketed. Recording
                // rejects it by name a moment later, which is a better error than a
                // silently mis-sorted frame.
                self.batch_in_gather_order();
                return;
            };
            self.counts[key + 1] += 1;
        }

        // Prefix sum turns counts into the first output index of each bucket.
        for index in 1..self.counts.len() {
            self.counts[index] += self.counts[index - 1];
        }

        // Scatter. Entries land in bucket order, and within a bucket in gather order —
        // so the grouping is stable and two runs of the same build produce the same frame.
        self.starts.clear();
        self.starts.extend_from_slice(&self.counts[..buckets]);
        self.order.resize(self.len(), 0);
        for (index, (mesh, material)) in self.meshes.iter().zip(&self.materials).enumerate() {
            let Some(key) = key_of(*mesh, *material) else {
                self.batch_in_gather_order();
                return;
            };
            let slot = self.counts[key] as usize;
            self.counts[key] += 1;
            self.order[slot] = index as u32;
        }

        for key in 0..buckets {
            let start = self.starts[key];
            let end = self.counts[key];
            if end == start {
                continue;
            }
            let first_entry = self.order[start as usize] as usize;
            self.batches.push(RenderBatch {
                mesh: self.meshes[first_entry],
                material: self.materials[first_entry],
                first: start,
                count: end - start,
            });
        }
    }

    /// Leaves the entries in gather order and derives batches from the runs already there.
    ///
    /// The fallback when grouping cannot help. Correct, and exactly what the ungrouped
    /// recording loop did before batches existed — consecutive entries that happen to share
    /// a mesh and material become one batch, and the rest are batches of one.
    fn batch_in_gather_order(&mut self) {
        self.order.clear();
        self.order.extend(0..self.len() as u32);
        self.batches.clear();

        let mut start = 0_u32;
        for index in 1..=self.len() {
            let ends = index == self.len()
                || self.meshes[index] != self.meshes[start as usize]
                || self.materials[index] != self.materials[start as usize];
            if ends {
                self.batches.push(RenderBatch {
                    mesh: self.meshes[start as usize],
                    material: self.materials[start as usize],
                    first: start,
                    count: index as u32 - start,
                });
                start = index as u32;
            }
        }
    }

    /// The order recording should walk, as indices into the payload arrays.
    ///
    /// Empty until [`RenderList::group_by_material_and_mesh`] has run.
    #[must_use]
    pub fn order(&self) -> &[u32] {
        &self.order
    }

    /// The runs of entries sharing a mesh and a material.
    ///
    /// Empty until [`RenderList::group_by_material_and_mesh`] has run. Their `count`s sum
    /// to [`RenderList::len`].
    #[must_use]
    pub fn batches(&self) -> &[RenderBatch] {
        &self.batches
    }

    /// One entry, gathered. `None` past the end.
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<RenderListEntry> {
        Some(RenderListEntry {
            entity: *self.entities.get(index)?,
            transform: *self.transforms.get(index)?,
            mesh: *self.meshes.get(index)?,
            material: *self.materials.get(index)?,
            style: *self.styles.get(index)?,
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
            list.push(
                entity,
                transform,
                mesh,
                material,
                InstanceStyle::NEUTRAL,
                true,
            );
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
            InstanceStyle::NEUTRAL,
            true,
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
            list.push(
                entity,
                transform,
                mesh,
                material,
                InstanceStyle::NEUTRAL,
                true,
            );
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
            list.push(
                entity,
                transform,
                mesh,
                material,
                InstanceStyle::NEUTRAL,
                true,
            );
        }
        let capacity = list.capacity();
        assert!(capacity >= 64);

        for _ in 0..10 {
            list.clear();
            for index in 0..64 {
                let (entity, transform, mesh, material) = entry(index);
                list.push(
                    entity,
                    transform,
                    mesh,
                    material,
                    InstanceStyle::NEUTRAL,
                    true,
                );
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
