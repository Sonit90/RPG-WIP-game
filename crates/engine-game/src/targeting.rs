//! Who an ability affects.
//!
//! Two mechanisms, which is the smallest set that demonstrates the system:
//!
//! ```text
//! SingleTarget        -> the selected target, if it is alive, hostile and in range
//! AreaAroundTarget    -> everything hostile within a radius of the selected target
//! SelfCentered        -> everything hostile within a radius of the caster
//! ```
//!
//! Selection itself is a command ([`crate::command::PlayerCommand::CycleTarget`]) rather
//! than a click, because the arena has no cursor picking and the interesting question is
//! whether an area effect hits the group and misses what is outside it.
//!
//! Every query gathers into a buffer the caller owns and reuses, so resolving an ability's
//! targets allocates nothing after the first few frames.

use crate::entity::EntityId;
use crate::world::{GameWorld, horizontal_distance};

/// Reusable scratch for target queries.
///
/// One per simulation. Held rather than created per cast: an ability that allocates a
/// `Vec` every time it is used allocates several times a second in combat.
#[derive(Debug, Default, Clone)]
pub struct TargetQuery {
    scratch: Vec<EntityId>,
    /// Distance and entity together, for the sort that cycling needs.
    ///
    /// One array of pairs rather than two parallel ones, so the sort is
    /// `sort_unstable_by` — `O(n log n)` with no allocation — instead of a hand-written
    /// insertion sort keeping two arrays in step. At 1,000 hostiles the insertion sort was
    /// measured at a 398 us p99 against a 17 us median tick; see `docs/phase3.md`.
    ranked: Vec<(f32, EntityId)>,
}

impl TargetQuery {
    /// An empty query buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty query buffer with room for `capacity` targets.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            scratch: Vec::with_capacity(capacity),
            ranked: Vec::with_capacity(capacity),
        }
    }

    /// How many targets fit before the buffer grows.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.scratch.capacity()
    }

    /// The living hostiles, nearest first.
    ///
    /// Ties are broken by entity id, so the order is total and two runs of the same
    /// scenario cycle targets identically.
    pub fn hostiles_by_distance(&mut self, world: &GameWorld, from: EntityId) -> &[EntityId] {
        self.scratch.clear();
        self.ranked.clear();

        let Some(faction) = world.faction(from) else {
            return &self.scratch;
        };

        for candidate in world.entities() {
            if candidate == from || !world.is_alive(candidate) {
                continue;
            }
            if !world
                .faction(candidate)
                .is_some_and(|other| faction.is_hostile_to(other))
            {
                continue;
            }
            let distance = world.distance_between(from, candidate).unwrap_or(f32::MAX);
            self.ranked.push((distance, candidate));
        }

        // The entity is the tie-break, which is what makes the order total: two dummies at
        // exactly the same distance must not swap places between runs. `sort_unstable_by`
        // rather than `sort_by`, because an unstable sort needs no scratch allocation and
        // the comparator is already total.
        self.ranked
            .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        self.scratch
            .extend(self.ranked.iter().map(|(_, entity)| *entity));
        &self.scratch
    }

    /// The nearest living hostile, if there is one.
    ///
    /// A single scan rather than a sort: this runs every tick to keep a valid target
    /// selected, and sorting a thousand hostiles to read one element off the front was
    /// measured as the dominant cost of a tick at that scale.
    pub fn nearest_hostile(&mut self, world: &GameWorld, from: EntityId) -> Option<EntityId> {
        let faction = world.faction(from)?;
        let mut best: Option<(f32, EntityId)> = None;

        for candidate in world.entities() {
            if candidate == from || !world.is_alive(candidate) {
                continue;
            }
            if !world
                .faction(candidate)
                .is_some_and(|other| faction.is_hostile_to(other))
            {
                continue;
            }
            let distance = world.distance_between(from, candidate).unwrap_or(f32::MAX);
            let better = match best {
                Some((best_distance, best_entity)) => {
                    is_further(best_distance, best_entity, distance, candidate)
                }
                None => true,
            };
            if better {
                best = Some((distance, candidate));
            }
        }
        best.map(|(_, entity)| entity)
    }

    /// The next living hostile after `current`, by distance, wrapping around.
    ///
    /// A dead or stale current target starts the cycle from the nearest, which is what a
    /// player pressing the key after their target died expects.
    pub fn next_hostile(
        &mut self,
        world: &GameWorld,
        from: EntityId,
        current: Option<EntityId>,
    ) -> Option<EntityId> {
        let ordered = self.hostiles_by_distance(world, from);
        let index = current.and_then(|target| ordered.iter().position(|id| *id == target));
        match index {
            Some(index) => ordered.get((index + 1) % ordered.len()).copied(),
            None => ordered.first().copied(),
        }
    }

    /// Gathers every living hostile whose edge is within `radius` of `centre`.
    pub fn gather_around(
        &mut self,
        world: &GameWorld,
        caster: EntityId,
        centre: engine_core::math::Vec3,
        radius: f32,
    ) -> &[EntityId] {
        self.scratch.clear();
        let Some(faction) = world.faction(caster) else {
            return &self.scratch;
        };

        for candidate in world.entities() {
            if candidate == caster || !world.is_alive(candidate) {
                continue;
            }
            if !world
                .faction(candidate)
                .is_some_and(|other| faction.is_hostile_to(other))
            {
                continue;
            }
            let Some(position) = world.position(candidate) else {
                continue;
            };
            // Edge-corrected, like every other range test here: a wide dummy at the rim of
            // the blast is inside it.
            if horizontal_distance(centre, position) - world.radius(candidate) <= radius {
                self.scratch.push(candidate);
            }
        }
        &self.scratch
    }
}

