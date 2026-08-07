//! Gameplay entity identity.
//!
//! An [`EntityId`] is an index and a generation in eight bytes: `Copy`, comparable,
//! hashable, and safe to keep in an event long after the entity it names has died. It owns
//! nothing. A raw pointer or a `Rc<RefCell<Entity>>` would make every combat event a
//! lifetime problem and every despawn a chance to dereference freed memory.
//!
//! The generation is what makes a stale reference *detectable*. A slot freed by a despawn
//! is reused by the next spawn, and without a generation the new occupant would answer to
//! the dead one's id — a damage event queued this tick would land on whatever spawned in
//! its place.
//!
//! This is the same design `engine_render_data::Handle` uses, deliberately duplicated
//! rather than shared: a gameplay id and a render handle must not be interchangeable, and
//! `engine-game` cannot depend on `engine-render-data` anyway.

/// A gameplay entity reference.
///
/// Stable for the entity's lifetime, cheap to copy, and detectably stale afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId {
    index: u32,
    generation: u32,
}

impl EntityId {
    /// An id that names nothing.
    ///
    /// Generation 0 is never issued, so this can never match a live slot. It exists so a
    /// field like "who killed this" can be empty without becoming an `Option` at every use.
    pub const INVALID: Self = Self {
        index: u32::MAX,
        generation: 0,
    };

    /// Builds an id from a raw slot and generation.
    ///
    /// Public for tests and for a future replay that restores a world from a recording.
    /// Fabricating one in gameplay code names a slot that may not exist; ask the world.
    #[must_use]
    pub const fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    /// The slot this id refers to.
    #[must_use]
    pub const fn index(self) -> usize {
        self.index as usize
    }

    /// The generation this id was issued with.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }

    /// Whether this id could name a live entity at all.
    ///
    /// A false answer is certain; a true answer still has to be checked against the world,
    /// which is what [`EntityAllocator::contains`] does.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.generation != 0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::INVALID
    }
}

/// Hands out entity slots and detects stale ids.
///
/// Slots are reused, so a world that spawns and despawns for an hour uses as many slots as
/// its peak population rather than as many as its total spawn count. The parallel arrays in
/// [`crate::world::GameWorld`] are indexed by the slot this allocator returns.
#[derive(Debug, Default, Clone)]
pub struct EntityAllocator {
    /// Generation currently living in each slot. Odd or even means nothing; zero is never
    /// stored, because generation 0 is the invalid id.
    generations: Vec<u32>,
    /// Whether each slot currently holds a live entity.
    occupied: Vec<bool>,
    /// Free slots, most recently freed first.
    free: Vec<u32>,
    live: usize,
}

