//! The gameplay world: every entity's state, in parallel arrays.
//!
//! ```text
//! slot   0        1        2        3
//! kind   Player   Dummy    Dummy    Dummy
//! body   [pos, velocity, grounded, facing, state]
//! health [current, max]
//! status [Burning 3.2 s]  []       []
//! ```
//!
//! Structure of arrays, indexed by the slot an [`EntityId`] names. The combat systems walk
//! one attribute at a time — targeting reads positions and factions, the status system
//! reads statuses and writes health — so streaming the attributes they do not look at would
//! waste the cache line they are reading through.
//!
//! ## Why not an ECS
//!
//! Every entity here has the same components. An archetype system would spend its storage
//! machinery, its query language and its scheduler on a world with two entity kinds and one
//! component set. The measurement that would change this is entity counts in the thousands
//! with divergent components; Milestone 24 benchmarks to 1,000 dummies so that decision has
//! evidence rather than taste behind it.
//!
//! ## What is *not* here
//!
//! No mesh, no material, no colour, no animation. Those are presentation, they live in a
//! `RenderWorld`, and this crate cannot even name their types.

use engine_core::math::Vec3;

use crate::combat::{DamageResult, Faction, Health};
use crate::entity::{EntityAllocator, EntityId};
use crate::movement::Body;
use crate::status::StatusSet;

/// What an entity is, for the presentation layer and for the arena's own bookkeeping.
///
/// Gameplay behaviour keys off [`Faction`] and off having a body, not off this: an enum
/// that systems branch on becomes a list every new entity kind has to be added to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityKind {
    /// The character the local player controls.
    Player,
    /// A training dummy: hostile, and deliberately inert.
    Dummy,
    /// Something that fights back.
    Enemy,
}

impl EntityKind {
    /// Short uppercase name, for the developer overlay's font.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Player => "PLAYER",
            Self::Dummy => "DUMMY",
            Self::Enemy => "ENEMY",
        }
    }
}

/// What a caller must supply to spawn an entity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityDesc {
    /// What it is.
    pub kind: EntityKind,
    /// Whose side it is on.
    pub faction: Faction,
    /// Where its feet start.
    pub position: Vec3,
    /// Its full health.
    pub max_health: f32,
    /// Horizontal radius, in metres. Used by area targeting so a wide target is hit when
    /// its edge is inside the radius rather than only when its centre is.
    pub radius: f32,
    /// Height, in metres. Gameplay uses it for nothing yet; extraction uses it to place a
    /// mesh whose origin is its centre on top of feet that are not.
    pub height: f32,
}

impl Default for EntityDesc {
    fn default() -> Self {
        Self {
            kind: EntityKind::Dummy,
            faction: Faction::Hostile,
            position: Vec3::ZERO,
            max_health: 100.0,
            radius: 0.5,
            height: 2.0,
        }
    }
}

impl EntityDesc {
    /// A player at `position`.
    #[must_use]
    pub fn player(position: Vec3) -> Self {
        Self {
            kind: EntityKind::Player,
            faction: Faction::Player,
            position,
            max_health: 500.0,
            radius: 0.5,
            height: 1.8,
        }
    }

    /// A training dummy at `position`.
    #[must_use]
    pub fn dummy(position: Vec3, max_health: f32) -> Self {
        Self {
            kind: EntityKind::Dummy,
            faction: Faction::Hostile,
            position,
            max_health,
            radius: 0.7,
            height: 2.2,
        }
    }
}

/// An enemy of `kind` at `position`, for [`GameWorld::spawn`].
///
/// Its numbers come from the archetype table, so "how much health does a Brute have" is
/// answered in exactly one place.
#[must_use]
pub fn enemy_desc(kind: crate::enemy::EnemyKind, position: Vec3) -> EntityDesc {
    let archetype = kind.archetype();
    EntityDesc {
        kind: EntityKind::Enemy,
        faction: Faction::Hostile,
        position,
        max_health: archetype.max_health,
        radius: archetype.radius,
        height: archetype.height,
    }
}

