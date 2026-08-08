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

use crate::ability::{AbilityDef, AbilityError, Targeting};
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

    /// Resolves everything an ability affects, or says why it cannot be used.
    ///
    /// Validation order matters and is deliberate: "no target" is reported before "out of
    /// range", so a player with nothing selected is told the useful thing.
    ///
    /// An area ability that finds nobody resolves to an empty slice rather than an error.
    /// It was still cast — the cooldown and the cost are spent — because that is what
    /// missing with an area effect means.
    pub fn resolve(
        &mut self,
        world: &GameWorld,
        caster: EntityId,
        target: Option<EntityId>,
        ability: &AbilityDef,
    ) -> Result<&[EntityId], AbilityError> {
        if !world.is_alive(caster) {
            return Err(AbilityError::CasterDead);
        }

        if ability.targeting.needs_target() {
            let target = target.ok_or(AbilityError::NoTarget)?;
            if !world.is_alive(target) {
                return Err(AbilityError::TargetInvalid);
            }
            if !world
                .faction(caster)
                .zip(world.faction(target))
                .is_some_and(|(mine, theirs)| mine.is_hostile_to(theirs))
            {
                return Err(AbilityError::TargetInvalid);
            }
            let distance = world
                .distance_between(caster, target)
                .ok_or(AbilityError::TargetInvalid)?;
            if distance > ability.range {
                return Err(AbilityError::OutOfRange);
            }
        }

        match ability.targeting {
            Targeting::SingleTarget => {
                self.scratch.clear();
                // Checked above; the `else` is unreachable rather than defensive.
                if let Some(target) = target {
                    self.scratch.push(target);
                }
                Ok(&self.scratch)
            }
            Targeting::AreaAroundTarget { radius } => {
                let centre = target.and_then(|id| world.position(id));
                match centre {
                    Some(centre) => Ok(self.gather_around(world, caster, centre, radius)),
                    None => Err(AbilityError::TargetInvalid),
                }
            }
            Targeting::SelfCentered { radius } => {
                let centre = world.position(caster).ok_or(AbilityError::CasterDead)?;
                Ok(self.gather_around(world, caster, centre, radius))
            }
        }
    }

    /// Gathers every living hostile whose edge is within `radius` of `centre`.
    fn gather_around(
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
    use crate::ability::{AbilityId, default_loadout};
    use crate::combat::{DamageType, Faction};
    use crate::world::{EntityDesc, EntityKind};
    use engine_core::math::Vec3;

    /// A player at the origin and dummies at the given positions.
    fn arena(positions: &[Vec3]) -> (GameWorld, EntityId, Vec<EntityId>) {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let dummies = positions
            .iter()
            .map(|position| world.spawn(EntityDesc::dummy(*position, 100.0)))
            .collect();
        (world, player, dummies)
    }

    fn fireball() -> AbilityDef {
        default_loadout()[0]
    }

    fn detonate() -> AbilityDef {
        default_loadout()[2]
    }

    fn ally(position: Vec3) -> EntityDesc {
        EntityDesc {
            kind: EntityKind::Dummy,
            faction: Faction::Player,
            position,
            ..EntityDesc::default()
        }
    }

    #[test]
    fn hostiles_come_back_nearest_first() {
        let (world, player, dummies) = arena(&[
            Vec3::new(0.0, 0.0, -30.0),
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, -15.0),
        ]);
        let mut query = TargetQuery::new();
        assert_eq!(
            query.hostiles_by_distance(&world, player),
            [dummies[1], dummies[2], dummies[0]]
        );
        assert_eq!(query.nearest_hostile(&world, player), Some(dummies[1]));
    }

    #[test]
    fn only_hostile_living_entities_are_targets() {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let friend = world.spawn(ally(Vec3::new(0.0, 0.0, -2.0)));
        let corpse = world.spawn(EntityDesc::dummy(Vec3::new(0.0, 0.0, -3.0), 10.0));
        let alive = world.spawn(EntityDesc::dummy(Vec3::new(0.0, 0.0, -20.0), 10.0));
        world.damage(corpse, 100.0);

        let mut query = TargetQuery::new();
        let hostiles = query.hostiles_by_distance(&world, player).to_vec();
        assert_eq!(hostiles, vec![alive]);
        assert!(!hostiles.contains(&friend), "an ally is not a target");
        assert!(!hostiles.contains(&corpse), "a corpse is not a target");
    }

    #[test]
    fn cycling_walks_the_list_and_wraps() {
        let (world, player, dummies) = arena(&[
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(0.0, 0.0, -15.0),
        ]);
        let mut query = TargetQuery::new();

        let first = query.next_hostile(&world, player, None);
        assert_eq!(first, Some(dummies[0]));
        let second = query.next_hostile(&world, player, first);
        assert_eq!(second, Some(dummies[1]));
        let third = query.next_hostile(&world, player, second);
        assert_eq!(third, Some(dummies[2]));
        assert_eq!(
            query.next_hostile(&world, player, third),
            Some(dummies[0]),
            "the cycle wraps"
        );
    }

    #[test]
    fn cycling_from_a_dead_target_starts_over_rather_than_failing() {
        let (mut world, player, dummies) =
            arena(&[Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, -10.0)]);
        world.damage(dummies[0], 1000.0);

        let mut query = TargetQuery::new();
        assert_eq!(
            query.next_hostile(&world, player, Some(dummies[0])),
            Some(dummies[1])
        );
    }

    #[test]
    fn a_single_target_ability_resolves_to_exactly_that_target() {
        let (world, player, dummies) = arena(&[Vec3::new(0.0, 0.0, -10.0)]);
        let mut query = TargetQuery::new();
        let resolved = query
            .resolve(&world, player, Some(dummies[0]), &fireball())
            .expect("a living hostile in range is a valid target");
        assert_eq!(resolved, [dummies[0]]);
    }

    #[test]
    fn every_way_a_single_target_cast_can_be_refused_is_named() {
        let (mut world, player, dummies) =
            arena(&[Vec3::new(0.0, 0.0, -10.0), Vec3::new(0.0, 0.0, -200.0)]);
        let friend = world.spawn(ally(Vec3::new(0.0, 0.0, -3.0)));
        let mut query = TargetQuery::new();
        let fireball = fireball();

        assert_eq!(
            query.resolve(&world, player, None, &fireball),
            Err(AbilityError::NoTarget)
        );
        assert_eq!(
            query.resolve(&world, player, Some(dummies[1]), &fireball),
            Err(AbilityError::OutOfRange),
            "200 metres against a 30 metre range"
        );
        assert_eq!(
            query.resolve(&world, player, Some(friend), &fireball),
            Err(AbilityError::TargetInvalid),
            "an ally is not a valid hostile target"
        );

        world.damage(dummies[0], 1000.0);
        assert_eq!(
            query.resolve(&world, player, Some(dummies[0]), &fireball),
            Err(AbilityError::TargetInvalid),
            "a corpse is not a valid target"
        );

        world.damage(player, 10_000.0);
        assert_eq!(
            query.resolve(&world, player, Some(dummies[1]), &fireball),
            Err(AbilityError::CasterDead)
        );
    }

    #[test]
    fn an_area_ability_hits_the_group_and_misses_what_is_outside_it() {
        // The Milestone 15 property, asserted directly: four dummies in a cluster, one far
        // enough away that the blast must not reach it.
        let (world, player, dummies) = arena(&[
            Vec3::new(0.0, 0.0, -20.0),
            Vec3::new(3.0, 0.0, -21.0),
            Vec3::new(-3.0, 0.0, -22.0),
            Vec3::new(1.0, 0.0, -24.0),
            Vec3::new(30.0, 0.0, -20.0),
        ]);
        let mut query = TargetQuery::new();
        let hit = query
            .resolve(&world, player, Some(dummies[0]), &detonate())
            .expect("the cluster is in range")
            .to_vec();

        assert_eq!(hit.len(), 4, "the whole cluster: {hit:?}");
        for dummy in &dummies[..4] {
            assert!(hit.contains(dummy), "{dummy:?} was in the radius");
        }
        assert!(
            !hit.contains(&dummies[4]),
            "a dummy 30 metres away took the blast"
        );
    }

    #[test]
    fn an_area_ability_that_finds_nobody_is_a_miss_rather_than_an_error() {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let mut query = TargetQuery::new();
        let nova = AbilityDef {
            cast_time: 0.0,
            id: AbilityId::from_slot(8).expect("slot 8 exists"),
            name: "NOVA",
            cooldown: 1.0,
            cost: 0.0,
            range: 6.0,
            targeting: Targeting::SelfCentered { radius: 6.0 },
            effect: crate::ability::AbilityEffect::DirectDamage { amount: 10.0 },
            damage_type: DamageType::Arcane,
            placeholder: true,
        };

        let resolved = query
            .resolve(&world, player, None, &nova)
            .expect("a self-centred ability needs no target");
        assert!(resolved.is_empty());
    }

    #[test]
    fn a_wide_target_is_reachable_from_its_edge() {
        let mut world = GameWorld::new();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));
        let wide = world.spawn(EntityDesc {
            radius: 25.0,
            ..EntityDesc::dummy(Vec3::new(0.0, 0.0, -50.0), 100.0)
        });

        // Centre 50 metres away, edge 24.5 away, against a 30 metre range.
        let mut query = TargetQuery::new();
        assert!(
            query
                .resolve(&world, player, Some(wide), &fireball())
                .is_ok()
        );
    }

    #[test]
    fn resolving_repeatedly_reuses_the_buffer() {
        let positions: Vec<Vec3> = (0..64)
            .map(|index| Vec3::new(index as f32 * 0.1, 0.0, -20.0))
            .collect();
        let (world, player, dummies) = arena(&positions);
        let mut query = TargetQuery::new();

        query
            .resolve(&world, player, Some(dummies[0]), &detonate())
            .expect("the cluster is in range");
        let capacity = query.capacity();
        for _ in 0..100 {
            query
                .resolve(&world, player, Some(dummies[0]), &detonate())
                .expect("the cluster is in range");
        }
        assert_eq!(query.capacity(), capacity, "target resolution reallocated");
    }
}
