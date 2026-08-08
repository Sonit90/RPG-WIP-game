//! The static collision world: what the level is made of, and how to ask what is nearby.
//!
//! ```text
//! CollisionWorld
//!   +-- shapes    Plane | Aabb | Obb
//!   +-- filters   layer + mask, one per shape
//!   +-- proxies   bounds + layer, one per shape   -> the broad phase reads only these
//!   +-- broad     BroadPhase::Linear | Grid
//! ```
//!
//! Static means static: shapes are added when a scene is built and removed when it is torn
//! down. There are no rigid bodies here, no velocities, no solver and no dynamics — a
//! character controller is the only thing that moves, and it moves itself.
//!
//! ## Generational handles
//!
//! A [`StaticId`] carries the generation of the slot it names, so a handle to a removed
//! object cannot address whatever is added into its place. The world's entity allocator uses
//! the same scheme for the same reason.

use engine_core::math::{Aabb, Plane, Vec3};

use crate::broad::{BroadPhase, Proxy};
use crate::contact::{QueryStats, StaticContact};
use crate::layer::{CollisionFilter, CollisionMask};
use crate::narrow;
use crate::shape::{Capsule, Obb, Shape};

/// A handle to one static object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StaticId {
    index: u32,
    generation: u32,
}

impl StaticId {
    /// A handle that names nothing.
    pub const INVALID: Self = Self {
        index: u32::MAX,
        generation: 0,
    };

    /// The slot this handle names.
    #[must_use]
    pub fn index(self) -> usize {
        self.index as usize
    }

    /// The generation this handle was issued for.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// What a caller supplies to add a static object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticDesc {
    /// The geometry.
    pub shape: Shape,
    /// What layer it is on and what it collides with.
    pub filter: CollisionFilter,
    /// A short name, for a debug readout. Never used to decide anything.
    ///
    /// `&'static str` rather than `String`: a level's surface names are literals, and a
    /// heap allocation per wall is a heap allocation per wall.
    pub label: &'static str,
}

impl StaticDesc {
    /// Static world geometry with no name.
    #[must_use]
    pub fn new(shape: Shape) -> Self {
        Self {
            shape,
            filter: CollisionFilter::world(),
            label: "",
        }
    }

    /// The same object, named.
    #[must_use]
    pub fn labelled(self, label: &'static str) -> Self {
        Self { label, ..self }
    }

    /// The same object on a different layer.
    #[must_use]
    pub fn with_filter(self, filter: CollisionFilter) -> Self {
        Self { filter, ..self }
    }
}

/// Scratch space a query fills, owned by the caller and reused.
///
/// The reason it exists: resolving one character move runs several queries, and every one
/// of them needs somewhere to put candidates and contacts. Allocating that per query would
/// be sixty allocations a second for a single character, which the engine's frame budget
/// does not have room for.
#[derive(Debug, Default, Clone)]
pub struct QueryBuffers {
    /// Broad-phase output. Not public: it is an index into the world's internals.
    candidates: Vec<u32>,
    /// Narrow-phase output.
    contacts: Vec<StaticContact>,
}

impl QueryBuffers {
    /// Empty buffers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Buffers with room for `capacity` contacts, so the first frame allocates nothing new.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            candidates: Vec::with_capacity(capacity),
            contacts: Vec::with_capacity(capacity),
        }
    }

    /// What the last query found.
    #[must_use]
    pub fn contacts(&self) -> &[StaticContact] {
        &self.contacts
    }

    /// How much room the buffers hold, for an allocation test.
    #[must_use]
    pub fn capacity(&self) -> (usize, usize) {
        (self.candidates.capacity(), self.contacts.capacity())
    }

    /// Empties both buffers, keeping their storage.
    pub fn clear(&mut self) {
        self.candidates.clear();
        self.contacts.clear();
    }
}

/// Every static collision shape in a scene.
#[derive(Debug, Default, Clone)]
pub struct CollisionWorld {
    shapes: Vec<Shape>,
    filters: Vec<CollisionFilter>,
    labels: Vec<&'static str>,
    generations: Vec<u32>,
    proxies: Vec<Proxy>,
    free: Vec<u32>,
    live: usize,
    broad: BroadPhase,
    /// Whether the broad phase reflects the current proxies.
    ///
    /// A dirty world answers queries with a linear scan, which is correct and slower.
    /// Rebuilding on every insert would make building a 10,000-object level quadratic.
    dirty: bool,
}