/// Every entity's gameplay state.
#[derive(Debug, Default, Clone)]
pub struct GameWorld {
    allocator: EntityAllocator,
    kinds: Vec<EntityKind>,
    factions: Vec<Faction>,
    bodies: Vec<Body>,
    /// Every body as it was at the start of the current tick.
    ///
    /// Not a gameplay input — nothing reads it to decide anything. It exists so presentation
    /// can draw *between* two simulation states instead of snapping from one to the next: at
    /// 60 Hz against 1,400 FPS, twenty-three frames in twenty-four would otherwise show the
    /// same pose. Recording where something was is a simulation fact; blending the two is
    /// presentation's job and happens there.
    previous_bodies: Vec<Body>,
    healths: Vec<Health>,
    statuses: Vec<StatusSet>,
    radii: Vec<f32>,
    heights: Vec<f32>,
    /// Where each entity was spawned, so a dummy reset can put it back.
    spawn_positions: Vec<Vec3>,
}

impl GameWorld {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty world with room for `capacity` entities.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            allocator: EntityAllocator::with_capacity(capacity),
            kinds: Vec::with_capacity(capacity),
            factions: Vec::with_capacity(capacity),
            bodies: Vec::with_capacity(capacity),
            previous_bodies: Vec::with_capacity(capacity),
            healths: Vec::with_capacity(capacity),
            statuses: Vec::with_capacity(capacity),
            radii: Vec::with_capacity(capacity),
            heights: Vec::with_capacity(capacity),
            spawn_positions: Vec::with_capacity(capacity),
        }
    }

    /// Adds an entity, reusing a freed slot when one is available.
    pub fn spawn(&mut self, desc: EntityDesc) -> EntityId {
        let id = self.allocator.allocate();
        let slot = id.index();
        let body = Body::at(desc.position);
        let health = Health::new(desc.max_health);

        if slot < self.kinds.len() {
            self.kinds[slot] = desc.kind;
            self.factions[slot] = desc.faction;
            self.bodies[slot] = body;
            // A new entity has no history: interpolating from wherever the slot's previous
            // occupant stood would slide it in from across the arena on its first frame.
            self.previous_bodies[slot] = body;
            self.healths[slot] = health;
            // Cleared rather than replaced: the `Vec` inside keeps its capacity, so a slot
            // that has held a burning dummy before does not allocate again.
            self.statuses[slot].clear();
            self.radii[slot] = desc.radius;
            self.heights[slot] = desc.height;
            self.spawn_positions[slot] = desc.position;
            return id;
        }

        self.kinds.push(desc.kind);
        self.factions.push(desc.faction);
        self.bodies.push(body);
        self.previous_bodies.push(body);
        self.healths.push(health);
        self.statuses.push(StatusSet::new());
        self.radii.push(desc.radius);
        self.heights.push(desc.height);
        self.spawn_positions.push(desc.position);
        id
    }

    /// Removes an entity. A stale id is reported rather than panicking.
    pub fn despawn(&mut self, id: EntityId) -> bool {
        if !self.allocator.contains(id) {
            return false;
        }
        self.statuses[id.index()].clear();
        self.allocator.free(id)
    }

    /// Whether the id names a live entity.
    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.allocator.contains(id)
    }

    /// Live entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.allocator.len()
    }

    /// Whether the world holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allocator.is_empty()
    }

    /// Slots ever allocated, live or free. The length of the parallel arrays.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.allocator.slot_count()
    }

    /// Whether `slot` holds a live entity.
    #[must_use]
    pub fn is_occupied(&self, slot: usize) -> bool {
        self.allocator.is_occupied(slot)
    }

    /// The id living in `slot`, if any.
    #[must_use]
    pub fn id_at(&self, slot: usize) -> Option<EntityId> {
        self.allocator.id_at(slot)
    }

    /// Every live entity, in slot order.
    ///
    /// Slot order rather than spawn order, and that is what makes it deterministic: two
    /// runs that spawn the same entities in the same order iterate identically.
    pub fn entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        (0..self.slot_count()).filter_map(|slot| self.id_at(slot))
    }

    /// What an entity is.
    #[must_use]
    pub fn kind(&self, id: EntityId) -> Option<EntityKind> {
        self.contains(id).then(|| self.kinds[id.index()])
    }

    /// Whose side an entity is on.
    #[must_use]
    pub fn faction(&self, id: EntityId) -> Option<Faction> {
        self.contains(id).then(|| self.factions[id.index()])
    }

    /// An entity's body.
    #[must_use]
    pub fn body(&self, id: EntityId) -> Option<&Body> {
        self.contains(id).then(|| &self.bodies[id.index()])
    }

    /// An entity's body as it was at the start of the current tick.
    ///
    /// Paired with [`GameWorld::body`] and an interpolation factor, this is what lets
    /// presentation draw a smooth 1,400 FPS from a 60 Hz simulation.
    #[must_use]
    pub fn previous_body(&self, id: EntityId) -> Option<&Body> {
        self.contains(id).then(|| &self.previous_bodies[id.index()])
    }

    /// Copies every body into the previous-body array.
    ///
    /// Called once at the start of each fixed tick, before anything moves. One memcpy over
    /// the array — at 1,000 entities it is 32 KB a tick, which is 0.05% of the tick's own
    /// budget and was measured rather than assumed.
    pub fn begin_tick(&mut self) {
        self.previous_bodies.clear();
        self.previous_bodies.extend_from_slice(&self.bodies);
    }

    /// An entity's body, for a system that moves it.
    pub fn body_mut(&mut self, id: EntityId) -> Option<&mut Body> {
        if !self.contains(id) {
            return None;
        }
        Some(&mut self.bodies[id.index()])
    }

    /// Where an entity's feet are.
    #[must_use]
    pub fn position(&self, id: EntityId) -> Option<Vec3> {
        self.body(id).map(|body| body.position)
    }

    /// An entity's health.
    #[must_use]
    pub fn health(&self, id: EntityId) -> Option<&Health> {
        self.contains(id).then(|| &self.healths[id.index()])
    }

    /// Whether an entity exists and has health left.
    #[must_use]
    pub fn is_alive(&self, id: EntityId) -> bool {
        self.health(id).is_some_and(Health::is_alive)
    }

    /// Applies damage and reports what landed.
    ///
    /// The only way health goes down. Emitting the [`crate::event::CombatEvent`] is the
    /// caller's job, because only the caller knows which ability to credit.
    pub fn damage(&mut self, id: EntityId, amount: f32) -> DamageResult {
        if !self.contains(id) {
            return DamageResult::NONE;
        }
        self.healths[id.index()].damage(amount)
    }

    /// Restores health and reports what landed.
    pub fn heal(&mut self, id: EntityId, amount: f32) -> DamageResult {
        if !self.contains(id) {
            return DamageResult::NONE;
        }
        self.healths[id.index()].heal(amount)
    }

    /// An entity's statuses.
    #[must_use]
    pub fn statuses(&self, id: EntityId) -> Option<&StatusSet> {
        self.contains(id).then(|| &self.statuses[id.index()])
    }

    /// An entity's statuses, for a system that applies or consumes one.
    pub fn statuses_mut(&mut self, id: EntityId) -> Option<&mut StatusSet> {
        if !self.contains(id) {
            return None;
        }
        Some(&mut self.statuses[id.index()])
    }

    /// An entity's horizontal radius.
    #[must_use]
    pub fn radius(&self, id: EntityId) -> f32 {
        if self.contains(id) {
            self.radii[id.index()]
        } else {
            0.0
        }
    }

    /// An entity's height.
    #[must_use]
    pub fn height(&self, id: EntityId) -> f32 {
        if self.contains(id) {
            self.heights[id.index()]
        } else {
            0.0
        }
    }

    /// Where an entity was spawned.
    #[must_use]
    pub fn spawn_position(&self, id: EntityId) -> Option<Vec3> {
        self.contains(id).then(|| self.spawn_positions[id.index()])
    }

    /// Horizontal distance between two entities' centres, edge-corrected.
    ///
    /// Subtracting the radii means a wide target is in range when its *edge* is, which is
    /// what stops a large dummy from being unhittable from a metre away. Never negative.
    #[must_use]
    pub fn distance_between(&self, a: EntityId, b: EntityId) -> Option<f32> {
        let first = self.position(a)?;
        let second = self.position(b)?;
        let gap = horizontal_distance(first, second) - self.radius(a) - self.radius(b);
        Some(gap.max(0.0))
    }

    /// Puts an entity back where it spawned, at full health, with no statuses.
    ///
    /// Returns `false` for a stale id. Velocity is cleared too: a dummy reset while being
    /// knocked around must not keep drifting.
    pub fn restore(&mut self, id: EntityId) -> bool {
        if !self.contains(id) {
            return false;
        }
        let slot = id.index();
        let spawn = self.spawn_positions[slot];
        self.bodies[slot] = Body::at(spawn);
        // Restoring is a teleport, not a movement: without this the dummy would visibly
        // slide back to its spawn over the following frame.
        self.previous_bodies[slot] = self.bodies[slot];
        self.healths[slot].restore();
        self.statuses[slot].clear();
        true
    }

    /// Forgets where an entity was last tick, so it is drawn where it is now.
    ///
    /// For a teleport. Without it the presentation layer interpolates between the old
    /// position and the new one, and a character sent across the level slides there over a
    /// frame instead of arriving.
    pub fn reset_history(&mut self, id: EntityId) {
        if !self.contains(id) {
            return;
        }
        let slot = id.index();
        self.previous_bodies[slot] = self.bodies[slot];
    }

    /// Removes every entity, keeping the allocated capacity.
    pub fn clear(&mut self) {
        for statuses in &mut self.statuses {
            statuses.clear();
        }
        self.allocator.clear();
    }
}

