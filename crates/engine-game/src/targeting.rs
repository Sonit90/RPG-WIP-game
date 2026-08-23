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
//! Selection is a command either way it is asked for:
//!
//! ```text
//! CycleTarget                    -> the next hostile by distance, within the class's reach
//! PickTarget { origin, dir }     -> whatever a ray meets, nearest along the ray
//! ```
//!
//! Both are commands rather than direct writes, and the click carries a *ray* rather than an
//! entity: the caller owns the camera and the cursor, and this module owns who is hostile,
//! who is alive and what shape a body is. Nothing here knows a screen exists.
//!
//! Cycling is limited to the longest range the class has, because a target nothing on the bar
//! can reach is a target the player would have to cycle past. Clicking is not: pointing at
//! something across a room is unambiguous, and a click that was silently refused would be
//! worse than a selection that is out of range and says so.
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
        self.hostiles_within(world, from, f32::INFINITY)
    }

    /// The living hostiles within `max_range` metres, nearest first.
    ///
    /// `max_range` is measured the way every range test in this crate is: between the two
    /// bodies' edges, so a wide enemy is in range when its side is. `f32::INFINITY` asks for
    /// all of them.
    ///
    /// This is what makes cycling useful rather than exhaustive. In an arena with one pack in
    /// it the two questions have the same answer; in a dungeon with twenty-three enemies
    /// standing in five rooms, "the next hostile in the world" is almost always something
    /// behind a wall a hundred metres away.
    pub fn hostiles_within(
        &mut self,
        world: &GameWorld,
        from: EntityId,
        max_range: f32,
    ) -> &[EntityId] {
        self.scratch.clear();
        self.ranked.clear();

        let Some(faction) = world.faction(from) else {
            return &self.scratch;
        };

        for candidate in world.entities() {
            if candidate == from || !world.is_alive(candidate) || !Self::is_combatant(world, candidate)
            {
                continue;
            }
            if !world
                .faction(candidate)
                .is_some_and(|other| faction.is_hostile_to(other))
            {
                continue;
            }
            let distance = world.distance_between(from, candidate).unwrap_or(f32::MAX);
            if distance > max_range {
                continue;
            }
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
            if candidate == from || !world.is_alive(candidate) || !Self::is_combatant(world, candidate)
            {
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

    /// The nearest living hostile within `max_range`, if there is one.
    pub fn nearest_hostile_within(
        &mut self,
        world: &GameWorld,
        from: EntityId,
        max_range: f32,
    ) -> Option<EntityId> {
        self.hostiles_within(world, from, max_range)
            .first()
            .copied()
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
        self.next_hostile_within(world, from, current, f32::INFINITY)
    }

    /// The next living hostile after `current` within `max_range`, wrapping around.
    ///
    /// A target that has walked out of range is not in the list, so the cycle restarts from
    /// the nearest one that is — which is what a player pressing the key after their target
    /// ran away expects, and the same rule a dead target already followed.
    ///
    /// Returns `None` when nothing is in range at all. The caller decides what that means;
    /// the simulation keeps the target it had, because a key that silently deselected would
    /// be worse than one that did nothing.
    pub fn next_hostile_within(
        &mut self,
        world: &GameWorld,
        from: EntityId,
        current: Option<EntityId>,
        max_range: f32,
    ) -> Option<EntityId> {
        let ordered = self.hostiles_within(world, from, max_range);
        if ordered.is_empty() {
            return None;
        }
        let index = current.and_then(|target| ordered.iter().position(|id| *id == target));
        match index {
            Some(index) => ordered.get((index + 1) % ordered.len()).copied(),
            None => ordered.first().copied(),
        }
    }

    /// Whether `candidate` is something that can be fought at all.
    ///
    /// Everything except a healing dummy, which is a *target for practice* standing in the
    /// player's own faction. Without this an enemy would walk past the player to hit an inert
    /// post that never fights back, and the pull the level was built around would be decided
    /// by whichever body happened to be nearer.
    #[must_use]
    pub fn is_combatant(world: &GameWorld, candidate: EntityId) -> bool {
        world.kind(candidate) != Some(crate::world::EntityKind::HealingDummy)
    }

    /// Whether `viewer` is allowed to select `candidate`.
    ///
    /// Anything alive, including the viewer itself and anything friendly. Selection is
    /// "what am I looking at", not "what am I about to hit" — a player selects an ally to
    /// read their health, and selects themselves for the same reason.
    ///
    /// The restriction that matters lives on the *ability* instead: what a spell may be
    /// pointed at is the spell's business, and the FireMage's single-target ones refuse a
    /// friendly target. Putting the rule there rather than here is what makes a future heal
    /// possible without changing what a click means.
    ///
    /// `viewer` is unused today and is kept because the question is genuinely about a pair:
    /// a future faction that cannot see stealth, or a duel that hides the rest of a party,
    /// answers it differently for two different viewers.
    #[must_use]
    pub fn is_selectable(world: &GameWorld, viewer: EntityId, candidate: EntityId) -> bool {
        let _ = viewer;
        world.is_alive(candidate)
    }

    /// The nearest selectable body a ray passes through, if it passes through one.
    ///
    /// The gameplay half of clicking on something. Presentation owns the camera and turns a
    /// cursor position into a ray; this owns *what a ray hits*, which is a question about
    /// bodies and factions and has no business knowing that a screen exists.
    ///
    /// A body is tested as an upright cylinder of the entity's own radius and height — the
    /// same two numbers collision and every range test use — so what the player clicks on is
    /// the shape the game already thinks is there. `direction` need not be normalised.
    ///
    /// Nearest along the ray wins, not nearest to the camera: two enemies in a line are
    /// picked front to back, which is what pointing at the front one means.
    ///
    /// The viewer's own body is a candidate. Clicking your character selects you, which is
    /// how every game in the genre lets a player look at themselves — and the reason a ray
    /// that starts *inside* a body reports a hit at zero rather than missing.
    pub fn pick_selectable(
        &self,
        world: &GameWorld,
        from: EntityId,
        origin: engine_core::math::Vec3,
        direction: engine_core::math::Vec3,
    ) -> Option<EntityId> {
        let mut best: Option<(f32, EntityId)> = None;

        for candidate in world.entities() {
            if !Self::is_selectable(world, from, candidate) {
                continue;
            }
            let Some(position) = world.position(candidate) else {
                continue;
            };
            let Some(distance) = ray_hits_body(
                origin,
                direction,
                position,
                world.radius(candidate),
                world.height(candidate),
            ) else {
                continue;
            };
            // A camera zoomed all the way in sits inside its own character, and every ray it
            // casts starts inside that body. Selecting yourself with every click because the
            // camera is in your head is not what the player meant — and a body the ray starts
            // inside is a body they cannot see to click on.
            if candidate == from && distance <= 0.0 {
                continue;
            }
            // The id is the tie-break for the same reason it is everywhere else in this file:
            // two bodies at exactly the same depth must not swap between runs.
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

/// How far along `direction` a ray from `origin` first meets an upright cylinder.
///
/// The cylinder stands on `feet` with `radius` and `height`. Returns the distance in units of
/// `direction`'s length, or `None` if the ray misses, or only meets it behind the origin.
///
/// A cylinder rather than a capsule: a capsule's rounded cap makes the top of a body slightly
/// harder to click than its middle, which is a difference nobody can see and a solver nobody
/// should have to read. The two disagree only within half a radius of the crown.
fn ray_hits_body(
    origin: engine_core::math::Vec3,
    direction: engine_core::math::Vec3,
    feet: engine_core::math::Vec3,
    radius: f32,
    height: f32,
) -> Option<f32> {
    let height = height.max(0.0);

    // The infinite cylinder, in the horizontal plane: a quadratic in the ray parameter, whose
    // two roots bound the span of the ray that is inside the circle.
    let (dx, dz) = (direction.x, direction.z);
    let (ox, oz) = (origin.x - feet.x, origin.z - feet.z);
    let a = dx.mul_add(dx, dz * dz);
    let (mut enter, mut exit) = if a <= f32::EPSILON {
        // Straight up or straight down: it is inside the circle for its whole length, or not
        // at all.
        if ox.mul_add(ox, oz * oz) > radius * radius {
            return None;
        }
        (f32::NEG_INFINITY, f32::INFINITY)
    } else {
        let b = 2.0 * dx.mul_add(ox, dz * oz);
        let c = ox.mul_add(ox, oz * oz) - radius * radius;
        let discriminant = b.mul_add(b, -(4.0 * a * c));
        if discriminant < 0.0 {
            return None;
        }
        let root = discriminant.sqrt();
        ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a))
    };

    // Then the slab between the feet and the crown. Intersecting the two spans rather than
    // testing the side and the caps separately is what makes a ray from a camera looking
    // steeply down — which enters through the top, not the side — hit the body it is
    // pointing at.
    if direction.y.abs() <= f32::EPSILON {
        if origin.y < feet.y || origin.y > feet.y + height {
            return None;
        }
    } else {
        let low = (feet.y - origin.y) / direction.y;
        let high = (feet.y + height - origin.y) / direction.y;
        enter = enter.max(low.min(high));
        exit = exit.min(low.max(high));
    }

    if exit < 0.0 || enter > exit {
        return None;
    }
    // A ray that started inside the body is touching it right now, at zero.
    Some(enter.max(0.0))
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
    fn cycling_only_offers_what_the_class_can_reach() {
        // Three dummies at 2, 4 and 6 m, and a fourth well outside anything's range. The far
        // one is never selected, and the cycle wraps at the last one that is in range.
        let (mut world, player, dummies) = world();
        let far = world.spawn(EntityDesc::dummy(Vec3::new(0.0, 0.0, -80.0), 100.0));
        let mut query = TargetQuery::with_capacity(8);

        let mut current = None;
        for expected in &dummies {
            current = query.next_hostile_within(&world, player, current, 35.0);
            assert_eq!(current, Some(*expected));
        }
        assert_eq!(
            query.next_hostile_within(&world, player, current, 35.0),
            Some(dummies[0]),
            "the cycle wrapped past the end of what is in range"
        );
        assert!(
            !query.hostiles_within(&world, player, 35.0).contains(&far),
            "something out of range is in the cycle"
        );
        // And with everything out of range there is nothing to move to at all.
        assert_eq!(query.next_hostile_within(&world, player, None, 0.05), None);
    }

    #[test]
    fn a_target_that_walked_out_of_range_restarts_the_cycle_at_the_nearest() {
        let (mut world, player, dummies) = world();
        let mut query = TargetQuery::with_capacity(8);
        // The third dummy is selected, and then moves out of reach.
        let current = Some(dummies[2]);
        if let Some(body) = world.body_mut(dummies[2]) {
            body.position = Vec3::new(0.0, 0.0, -90.0);
        }
        assert_eq!(
            query.next_hostile_within(&world, player, current, 35.0),
            Some(dummies[0]),
            "a target out of range should restart the cycle, as a dead one does"
        );
    }

    #[test]
    fn a_ray_down_the_line_of_dummies_picks_the_nearest_one() {
        let (world, player, dummies) = world();
        let query = TargetQuery::with_capacity(8);
        // From above, in front of the player's own body, looking down the line of dummies.
        // Started past the player deliberately: the player is selectable too, and a ray that
        // crossed them would be picking them rather than what it is aimed along.
        let origin = Vec3::new(0.0, 3.0, -0.5);
        let direction = Vec3::new(0.0, -2.0, -2.0);
        assert_eq!(
            query.pick_selectable(&world, player, origin, direction),
            Some(dummies[0]),
            "a ray through three bodies picked something other than the first"
        );
    }

    #[test]
    fn a_ray_that_meets_nothing_picks_nothing() {
        let (world, player, _) = world();
        let query = TargetQuery::with_capacity(8);
        // Off to one side, past everything.
        assert_eq!(
            query.pick_selectable(
                &world,
                player,
                Vec3::new(50.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, -1.0)
            ),
            None
        );
    }

    #[test]
    fn clicking_your_own_character_selects_you() {
        // The one selection that is always available, and the reason the picker considers the
        // viewer's own body at all.
        let (world, player, _) = world();
        let query = TargetQuery::with_capacity(8);
        assert_eq!(
            query.pick_selectable(
                &world,
                player,
                Vec3::new(0.0, 10.0, 0.0),
                Vec3::new(0.0, -1.0, 0.0)
            ),
            Some(player)
        );
        assert!(TargetQuery::is_selectable(&world, player, player));
    }

    #[test]
    fn a_camera_inside_its_own_character_does_not_select_it() {
        // Zoomed all the way in, the ray starts inside the player's own body. Selecting
        // yourself with every click because the camera is in your head is not a click.
        let (world, player, dummies) = world();
        let query = TargetQuery::with_capacity(8);
        assert_eq!(
            query.pick_selectable(
                &world,
                player,
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, -1.0)
            ),
            Some(dummies[0]),
            "the body the ray started inside was picked instead of what it points at"
        );
    }

    #[test]
    fn a_ray_from_straight_overhead_hits_the_body_it_is_over() {
        // The steep case: the ray enters through the crown rather than the side, which a
        // side-only cylinder test misses entirely.
        let (world, player, dummies) = world();
        let query = TargetQuery::with_capacity(8);
        let above = Vec3::new(0.0, 20.0, -4.0);
        assert_eq!(
            query.pick_selectable(&world, player, above, Vec3::new(0.0, -1.0, 0.0)),
            Some(dummies[1])
        );
        // And a hand's width to the side of it hits nothing at all.
        assert_eq!(
            query.pick_selectable(
                &world,
                player,
                Vec3::new(6.0, 20.0, -4.0),
                Vec3::new(0.0, -1.0, 0.0)
            ),
            None
        );
    }

    #[test]
    fn a_ray_pointing_away_from_a_body_does_not_pick_it() {
        // Off to the side, pointing further off to the side: everything is behind the ray.
        let (world, player, _) = world();
        let query = TargetQuery::with_capacity(8);
        assert_eq!(
            query.pick_selectable(
                &world,
                player,
                Vec3::new(5.0, 1.0, -4.0),
                Vec3::new(1.0, 0.0, 0.0)
            ),
            None,
            "a body behind the ray's origin was picked"
        );
    }

    #[test]
    fn a_dead_body_cannot_be_clicked() {
        let (mut world, player, dummies) = world();
        let query = TargetQuery::with_capacity(8);
        let origin = Vec3::new(0.0, 3.0, -0.5);
        let direction = Vec3::new(0.0, -2.0, -2.0);
        assert_eq!(
            query.pick_selectable(&world, player, origin, direction),
            Some(dummies[0])
        );
        world.damage(dummies[0], 1_000.0);
        assert_eq!(
            query.pick_selectable(&world, player, origin, direction),
            Some(dummies[1]),
            "a corpse was clicked"
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