impl CollisionWorld {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty world with room for `capacity` objects.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            shapes: Vec::with_capacity(capacity),
            filters: Vec::with_capacity(capacity),
            labels: Vec::with_capacity(capacity),
            generations: Vec::with_capacity(capacity),
            proxies: Vec::with_capacity(capacity),
            ..Self::default()
        }
    }

    /// Adds a static object and returns its handle.
    ///
    /// The broad phase is left dirty; call [`CollisionWorld::rebuild_broad_phase`] once the
    /// scene is built. Forgetting costs speed and never correctness.
    pub fn add_static(&mut self, desc: StaticDesc) -> StaticId {
        let proxy = Proxy {
            bounds: desc.shape.bounds().unwrap_or(Aabb::EMPTY),
            layer: desc.filter.layer,
            live: true,
        };
        self.dirty = true;
        self.live += 1;

        if let Some(index) = self.free.pop() {
            let slot = index as usize;
            self.shapes[slot] = desc.shape;
            self.filters[slot] = desc.filter;
            self.labels[slot] = desc.label;
            self.proxies[slot] = proxy;
            self.generations[slot] += 1;
            return StaticId {
                index,
                generation: self.generations[slot],
            };
        }

        let index = self.shapes.len() as u32;
        self.shapes.push(desc.shape);
        self.filters.push(desc.filter);
        self.labels.push(desc.label);
        self.proxies.push(proxy);
        self.generations.push(1);
        StaticId {
            index,
            generation: 1,
        }
    }

    /// Adds an axis-aligned box of world geometry.
    pub fn add_box(&mut self, min: Vec3, max: Vec3, label: &'static str) -> StaticId {
        self.add_static(StaticDesc::new(Shape::box_between(min, max)).labelled(label))
    }

    /// Adds a box standing on the floor: a footprint and a size.
    pub fn add_block(&mut self, footprint: Vec3, size: Vec3, label: &'static str) -> StaticId {
        self.add_static(StaticDesc::new(Shape::box_standing(footprint, size)).labelled(label))
    }

    /// Adds a tilted slab: a ramp.
    pub fn add_ramp(&mut self, ramp: Obb, label: &'static str) -> StaticId {
        self.add_static(StaticDesc::new(Shape::Obb(ramp)).labelled(label))
    }

    /// Adds an infinite horizontal floor at height `y`.
    pub fn add_ground(&mut self, y: f32, label: &'static str) -> StaticId {
        self.add_static(StaticDesc::new(Shape::ground(y)).labelled(label))
    }

    /// Adds an arbitrary plane.
    pub fn add_plane(&mut self, plane: Plane, label: &'static str) -> StaticId {
        self.add_static(StaticDesc::new(Shape::Plane(plane)).labelled(label))
    }

    /// Removes a static object. A stale handle is reported rather than panicking.
    pub fn remove_static(&mut self, id: StaticId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let slot = id.index();
        self.proxies[slot].live = false;
        self.free.push(id.index);
        self.live -= 1;
        self.dirty = true;
        true
    }

    /// Removes everything, keeping the allocated storage.
    pub fn clear(&mut self) {
        self.shapes.clear();
        self.filters.clear();
        self.labels.clear();
        self.generations.clear();
        self.proxies.clear();
        self.free.clear();
        self.live = 0;
        self.broad = BroadPhase::Linear;
        self.dirty = false;
    }

    /// Whether the handle names a live object.
    #[must_use]
    pub fn contains(&self, id: StaticId) -> bool {
        let slot = id.index();
        slot < self.proxies.len()
            && self.proxies[slot].live
            && self.generations[slot] == id.generation
    }

    /// Live static objects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether the world holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Slots ever allocated, live or free.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.shapes.len()
    }

    /// The handle living in `slot`, if any.
    #[must_use]
    pub fn id_at(&self, slot: usize) -> Option<StaticId> {
        if slot >= self.proxies.len() || !self.proxies[slot].live {
            return None;
        }
        Some(StaticId {
            index: slot as u32,
            generation: self.generations[slot],
        })
    }

    /// Every live object, in slot order.
    ///
    /// Slot order, and that is what makes iteration deterministic: two identically built
    /// worlds iterate identically.
    pub fn statics(&self) -> impl Iterator<Item = StaticId> + '_ {
        (0..self.slot_count()).filter_map(|slot| self.id_at(slot))
    }

    /// An object's shape.
    #[must_use]
    pub fn shape(&self, id: StaticId) -> Option<&Shape> {
        self.contains(id).then(|| &self.shapes[id.index()])
    }

    /// An object's layer and mask.
    #[must_use]
    pub fn filter(&self, id: StaticId) -> Option<CollisionFilter> {
        self.contains(id).then(|| self.filters[id.index()])
    }

    /// An object's debug name.
    #[must_use]
    pub fn label(&self, id: StaticId) -> &'static str {
        if self.contains(id) {
            self.labels[id.index()]
        } else {
            ""
        }
    }

    /// An object's world-space bounds, or `None` for a plane.
    #[must_use]
    pub fn bounds(&self, id: StaticId) -> Option<Aabb> {
        let bounds = self.contains(id).then(|| self.proxies[id.index()].bounds)?;
        (!bounds.is_empty()).then_some(bounds)
    }

    /// Which broad phase the world is using.
    #[must_use]
    pub fn broad_phase(&self) -> &BroadPhase {
        &self.broad
    }

    /// Whether the broad phase is out of date with the objects.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Rebuilds the broad phase for the objects currently in the world.
    ///
    /// Called once after a scene is built. Cheap enough to call again; it is O(N) plus one
    /// pass over the cells.
    pub fn rebuild_broad_phase(&mut self) {
        self.broad = BroadPhase::build(&self.proxies);
        self.dirty = false;
    }

    /// Every contact between `capsule` and the static objects `mask` selects.
    ///
    /// Contacts land in `buffers`, which is cleared first and whose storage is reused.
    /// Returns what the query cost, counted rather than estimated.
    pub fn overlap_capsule(
        &self,
        capsule: &Capsule,
        mask: CollisionMask,
        buffers: &mut QueryBuffers,
    ) -> QueryStats {
        buffers.clear();
        let mut stats = QueryStats::new();
        if mask.is_empty() || self.live == 0 {
            return stats;
        }

        let bounds = capsule.bounds();
        if self.dirty {
            BroadPhase::Linear.query(&self.proxies, &bounds, mask, &mut buffers.candidates);
        } else {
            self.broad
                .query(&self.proxies, &bounds, mask, &mut buffers.candidates);
        }
        stats.broad_candidates = buffers.candidates.len() as u32;

        for &index in &buffers.candidates {
            let slot = index as usize;
            let filter = self.filters[slot];
            stats.narrow_tests += 1;
            let Some(contact) = narrow::capsule_vs_shape(capsule, &self.shapes[slot]) else {
                continue;
            };
            stats.contacts += 1;
            buffers.contacts.push(StaticContact {
                contact,
                id: StaticId {
                    index,
                    generation: self.generations[slot],
                },
                filter,
            });
        }

        stats
    }

    /// Whether `capsule` touches anything `mask` selects.
    ///
    /// Separate from [`CollisionWorld::overlap_capsule`] because "is this position free?" is
    /// a question a spawn point or a teleport asks, and it does not need the contacts.
    pub fn capsule_overlaps(
        &self,
        capsule: &Capsule,
        mask: CollisionMask,
        buffers: &mut QueryBuffers,
    ) -> bool {
        self.overlap_capsule(capsule, mask, buffers);
        !buffers.contacts.is_empty()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::layer::CollisionLayer;

    fn standing(x: f32, y: f32, z: f32) -> Capsule {
        Capsule::upright(Vec3::new(x, y, z), 0.4, 1.8)
    }

    fn world_with_a_floor_and_a_wall() -> (CollisionWorld, StaticId, StaticId) {
        let mut world = CollisionWorld::with_capacity(8);
        let floor = world.add_ground(0.0, "FLOOR");
        let wall = world.add_box(Vec3::new(2.0, 0.0, -5.0), Vec3::new(3.0, 4.0, 5.0), "WALL");
        world.rebuild_broad_phase();
        (world, floor, wall)
    }

    #[test]
    fn a_world_reports_what_was_added_to_it() {
        let (world, floor, wall) = world_with_a_floor_and_a_wall();
        assert_eq!(world.len(), 2);
        assert!(!world.is_empty());
        assert_eq!(world.label(floor), "FLOOR");
        assert_eq!(world.label(wall), "WALL");
        assert!(matches!(world.shape(floor), Some(Shape::Plane(_))));
        assert_eq!(world.filter(wall), Some(CollisionFilter::world()));
        assert_eq!(world.statics().count(), 2);
    }

    #[test]
    fn a_plane_has_no_bounds_and_a_box_does() {
        let (world, floor, wall) = world_with_a_floor_and_a_wall();
        assert_eq!(world.bounds(floor), None);
        assert_eq!(
            world.bounds(wall),
            Some(Aabb::new(
                Vec3::new(2.0, 0.0, -5.0),
                Vec3::new(3.0, 4.0, 5.0)
            ))
        );
    }

    #[test]
    fn a_capsule_standing_on_the_floor_finds_the_floor_and_nothing_else() {
        let (world, floor, _) = world_with_a_floor_and_a_wall();
        let mut buffers = QueryBuffers::new();
        let stats = world.overlap_capsule(
            &standing(0.0, -0.05, 0.0),
            CollisionMask::WORLD,
            &mut buffers,
        );

        assert_eq!(buffers.contacts().len(), 1);
        assert_eq!(buffers.contacts()[0].id, floor);
        assert_eq!(buffers.contacts()[0].contact.normal, Vec3::Y);
        assert_eq!(stats.contacts, 1);
        assert!(stats.narrow_tests >= 1);
    }

    #[test]
    fn a_capsule_in_the_corner_of_a_wall_and_the_floor_finds_both() {
        let (world, _, _) = world_with_a_floor_and_a_wall();
        let mut buffers = QueryBuffers::new();
        world.overlap_capsule(
            &standing(1.7, -0.05, 0.0),
            CollisionMask::WORLD,
            &mut buffers,
        );
        assert_eq!(buffers.contacts().len(), 2, "the floor and the wall");
    }

    #[test]
    fn a_mask_that_asks_for_nothing_costs_nothing() {
        let (world, _, _) = world_with_a_floor_and_a_wall();
        let mut buffers = QueryBuffers::new();
        let stats =
            world.overlap_capsule(&standing(0.0, -1.0, 0.0), CollisionMask::NONE, &mut buffers);
        assert_eq!(stats, QueryStats::new());
        assert!(buffers.contacts().is_empty());
    }

    #[test]
    fn filtering_keeps_a_non_blocking_object_out_of_the_results() {
        let mut world = CollisionWorld::new();
        world.add_ground(0.0, "FLOOR");
        world.add_static(
            StaticDesc::new(Shape::box_standing(Vec3::ZERO, Vec3::splat(2.0)))
                .with_filter(CollisionFilter::dummy())
                .labelled("DUMMY"),
        );
        world.rebuild_broad_phase();

        let mut buffers = QueryBuffers::new();
        world.overlap_capsule(
            &standing(0.0, -0.05, 0.0),
            CollisionMask::WORLD,
            &mut buffers,
        );
        assert_eq!(
            buffers.contacts().len(),
            1,
            "the dummy is present and does not block"
        );
    }

    #[test]
    fn a_removed_object_stops_colliding_and_its_handle_goes_stale() {
        let (mut world, _, wall) = world_with_a_floor_and_a_wall();
        assert!(world.remove_static(wall));
        assert!(!world.contains(wall));
        assert!(!world.remove_static(wall), "removing twice is refused");
        assert_eq!(world.len(), 1);
        assert_eq!(world.shape(wall), None);
        assert_eq!(world.label(wall), "");

        let mut buffers = QueryBuffers::new();
        world.overlap_capsule(
            &standing(1.7, -0.05, 0.0),
            CollisionMask::WORLD,
            &mut buffers,
        );
        assert_eq!(buffers.contacts().len(), 1, "only the floor is left");
    }

    #[test]
    fn a_recycled_slot_does_not_answer_to_the_old_handle() {
        let (mut world, _, wall) = world_with_a_floor_and_a_wall();
        world.remove_static(wall);
        let replacement = world.add_box(Vec3::ZERO, Vec3::ONE, "NEW");
        assert_eq!(replacement.index(), wall.index(), "the slot was reused");
        assert_ne!(replacement.generation(), wall.generation());
        assert!(!world.contains(wall));
        assert!(world.contains(replacement));
        assert_eq!(world.label(replacement), "NEW");
    }

    #[test]
    fn a_dirty_world_answers_the_same_as_a_rebuilt_one() {
        // The correctness guarantee behind the lazy rebuild: forgetting to rebuild costs
        // speed, never an answer.
        let mut world = CollisionWorld::new();
        for index in 0..200 {
            let x = (index % 20) as f32 * 3.0;
            let z = (index / 20) as f32 * 3.0;
            world.add_block(Vec3::new(x, 0.0, z), Vec3::splat(1.0), "BLOCK");
        }
        assert!(world.is_dirty());

        let capsule = standing(3.0, 0.0, 0.0);
        let mut dirty_buffers = QueryBuffers::new();
        world.overlap_capsule(&capsule, CollisionMask::WORLD, &mut dirty_buffers);
        let dirty: Vec<StaticId> = dirty_buffers.contacts().iter().map(|c| c.id).collect();

        world.rebuild_broad_phase();
        assert!(!world.is_dirty());
        let mut clean_buffers = QueryBuffers::new();
        world.overlap_capsule(&capsule, CollisionMask::WORLD, &mut clean_buffers);
        let clean: Vec<StaticId> = clean_buffers.contacts().iter().map(|c| c.id).collect();

        assert_eq!(dirty, clean);
        assert!(!dirty.is_empty(), "the query found something to compare");
    }

    #[test]
    fn a_large_world_switches_to_the_grid_and_still_finds_the_right_thing() {
        let mut world = CollisionWorld::with_capacity(1_024);
        world.add_ground(0.0, "FLOOR");
        let mut expected = StaticId::INVALID;
        for index in 0..1_000 {
            let x = (index % 40) as f32 * 4.0;
            let z = (index / 40) as f32 * 4.0;
            let id = world.add_block(Vec3::new(x, 0.0, z), Vec3::splat(1.0), "BLOCK");
            if index == 0 {
                expected = id;
            }
        }
        world.rebuild_broad_phase();
        assert_eq!(world.broad_phase().name(), "GRID");

        let mut buffers = QueryBuffers::new();
        let stats =
            world.overlap_capsule(&standing(0.9, 0.0, 0.0), CollisionMask::WORLD, &mut buffers);
        assert!(
            buffers.contacts().iter().any(|c| c.id == expected),
            "the block next door was missed"
        );
        assert!(
            stats.broad_candidates < 30,
            "the grid offered {} of 1,001 candidates",
            stats.broad_candidates
        );
    }

    #[test]
    fn repeated_queries_stop_allocating() {
        // The frame-budget guarantee: after the buffers have reached their high-water mark,
        // a query writes into storage that already exists.
        let (world, _, _) = world_with_a_floor_and_a_wall();
        let mut buffers = QueryBuffers::with_capacity(8);
        for _ in 0..10 {
            world.overlap_capsule(
                &standing(1.7, -0.05, 0.0),
                CollisionMask::WORLD,
                &mut buffers,
            );
        }
        let capacity = buffers.capacity();

        for step in 0..1_000 {
            let x = 1.7 + (step % 7) as f32 * 0.01;
            world.overlap_capsule(&standing(x, -0.05, 0.0), CollisionMask::WORLD, &mut buffers);
        }
        assert_eq!(buffers.capacity(), capacity, "a query allocated");
    }

    #[test]
    fn an_overlap_test_answers_yes_or_no_without_the_contacts() {
        let (world, _, _) = world_with_a_floor_and_a_wall();
        let mut buffers = QueryBuffers::new();
        assert!(world.capsule_overlaps(
            &standing(2.5, 0.0, 0.0),
            CollisionMask::WORLD,
            &mut buffers
        ));
        assert!(!world.capsule_overlaps(
            &standing(0.0, 5.0, 0.0),
            CollisionMask::WORLD,
            &mut buffers
        ));
    }

    #[test]
    fn clearing_empties_the_world_and_invalidates_every_handle() {
        let (mut world, floor, wall) = world_with_a_floor_and_a_wall();
        world.clear();
        assert!(world.is_empty());
        assert!(!world.contains(floor));
        assert!(!world.contains(wall));
        assert_eq!(world.statics().count(), 0);

        let mut buffers = QueryBuffers::new();
        let stats = world.overlap_capsule(
            &standing(0.0, -1.0, 0.0),
            CollisionMask::WORLD,
            &mut buffers,
        );
        assert_eq!(stats, QueryStats::new());
    }

    #[test]
    fn two_identically_built_worlds_report_contacts_in_the_same_order() {
        // Determinism at the world level: resolution walks the contact list, so a different
        // order is a different final position.
        let build = || {
            let (world, _, _) = world_with_a_floor_and_a_wall();
            let mut buffers = QueryBuffers::new();
            world.overlap_capsule(
                &standing(1.7, -0.05, 0.0),
                CollisionMask::WORLD,
                &mut buffers,
            );
            buffers
                .contacts()
                .iter()
                .map(|c| (c.id, c.contact.normal, c.contact.penetration))
                .collect::<Vec<_>>()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn an_unrelated_layer_is_answered_with_nothing() {
        let (world, _, _) = world_with_a_floor_and_a_wall();
        let mut buffers = QueryBuffers::new();
        world.overlap_capsule(
            &standing(1.7, -0.05, 0.0),
            CollisionMask::NONE.with(CollisionLayer::DUMMY),
            &mut buffers,
        );
        assert!(buffers.contacts().is_empty());
    }
}