/// Distance between two points, ignoring height.
///
/// Horizontal because the arena is flat and a jumping player must not fall out of range of
/// the thing they are attacking.
#[must_use]
pub fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    (a.x - b.x).hypot(a.z - b.z)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::status::{Status, StatusKind};

    fn world_with_two() -> (GameWorld, EntityId, EntityId) {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let dummy = world.spawn(EntityDesc::dummy(Vec3::new(0.0, 0.0, -10.0), 200.0));
        (world, player, dummy)
    }

    #[test]
    fn a_spawned_entity_carries_everything_it_was_given() {
        let (world, player, dummy) = world_with_two();

        assert_eq!(world.kind(player), Some(EntityKind::Player));
        assert_eq!(world.faction(player), Some(Faction::Player));
        assert_eq!(world.position(player), Some(Vec3::ZERO));
        assert!(world.is_alive(player));

        assert_eq!(world.kind(dummy), Some(EntityKind::Dummy));
        assert_eq!(world.faction(dummy), Some(Faction::Hostile));
        assert_eq!(
            world.health(dummy).map(Health::max),
            Some(200.0),
            "the dummy kept its own maximum"
        );
        assert_eq!(world.len(), 2);
    }

    #[test]
    fn a_despawned_entity_is_gone_and_every_accessor_says_so() {
        let (mut world, player, dummy) = world_with_two();
        assert!(world.despawn(dummy));

        assert!(!world.contains(dummy));
        assert_eq!(world.kind(dummy), None);
        assert_eq!(world.body(dummy), None);
        assert_eq!(world.health(dummy), None);
        assert_eq!(world.statuses(dummy), None);
        assert_eq!(world.position(dummy), None);
        assert!(!world.is_alive(dummy));
        assert_eq!(world.radius(dummy), 0.0);
        assert_eq!(world.len(), 1);
        assert!(world.contains(player), "despawning one kept the other");
    }

    #[test]
    fn a_stale_id_cannot_damage_the_entity_that_replaced_it() {
        // The reason every accessor checks the generation: a damage event queued against a
        // dead dummy must not hit whatever spawned into its slot.
        let (mut world, _, dummy) = world_with_two();
        world.despawn(dummy);
        let replacement = world.spawn(EntityDesc::dummy(Vec3::ZERO, 100.0));
        assert_eq!(dummy.index(), replacement.index(), "the slot was reused");

        assert_eq!(world.damage(dummy, 50.0), DamageResult::NONE);
        assert_eq!(
            world.health(replacement).map(Health::current),
            Some(100.0),
            "the replacement was untouched"
        );
    }

    #[test]
    fn damage_and_healing_go_through_the_world() {
        let (mut world, _, dummy) = world_with_two();
        let result = world.damage(dummy, 80.0);
        assert_eq!(result.applied, 80.0);
        assert_eq!(world.health(dummy).map(Health::current), Some(120.0));

        let healed = world.heal(dummy, 30.0);
        assert_eq!(healed.applied, 30.0);
        assert_eq!(world.health(dummy).map(Health::current), Some(150.0));
    }

    #[test]
    fn a_killed_entity_stays_in_the_world_as_a_corpse() {
        // Death is not a despawn: the meter, the overlay and a reset all need the entity to
        // still be there, and a body that vanishes on death cannot be reset by pressing R.
        let (mut world, _, dummy) = world_with_two();
        let result = world.damage(dummy, 500.0);

        assert!(result.killed);
        assert!(world.contains(dummy), "a corpse is still an entity");
        assert!(!world.is_alive(dummy));
        assert_eq!(world.len(), 2);
    }

    #[test]
    fn restoring_puts_a_dummy_back_the_way_it_started() {
        let (mut world, player, dummy) = world_with_two();
        world.damage(dummy, 1000.0);
        if let Some(statuses) = world.statuses_mut(dummy) {
            statuses.apply(Status::new(
                StatusKind::Burning,
                player,
                crate::ability::AbilityId::NONE,
                6.0,
                8.0,
            ));
        }
        if let Some(body) = world.body_mut(dummy) {
            body.position = Vec3::new(20.0, 0.0, 20.0);
            body.velocity = Vec3::new(5.0, 0.0, 0.0);
        }

        assert!(world.restore(dummy));
        assert!(world.is_alive(dummy));
        assert_eq!(world.health(dummy).map(Health::current), Some(200.0));
        assert_eq!(world.position(dummy), Some(Vec3::new(0.0, 0.0, -10.0)));
        assert_eq!(
            world.body(dummy).map(|body| body.velocity),
            Some(Vec3::ZERO)
        );
        assert_eq!(world.statuses(dummy).map(StatusSet::len), Some(0));
    }

    #[test]
    fn distance_is_horizontal_and_edge_corrected() {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::new(0.0, 12.0, 0.0)));
        let dummy = world.spawn(EntityDesc::dummy(Vec3::new(10.0, 0.0, 0.0), 100.0));

        // Ten metres apart, less half a metre of player and 0.7 of dummy. Height is ignored
        // so jumping cannot put a target out of range.
        let distance = world
            .distance_between(player, dummy)
            .expect("both entities exist");
        assert!((distance - 8.8).abs() < 1e-4, "{distance}");

        // Overlapping entities are at distance zero, never negative.
        if let Some(body) = world.body_mut(dummy) {
            body.position = Vec3::ZERO;
        }
        assert_eq!(world.distance_between(player, dummy), Some(0.0));
    }

    #[test]
    fn iteration_is_in_slot_order_and_skips_the_dead_slots() {
        let mut world = GameWorld::new();
        let first = world.spawn(EntityDesc::dummy(Vec3::ZERO, 10.0));
        let second = world.spawn(EntityDesc::dummy(Vec3::ZERO, 10.0));
        let third = world.spawn(EntityDesc::dummy(Vec3::ZERO, 10.0));
        world.despawn(second);

        let ids: Vec<EntityId> = world.entities().collect();
        assert_eq!(ids, vec![first, third]);
    }

    #[test]
    fn a_reused_slot_starts_with_no_statuses() {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let first = world.spawn(EntityDesc::dummy(Vec3::ZERO, 10.0));
        if let Some(statuses) = world.statuses_mut(first) {
            statuses.apply(Status::new(
                StatusKind::Burning,
                player,
                crate::ability::AbilityId::NONE,
                6.0,
                8.0,
            ));
        }
        world.despawn(first);

        let second = world.spawn(EntityDesc::dummy(Vec3::ZERO, 10.0));
        assert_eq!(second.index(), first.index(), "the slot was reused");
        assert_eq!(
            world.statuses(second).map(StatusSet::len),
            Some(0),
            "a new entity inherited the previous occupant's burn"
        );
    }

    #[test]
    fn clearing_empties_the_world_and_invalidates_every_id() {
        let (mut world, player, dummy) = world_with_two();
        world.clear();

        assert!(world.is_empty());
        assert!(!world.contains(player));
        assert!(!world.contains(dummy));
        assert_eq!(world.entities().count(), 0);
    }
}