/// Whether `(distance_a, entity_a)` sorts after `(distance_b, entity_b)`.
///
/// The id is the tie-break, which is what makes the order total: two dummies at exactly the
/// same distance must not swap places between runs.
fn is_further(distance_a: f32, entity_a: EntityId, distance_b: f32, entity_b: EntityId) -> bool {
    match distance_a.partial_cmp(&distance_b) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Equal) | None => entity_a > entity_b,
        Some(std::cmp::Ordering::Less) => false,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::combat::Faction;
    use crate::world::EntityDesc;
    use engine_core::math::Vec3;

    /// A player and three dummies in a line, two metres apart.
    fn world() -> (GameWorld, EntityId, Vec<EntityId>) {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let dummies = (1..=3)
            .map(|step| {
                world.spawn(EntityDesc::dummy(
                    Vec3::new(0.0, 0.0, -2.0 * step as f32),
                    100.0,
                ))
            })
            .collect();
        (world, player, dummies)
    }

    #[test]
    fn the_nearest_hostile_is_the_closest_living_one() {
        let (mut world, player, dummies) = world();
        let mut query = TargetQuery::with_capacity(8);
        assert_eq!(query.nearest_hostile(&world, player), Some(dummies[0]));

        world.damage(dummies[0], 1_000.0);
        assert_eq!(
            query.nearest_hostile(&world, player),
            Some(dummies[1]),
            "a corpse was selected"
        );
    }

    #[test]
    fn cycling_walks_outwards_and_comes_back_round() {
        let (world, player, dummies) = world();
        let mut query = TargetQuery::with_capacity(8);
        let mut current = None;
        for expected in &dummies {
            current = query.next_hostile(&world, player, current);
            assert_eq!(current, Some(*expected));
        }
        assert_eq!(
            query.next_hostile(&world, player, current),
            Some(dummies[0])
        );
    }

    #[test]
    fn gathering_is_edge_corrected_and_skips_the_caster() {
        // A dummy whose centre is just outside the circle but whose body is inside it is
        // hit; the caster never hits itself.
        let (world, player, dummies) = world();
        let mut query = TargetQuery::with_capacity(8);
        let hit = query.gather_around(&world, player, Vec3::ZERO, 2.5);
        assert!(hit.contains(&dummies[0]));
        assert!(!hit.contains(&player), "the caster hit itself");
        assert!(!hit.contains(&dummies[2]), "something far away was hit");
    }

    #[test]
    fn nothing_hostile_gathers_nothing_rather_than_failing() {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let mut query = TargetQuery::with_capacity(8);
        assert!(
            query
                .gather_around(&world, player, Vec3::ZERO, 50.0)
                .is_empty()
        );
        assert_eq!(query.nearest_hostile(&world, player), None);
        assert!(!Faction::Player.is_hostile_to(Faction::Player));
    }
}