impl EntityAllocator {
    /// An allocator with no slots.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An allocator with room for `capacity` slots before it grows.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            generations: Vec::with_capacity(capacity),
            occupied: Vec::with_capacity(capacity),
            free: Vec::new(),
            live: 0,
        }
    }

    /// Takes a slot, reusing a freed one when there is one.
    pub fn allocate(&mut self) -> EntityId {
        self.live += 1;
        if let Some(index) = self.free.pop() {
            let slot = index as usize;
            self.occupied[slot] = true;
            return EntityId::new(index, self.generations[slot]);
        }

        let index = self.generations.len() as u32;
        // Generation 1, not 0: a default-constructed id has generation 0 and must never
        // address a live slot.
        self.generations.push(1);
        self.occupied.push(true);
        EntityId::new(index, 1)
    }

    /// Releases a slot. Returns `false` for an id that was already stale.
    pub fn free(&mut self, id: EntityId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let slot = id.index();
        // Bumping the generation is what makes every outstanding id to this slot stale.
        // `max(1)` keeps the wrap away from the reserved invalid generation.
        self.generations[slot] = self.generations[slot].wrapping_add(1).max(1);
        self.occupied[slot] = false;
        self.free.push(id.index);
        self.live -= 1;
        true
    }

    /// Whether the id names a live entity.
    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        let slot = id.index();
        slot < self.generations.len()
            && self.occupied[slot]
            && self.generations[slot] == id.generation()
    }

    /// The id living in `slot`, if the slot is occupied.
    ///
    /// The inverse of [`EntityId::index`], for a system that walks slots — every combat
    /// system does — and has to name what it found without the caller having kept an id.
    #[must_use]
    pub fn id_at(&self, slot: usize) -> Option<EntityId> {
        if slot < self.generations.len() && self.occupied[slot] {
            Some(EntityId::new(slot as u32, self.generations[slot]))
        } else {
            None
        }
    }

    /// Whether `slot` holds a live entity.
    #[must_use]
    pub fn is_occupied(&self, slot: usize) -> bool {
        self.occupied.get(slot).copied().unwrap_or(false)
    }

    /// Live entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether nothing is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Slots ever allocated, live or free. The length of the parallel arrays.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.generations.len()
    }

    /// Frees every slot, keeping the allocated capacity.
    ///
    /// Generations are bumped rather than cleared, so ids handed out before the clear stay
    /// stale instead of silently becoming valid again.
    pub fn clear(&mut self) {
        for (slot, generation) in self.generations.iter_mut().enumerate() {
            if self.occupied[slot] {
                *generation = generation.wrapping_add(1).max(1);
            }
        }
        self.occupied.fill(false);
        self.free.clear();
        self.free.extend((0..self.generations.len() as u32).rev());
        self.live = 0;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn an_allocated_id_is_live_and_names_its_slot() {
        let mut allocator = EntityAllocator::new();
        let id = allocator.allocate();

        assert!(id.is_valid());
        assert!(allocator.contains(id));
        assert_eq!(allocator.id_at(id.index()), Some(id));
        assert_eq!(allocator.len(), 1);
        assert_eq!(allocator.slot_count(), 1);
    }

    #[test]
    fn ids_are_unique_while_they_are_alive() {
        let mut allocator = EntityAllocator::new();
        let ids: Vec<_> = (0..64).map(|_| allocator.allocate()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "two live entities share an id");
        assert!(ids.iter().all(|id| allocator.contains(*id)));
    }

    #[test]
    fn the_invalid_id_never_names_anything() {
        let mut allocator = EntityAllocator::new();
        allocator.allocate();
        assert!(!EntityId::INVALID.is_valid());
        assert!(!allocator.contains(EntityId::INVALID));
        assert_eq!(EntityId::default(), EntityId::INVALID);
    }

    #[test]
    fn freeing_makes_the_id_stale_and_the_slot_reusable() {
        let mut allocator = EntityAllocator::new();
        let first = allocator.allocate();
        assert!(allocator.free(first));
        assert!(!allocator.contains(first));
        assert_eq!(allocator.id_at(first.index()), None);

        let second = allocator.allocate();
        assert_eq!(
            first.index(),
            second.index(),
            "the freed slot must be reused"
        );
        assert_ne!(first, second);
        assert!(allocator.contains(second));
        assert_eq!(allocator.slot_count(), 1, "no new slot was needed");
    }

    #[test]
    fn a_stale_id_cannot_reach_the_entity_that_replaced_it() {
        // The reason ids carry a generation at all: a damage event queued against a dead
        // entity must not land on whatever spawned into its slot.
        let mut allocator = EntityAllocator::new();
        let dead = allocator.allocate();
        allocator.free(dead);
        let replacement = allocator.allocate();

        assert!(!allocator.contains(dead));
        assert!(allocator.contains(replacement));
        assert_eq!(allocator.id_at(dead.index()), Some(replacement));
    }

    #[test]
    fn freeing_twice_is_reported_rather_than_corrupting_the_free_list() {
        let mut allocator = EntityAllocator::new();
        let id = allocator.allocate();
        assert!(allocator.free(id));
        assert!(!allocator.free(id));
        assert_eq!(allocator.len(), 0);

        // A double free that pushed the slot twice would hand the same slot to two live
        // entities on the next two allocations.
        let a = allocator.allocate();
        let b = allocator.allocate();
        assert_ne!(a.index(), b.index());
    }

    #[test]
    fn clearing_kills_everything_and_leaves_no_id_valid() {
        let mut allocator = EntityAllocator::with_capacity(8);
        let ids: Vec<_> = (0..4).map(|_| allocator.allocate()).collect();
        allocator.clear();

        assert!(allocator.is_empty());
        for id in &ids {
            assert!(!allocator.contains(*id), "{id:?} survived a clear");
        }
        // Slots are reused rather than regrown.
        for _ in 0..4 {
            allocator.allocate();
        }
        assert_eq!(allocator.slot_count(), 4);
    }
}
