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

use engine_core::math::Vec3;
use engine_core::transform::Transform;
use engine_render_data::{InstanceStyle, MaterialHandle, MeshHandle, RenderEntity};

use crate::material::SurfaceClass;
use crate::mesh_pipeline::InstanceTransform;
use crate::upload::UploadRing;

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

/// Alignment for a per-instance vertex stream in the upload ring.
///
/// A vertex buffer view has no 256-byte requirement the way a constant buffer view does;
/// what it needs is an address aligned to its largest element, so the input assembler's
/// fetches do not straddle a boundary. Every element in the stream is a `float4` or
/// smaller, so sixteen bytes is enough.
///
/// It is deliberately **not** `size_of::<InstanceTransform>()`, which is what it used to
/// be: the record grew to 96 bytes when it gained a tint, and 96 is not a power of two —
/// which the ring's aligner requires and which turned an alignment constant into a panic.
pub(crate) const INSTANCE_STREAM_ALIGNMENT: usize = 16;

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
    ///
    /// Dense over the whole key space but kept all-zero between calls: the grouping resets
    /// exactly the counters it touched, so it never clears the array whole.
    counts: Vec<u32>,
    /// The distinct keys the last grouping saw, reused across frames.
    ///
    /// Every buffer here is held rather than allocated per call. The bucket starts used to
    /// be a `to_vec()` inside the pass — one heap allocation of `material_count *
    /// mesh_count` words **every frame**, which went unnoticed while the scene had nine
    /// materials and four meshes and showed up the moment the environment brought forty of
    /// each. `env_bench` counts it.
    touched: Vec<u32>,
    /// Each entry's key, so the scatter does not recompute it, reused across frames.
    keys: Vec<u32>,
    /// `(squared distance from the eye, entry index)` for the depth sort, reused across
    /// frames.
    ///
    /// Only the transparent list ever fills it. Held here rather than passed in so the sort
    /// allocates nothing after the first frame that sees its high-water mark, exactly as the
    /// counting sort's histogram does.
    depth_keys: Vec<(f32, u32)>,
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
            touched: Vec::new(),
            keys: Vec::new(),
            depth_keys: Vec::new(),
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

    /// Moves every entry whose material is [`SurfaceClass::Transparent`] into `out`.
    ///
    /// The split that creates the two phases. What is left in `self` is the opaque phase -
    /// opaque and alpha-tested surfaces, in the order the cull produced them, which is the
    /// same order and the same content the list held before transparency existed. `out` is the
    /// transparent phase, still unsorted; [`RenderList::sort_back_to_front`] is what orders it.
    ///
    /// # Why here and not in the cull
    ///
    /// The culler answers "is it on screen", which it can do from the world's bounds alone. It
    /// has no material table and should not grow one: the class of a material is a fact the
    /// renderer owns, and threading it into a parallel cull would put a shared lookup inside
    /// the hot loop for a question that can be asked once afterwards, over the visible set
    /// only.
    ///
    /// # Cost when nothing is transparent
    ///
    /// One predicate pass over the material handles and nothing else - no writes, no copy, no
    /// allocation. A scene with no transparent materials pays a linear scan of an 8-byte-per-
    /// entry array and leaves the opaque list byte-identical, which is what makes it safe to
    /// call unconditionally every frame.
    ///
    /// Both lists' `order` and `batches` are invalidated, because the entries they index have
    /// moved. Group or sort afterwards; recording reads neither until then.
    pub fn split_transparent(&mut self, surfaces: &[SurfaceClass], out: &mut Self) {
        out.clear();
        self.order.clear();
        self.batches.clear();

        // A handle past the end of the table names a material that does not exist. It is
        // classified opaque and left in place, so recording rejects it by name a moment later
        // - a clear error beats a silently relocated draw.
        let class = |material: MaterialHandle| {
            surfaces
                .get(material.index())
                .copied()
                .unwrap_or(SurfaceClass::Opaque)
        };

        if !self
            .materials
            .iter()
            .any(|handle| class(*handle).is_transparent())
        {
            return;
        }

        let mut kept = 0;
        for index in 0..self.len() {
            if class(self.materials[index]).is_transparent() {
                out.push(
                    self.entities[index],
                    self.transforms[index],
                    self.meshes[index],
                    self.materials[index],
                    self.styles[index],
                    self.receives_shadow[index],
                );
                continue;
            }
            if kept != index {
                self.entities[kept] = self.entities[index];
                self.transforms[kept] = self.transforms[index];
                self.meshes[kept] = self.meshes[index];
                self.materials[kept] = self.materials[index];
                self.styles[kept] = self.styles[index];
                self.receives_shadow[kept] = self.receives_shadow[index];
            }
            kept += 1;
        }
        self.truncate(kept);
    }

    /// Moves every entry `moves` accepts into `out`, keeping both lists in gather order.
    ///
    /// The general form of [`RenderList::split_transparent`], for a split decided by the
    /// entity and its style rather than by its material: the renderer uses it to take the
    /// entries the camera may dither this frame out of the list its depth prepass draws.
    /// Costs one predicate call per entry and nothing else when it moves nothing.
    ///
    /// Both lists' `order` and `batches` are invalidated, as with the transparent split.
    pub fn split_where(
        &mut self,
        out: &mut Self,
        mut moves: impl FnMut(RenderEntity, InstanceStyle) -> bool,
    ) {
        out.clear();
        self.order.clear();
        self.batches.clear();

        let mut kept = 0;
        for index in 0..self.len() {
            if moves(self.entities[index], self.styles[index]) {
                out.push(
                    self.entities[index],
                    self.transforms[index],
                    self.meshes[index],
                    self.materials[index],
                    self.styles[index],
                    self.receives_shadow[index],
                );
                continue;
            }
            if kept != index {
                self.entities[kept] = self.entities[index];
                self.transforms[kept] = self.transforms[index];
                self.meshes[kept] = self.meshes[index];
                self.materials[kept] = self.materials[index];
                self.styles[kept] = self.styles[index];
                self.receives_shadow[kept] = self.receives_shadow[index];
            }
            kept += 1;
        }
        self.truncate(kept);
    }

    /// Composes every entry's per-instance data into one allocation of this frame's upload
    /// ring, in the order the batches walk, and returns the view a batch loop binds once.
    ///
    /// Batch `b`'s instances then start at `b.first`, so each batch is one draw with
    /// `start_instance = first` and the instance stream is bound once per pass rather than once
    /// per batch. It is also what lets two passes over the same list share one copy: the
    /// camera's depth prepass and the camera pass draw the same list and bind the same view.
    /// `None` for an empty list. Group first; the batches index the order this writes in.
    ///
    /// # Errors
    ///
    /// Returns [`crate::RendererError::InvalidConfig`] if the ring cannot fit the instances.
    pub(crate) fn upload_instances(
        &self,
        uploads: &mut UploadRing,
        slot: usize,
        scratch: &mut Vec<InstanceTransform>,
    ) -> Result<
        Option<windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW>,
        crate::RendererError,
    > {
        if self.order.is_empty() {
            return Ok(None);
        }
        scratch.clear();
        scratch.extend(self.order.iter().map(|&entry| {
            let entry = entry as usize;
            InstanceTransform::shadowed(
                self.transforms[entry].matrix(),
                self.styles[entry],
                self.receives_shadow[entry],
            )
        }));
        let allocation = uploads.write(slot, scratch, INSTANCE_STREAM_ALIGNMENT)?;
        Ok(Some(
            windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                BufferLocation: allocation.gpu_address(),
                SizeInBytes: std::mem::size_of_val(scratch.as_slice()) as u32,
                StrideInBytes: size_of::<InstanceTransform>() as u32,
            },
        ))
    }

    /// Shortens every payload array to `length`, keeping the allocations.
    fn truncate(&mut self, length: usize) {
        self.entities.truncate(length);
        self.transforms.truncate(length);
        self.meshes.truncate(length);
        self.materials.truncate(length);
        self.styles.truncate(length);
        self.receives_shadow.truncate(length);
    }

    /// Fills [`RenderList::order`] with the entries sorted farthest-first from `eye`.
    ///
    /// The transparent phase's whole ordering rule. Blending is not commutative - `a` over `b`
    /// is a different colour from `b` over `a` - and the transparent pipeline writes no depth,
    /// so nothing but paint order decides the result. Farthest first is what makes that order
    /// the correct one.
    ///
    /// # The key is the object's origin, and that is a real limitation
    ///
    /// One distance per entry, from the entry's world-space translation. It is exact for the
    /// things this phase is for - a particle-like card, a spell quad, a small effect volume -
    /// and it is wrong for two cases that are worth naming rather than discovering:
    ///
    /// * **Intersecting transparent surfaces.** Two quads that pass through each other have no
    ///   correct back-to-front order at all; per-object sorting picks one and the intersection
    ///   shows a hard seam. Nothing short of per-fragment ordering fixes it.
    /// * **Large or concave transparent meshes.** A single long transparent object can be both
    ///   nearer and farther than another; its origin answers for the whole of it.
    ///
    /// Both are the accepted cost of the simplest correct back-to-front path. The alternatives
    /// - sorting triangles, or an order-independent method - are deliberately not built.
    ///
    /// # Determinism
    ///
    /// The order is a pure function of the list's contents and `eye`: distances are compared
    /// first and ties are broken by the entry's position in the list, so equal-distance
    /// entries keep gather order and two runs of the same frame record the same draws. A
    /// non-finite distance - which a NaN transform would produce - sorts as the farthest
    /// possible value rather than making the comparison inconsistent, because a sort whose
    /// comparator is not a total order can produce a different permutation on each run.
    pub fn sort_back_to_front(&mut self, eye: Vec3) {
        self.order.clear();
        self.batches.clear();
        if self.is_empty() {
            return;
        }

        self.depth_keys.clear();
        self.depth_keys.reserve(self.len());
        for (index, transform) in self.transforms.iter().enumerate() {
            let offset = transform.translation - eye;
            let distance = offset.dot(offset);
            // Squared, not the distance itself: the ordering is identical and a square root
            // per visible transparent object buys nothing.
            let distance = if distance.is_finite() {
                distance
            } else {
                f32::MAX
            };
            self.depth_keys.push((distance, index as u32));
        }

        self.depth_keys.sort_unstable_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(left.1.cmp(&right.1))
        });
        self.order
            .extend(self.depth_keys.iter().map(|(_, index)| *index));
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
    /// # Sparse: only the keys the list uses are visited
    ///
    /// The histogram is a dense array of `material_count * mesh_count` counters, but the
    /// prefix sum and the batch pass walk only the keys this list actually contains, sorted —
    /// and only those counters are reset afterwards, so the array stays zero between calls
    /// and is never cleared whole. The difference is the shadow stage: a game level has
    /// thousands of possible keys (78 materials x 94 meshes in the graveyard) and each of its
    /// ten shadow views holds a few dozen of them. Walking and clearing every bucket for every
    /// view cost more than the instancing saved; walking the few dozen costs nothing. The
    /// output is the same order the dense walk produced: ascending key, gather order within a
    /// key.
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

        // The dense histogram, grown once to the largest key space seen and otherwise kept
        // all-zero between calls: every counter this call touches is reset before it
        // returns, so nothing ever clears the whole array.
        if self.counts.len() < buckets {
            self.counts.resize(buckets, 0);
        }
        let key_of = |mesh: MeshHandle, material: MaterialHandle| -> Option<u32> {
            let (mesh, material) = (mesh.index(), material.index());
            (mesh < mesh_count && material < material_count)
                .then_some((material * mesh_count + mesh) as u32)
        };

        // Histogram, noting each key the first time it appears.
        self.keys.clear();
        self.touched.clear();
        let mut bucketable = true;
        for (mesh, material) in self.meshes.iter().zip(&self.materials) {
            let Some(key) = key_of(*mesh, *material) else {
                bucketable = false;
                break;
            };
            let count = &mut self.counts[key as usize];
            if *count == 0 {
                self.touched.push(key);
            }
            *count += 1;
            self.keys.push(key);
        }
        if !bucketable {
            // A handle outside the renderer's arrays cannot be bucketed. Recording rejects it
            // by name a moment later, which is a better error than a silently mis-sorted
            // frame.
            self.reset_touched_counts();
            self.batch_in_gather_order();
            return;
        }

        // Ascending key, exactly the order a walk over every bucket visits them in. A few
        // dozen keys per list, so this is the cheap sort.
        self.touched.sort_unstable();

        // Prefix sum over the touched keys only: each counter becomes its bucket's first
        // output index, and the batches are known before the scatter fills them.
        let mut offset = 0_u32;
        for &key in &self.touched {
            let count = std::mem::replace(&mut self.counts[key as usize], offset);
            self.batches.push(RenderBatch {
                // Filled from the bucket's first entry once the scatter has placed it.
                mesh: MeshHandle::INVALID,
                material: MaterialHandle::INVALID,
                first: offset,
                count,
            });
            offset += count;
        }

        // Scatter. Entries land in bucket order, and within a bucket in gather order —
        // so the grouping is stable and two runs of the same build produce the same frame.
        self.order.resize(self.len(), 0);
        for (index, &key) in self.keys.iter().enumerate() {
            let cursor = &mut self.counts[key as usize];
            self.order[*cursor as usize] = index as u32;
            *cursor += 1;
        }

        for batch in &mut self.batches {
            let first_entry = self.order[batch.first as usize] as usize;
            batch.mesh = self.meshes[first_entry];
            batch.material = self.materials[first_entry];
        }
        self.reset_touched_counts();
    }

    /// Returns every counter the last grouping touched to zero, which is the invariant the
    /// sparse histogram relies on between calls.
    fn reset_touched_counts(&mut self) {
        for &key in &self.touched {
            self.counts[key as usize] = 0;
        }
        self.touched.clear();
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

    /// The grouping as it was before it went sparse: every bucket walked, in key order, and
    /// gather order within a bucket. What the sparse version must reproduce exactly.
    fn dense_reference(
        list: &RenderList,
        materials: usize,
        meshes: usize,
    ) -> Vec<(u32, u32, Vec<u32>)> {
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); materials * meshes];
        for index in 0..list.len() {
            let key = list.materials()[index].index() * meshes + list.meshes()[index].index();
            buckets[key].push(index as u32);
        }
        buckets
            .into_iter()
            .enumerate()
            .filter(|(_, entries)| !entries.is_empty())
            .map(|(key, entries)| ((key % meshes) as u32, (key / meshes) as u32, entries))
            .collect()
    }

    fn grouped_runs(list: &RenderList) -> Vec<(u32, u32, Vec<u32>)> {
        list.batches()
            .iter()
            .map(|batch| {
                let first = batch.first as usize;
                (
                    batch.mesh.index() as u32,
                    batch.material.index() as u32,
                    list.order()[first..first + batch.count as usize].to_vec(),
                )
            })
            .collect()
    }

    #[test]
    fn the_sparse_grouping_matches_a_walk_over_every_bucket() {
        // A large key space with few keys in use — the shadow views' shape — and several
        // calls on one list with different sizes and key spaces, so a counter left non-zero
        // by one call would corrupt the next.
        let mut list = RenderList::new();
        let mut seed = 0x2545_f491_u32;
        for (count, materials, meshes) in [
            (40, 78, 94),
            (300, 78, 94),
            (7, 3, 2),
            (1, 78, 94),
            (120, 9, 4),
        ] {
            list.clear();
            for index in 0..count {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                // A handful of distinct keys, as a shadow view has.
                let mesh = (seed % 6) * (meshes as u32 / 6).max(1) % meshes as u32;
                let material = (seed / 7 % 4) * (materials as u32 / 4).max(1) % materials as u32;
                list.push(
                    RenderEntity::new(index, 1),
                    Transform::IDENTITY,
                    MeshHandle::new(mesh, 1),
                    MaterialHandle::new(material, 1),
                    InstanceStyle::NEUTRAL,
                    true,
                );
            }
            list.group_by_material_and_mesh(materials, meshes);
            assert_eq!(
                grouped_runs(&list),
                dense_reference(&list, materials, meshes),
                "{count} entries over {materials}x{meshes}"
            );
            assert!(
                list.counts.iter().all(|&count| count == 0),
                "every touched counter is reset"
            );
        }
    }

    #[test]
    fn a_handle_outside_the_key_space_falls_back_and_leaves_the_counters_clean() {
        let mut list = filled(30);
        // Material 4 exists in the list; a key space of four materials does not hold it.
        list.group_by_material_and_mesh(4, 3);
        assert_eq!(list.batches().iter().map(|b| b.count).sum::<u32>(), 30);
        assert!(list.counts.iter().all(|&count| count == 0));
        // And the next, valid call groups normally.
        list.group_by_material_and_mesh(5, 3);
        assert_eq!(list.batches().len(), 15);
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

    /// A material table where handle `index` is transparent iff `index` is in `transparent`.
    fn surfaces(count: usize, transparent: &[usize]) -> Vec<SurfaceClass> {
        (0..count)
            .map(|index| {
                if transparent.contains(&index) {
                    SurfaceClass::Transparent
                } else {
                    SurfaceClass::Opaque
                }
            })
            .collect()
    }

    #[test]
    fn a_list_with_no_transparent_material_is_left_exactly_as_it_was() {
        // The property the whole feature rests on: a scene that uses no transparency must be
        // the scene it was before transparency existed, entry for entry and in the same order.
        // A split that reordered or copied would change every opaque frame in the game.
        let mut list = filled(37);
        let before = list.clone();
        let mut transparent = RenderList::new();

        // `filled` uses materials 0..5, none of which is transparent.
        list.split_transparent(&surfaces(5, &[]), &mut transparent);

        assert!(transparent.is_empty(), "nothing should have moved");
        assert_eq!(list.len(), before.len());
        for index in 0..before.len() {
            assert_eq!(list.entry(index), before.entry(index), "entry {index}");
        }
    }

    #[test]
    fn the_split_moves_exactly_the_transparent_entries_and_keeps_both_orders() {
        // Material 2 is transparent; `filled` assigns material `index % 5`. Both halves must
        // come out in gather order - the opaque phase's batching assumes it, and the
        // transparent sort breaks its ties on it.
        let mut list = filled(20);
        let mut transparent = RenderList::new();
        list.split_transparent(&surfaces(5, &[2]), &mut transparent);

        assert_eq!(transparent.len(), 4, "20 entries, every fifth from index 2");
        assert_eq!(list.len(), 16);

        for handle in transparent.materials() {
            assert_eq!(handle.index(), 2);
        }
        for handle in list.materials() {
            assert_ne!(handle.index(), 2, "a transparent entry stayed behind");
        }

        let opaque_entities: Vec<_> = list.entities().iter().map(|e| e.index()).collect();
        let mut sorted = opaque_entities.clone();
        sorted.sort_unstable();
        assert_eq!(
            opaque_entities, sorted,
            "the opaque half must keep gather order"
        );

        let moved: Vec<_> = transparent.entities().iter().map(|e| e.index()).collect();
        assert_eq!(moved, vec![2, 7, 12, 17]);
    }

    #[test]
    fn the_split_invalidates_the_order_and_the_batches() {
        // Both index entries that have just moved. Leaving them in place would let a caller
        // that forgot to regroup record last frame's permutation against this frame's arrays.
        let mut list = filled(12);
        list.group_by_material_and_mesh(5, 3);
        assert!(!list.order().is_empty() && !list.batches().is_empty());

        let mut transparent = RenderList::new();
        list.split_transparent(&surfaces(5, &[1]), &mut transparent);
        assert!(list.order().is_empty());
        assert!(list.batches().is_empty());
    }

    #[test]
    fn a_material_handle_past_the_table_is_treated_as_opaque() {
        // Recording rejects an unknown material by name a moment later, which is a far better
        // diagnostic than an object silently relocated into another phase.
        let mut list = filled(6);
        let mut transparent = RenderList::new();
        list.split_transparent(&[], &mut transparent);
        assert!(transparent.is_empty());
        assert_eq!(list.len(), 6);
    }

    #[test]
    fn the_sort_orders_entries_farthest_from_the_eye_first() {
        // The ordering rule itself. Blending is not commutative and the transparent pipeline
        // writes no depth, so this order *is* the picture.
        let mut list = RenderList::new();
        for (index, z) in [5.0_f32, 1.0, 3.0].into_iter().enumerate() {
            list.push(
                RenderEntity::new(index as u32, 1),
                Transform::from_translation(Vec3::new(0.0, 0.0, z)),
                MeshHandle::new(0, 1),
                MaterialHandle::new(0, 1),
                InstanceStyle::NEUTRAL,
                true,
            );
        }

        // An eye at the origin: the entry at z = 5 is farthest and must be drawn first.
        list.sort_back_to_front(Vec3::ZERO);
        assert_eq!(list.order(), &[0, 2, 1]);

        // Move the eye past all three and the order reverses, which is what proves the key is
        // the distance to the eye rather than a fixed axis.
        list.sort_back_to_front(Vec3::new(0.0, 0.0, 10.0));
        assert_eq!(list.order(), &[1, 2, 0]);
    }

    #[test]
    fn the_sort_is_deterministic_and_breaks_ties_by_gather_order() {
        // Two runs of one frame must record the same draws. Equal distances are broken by
        // position in the list, so the answer is a pure function of the input rather than
        // whatever the sort's internals happened to do.
        let mut list = RenderList::new();
        for index in 0..8_u32 {
            list.push(
                RenderEntity::new(index, 1),
                // Every entry at the same distance from the eye below.
                Transform::from_translation(Vec3::new(1.0, 0.0, 0.0)),
                MeshHandle::new(0, 1),
                MaterialHandle::new(0, 1),
                InstanceStyle::NEUTRAL,
                true,
            );
        }

        list.sort_back_to_front(Vec3::ZERO);
        let first: Vec<u32> = list.order().to_vec();
        assert_eq!(first, (0..8).collect::<Vec<u32>>());

        for _ in 0..8 {
            list.sort_back_to_front(Vec3::ZERO);
            assert_eq!(
                list.order(),
                first.as_slice(),
                "the sort must be stable run to run"
            );
        }
    }

    #[test]
    fn a_non_finite_transform_does_not_make_the_sort_inconsistent() {
        // A NaN distance would make the comparator not a total order, and a sort with an
        // inconsistent comparator can produce a different permutation on each run - which is a
        // frame that flickers for reasons nothing in the renderer would explain.
        let mut list = RenderList::new();
        list.push(
            RenderEntity::new(0, 1),
            Transform::from_translation(Vec3::new(f32::NAN, 0.0, 0.0)),
            MeshHandle::new(0, 1),
            MaterialHandle::new(0, 1),
            InstanceStyle::NEUTRAL,
            true,
        );
        list.push(
            RenderEntity::new(1, 1),
            Transform::from_translation(Vec3::new(0.0, 0.0, 1.0)),
            MeshHandle::new(0, 1),
            MaterialHandle::new(0, 1),
            InstanceStyle::NEUTRAL,
            true,
        );

        list.sort_back_to_front(Vec3::ZERO);
        // The NaN sorts as the farthest possible value, so it is drawn first and the finite
        // entry follows. The point is that the answer exists and repeats, not which one it is.
        assert_eq!(list.order(), &[0, 1]);
        let first: Vec<u32> = list.order().to_vec();
        list.sort_back_to_front(Vec3::ZERO);
        assert_eq!(list.order(), first.as_slice());
    }

    #[test]
    fn sorting_an_empty_list_produces_an_empty_order() {
        let mut list = RenderList::new();
        list.sort_back_to_front(Vec3::ZERO);
        assert!(list.order().is_empty());
    }

    #[test]
    fn the_split_and_the_sort_stop_allocating_once_warmed_up() {
        // Both run every frame. If either kept allocating, the transparent phase would put a
        // heap allocation in the frame for a list that is the same size as last frame's.
        let mut list = filled(64);
        let mut transparent = RenderList::new();
        let table = surfaces(5, &[3]);

        for _ in 0..4 {
            list.split_transparent(&table, &mut transparent);
            transparent.sort_back_to_front(Vec3::ZERO);
            list = filled(64);
        }
        let capacity = transparent.capacity();
        assert!(capacity > 0);

        for _ in 0..8 {
            list = filled(64);
            list.split_transparent(&table, &mut transparent);
            transparent.sort_back_to_front(Vec3::ZERO);
            assert_eq!(
                transparent.capacity(),
                capacity,
                "refilling to the same size must reuse the allocation"
            );
        }
    }

    #[test]
    fn a_sized_list_starts_with_room() {
        let list = RenderList::with_capacity(1024);
        assert!(list.capacity() >= 1024);
        assert!(list.is_empty(), "capacity is not content");
    }
}
