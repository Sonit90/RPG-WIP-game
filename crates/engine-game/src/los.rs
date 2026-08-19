//! Line of sight for abilities that are pointed at a target.
//!
//! ```text
//! caster chest  o====================>  target chest
//!                          ^
//!               the first blocking shape between them, if there is one
//! ```
//!
//! ## What this is not
//!
//! It is **not** a visibility or perception system. Nothing here is called by enemy AI, by
//! threat, by aggro, by patrolling or by target acquisition, and there is deliberately no
//! `can_see`: the only question this module answers is whether a *targeted ability* has an
//! unobstructed path from its caster to its target. See `line_of_sight.md` for why the
//! scope is drawn there.
//!
//! ## What it knows
//!
//! Two entities and the static collision world. It does not know which abilities need line
//! of sight, when the check should run or what happens when it fails — those are the
//! ability system's decisions, taken in [`crate::sim`].
//!
//! ## The combat point
//!
//! A [`crate::movement::Body`]'s position is at the **feet**, so neither endpoint may be an
//! entity's origin: a ray between two pairs of feet is stopped by every kerb in the level
//! and passes under every raised barrier. Both ends are lifted by a fraction of the
//! entity's own height, and every targeted ability uses the same fraction —
//! [`CombatPoint::CHEST`] — because two abilities disagreeing about where a character's
//! chest is would be two different answers to one question.

use engine_collision::layer::CollisionMask;
use engine_collision::world::{CollisionWorld, StaticId};
use engine_core::math::Vec3;

use crate::entity::EntityId;
use crate::world::GameWorld;

/// Height above the feet used when an entity has no recorded height, in metres.
///
/// About chest height on a person. A fallback rather than zero: firing from the floor would
/// make every doorway sill a wall.
pub const FALLBACK_COMBAT_HEIGHT: f32 = 1.0;

/// Where on a character a combat query starts or ends.
///
/// A fraction of the entity's height rather than a bone: the simulation has no skeleton —
/// that lives in `engine-assets`, which gameplay cannot name — and a spell that stopped
/// working when an artist renamed a joint would be worse than one aimed at a fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatPoint {
    /// The top of the body: eyes, near enough.
    Head,
    /// The middle of the torso. What targeted abilities use.
    Chest,
    /// Halfway up. The geometric centre of the capsule.
    Center,
}

impl CombatPoint {
    /// What every targeted ability uses, at both ends.
    ///
    /// Chest rather than head at the target end because a head-height ray clears a
    /// chest-high barricade the character is plainly hiding behind, and chest rather than
    /// centre at the caster end because a caster's centre is low enough to be stopped by
    /// the step they are standing on.
    pub const CHEST: Self = Self::Chest;

    /// The fraction of an entity's height this point sits at.
    #[must_use]
    pub fn fraction(self) -> f32 {
        match self {
            Self::Head => 0.9,
            Self::Chest => 0.65,
            Self::Center => 0.5,
        }
    }
}

/// Where on `entity` a combat query starts or ends, in world space.
///
/// `None` for an entity that is not in the world; a caller with no position has no query to
/// run and must not be handed the origin instead.
#[must_use]
pub fn combat_position(world: &GameWorld, entity: EntityId, point: CombatPoint) -> Option<Vec3> {
    let position = world.position(entity)?;
    let height = world.height(entity);
    let lift = if height > 0.0 {
        height * point.fraction()
    } else {
        FALLBACK_COMBAT_HEIGHT
    };
    Some(Vec3::new(position.x, position.y + lift, position.z))
}

/// The answer to one line-of-sight question, and what produced it.
///
/// Carries its own endpoints and blocker rather than just a `bool`, because the developer
/// readout has to show the query gameplay actually ran. A debug view that performs a second
/// query of its own is a debug view that can disagree with the rule it is there to explain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineOfSightCheck {
    /// Whether the target can be reached.
    pub visible: bool,
    /// Where the ray started: the caster's combat point.
    pub from: Vec3,
    /// Where it was aimed: the target's combat point.
    pub to: Vec3,
    /// The blocking object, if one was found.
    pub blocker: Option<StaticId>,
    /// That object's label, or an empty string when nothing blocked.
    pub blocker_label: &'static str,
    /// Where the ray met the blocker, if it met one.
    pub blocked_at: Option<Vec3>,
}

impl LineOfSightCheck {
    /// A clear line between two points.
    #[must_use]
    pub fn clear(from: Vec3, to: Vec3) -> Self {
        Self {
            visible: true,
            from,
            to,
            blocker: None,
            blocker_label: "",
            blocked_at: None,
        }
    }
}

/// Line of sight between two entities, for a targeted ability.
///
/// The whole of the system. It is a free function rather than a type with state because
/// there is no state: the collision world already holds every blocker, and a cache would be
/// a second copy of it that can be wrong.
///
/// ```text
/// world geometry hit first  ->  blocked
/// nothing hit inside the segment  ->  clear
/// ```
///
/// The caster can never block its own ray, and the target can never block the ray aimed at
/// it: characters are not in the collision world at all — only static geometry is — so
/// there is nothing of either to hit. The segment stops exactly at the target's combat
/// point, so a wall *behind* the target is not between the two and does not block.
#[must_use]
pub fn line_of_sight(
    collision: &CollisionWorld,
    world: &GameWorld,
    source: EntityId,
    target: EntityId,
) -> Option<LineOfSightCheck> {
    let from = combat_position(world, source, CombatPoint::CHEST)?;
    let to = combat_position(world, target, CombatPoint::CHEST)?;
    Some(segment_line_of_sight(collision, from, to))
}

/// Line of sight between two points that are already combat points.
///
/// What [`line_of_sight`] does once it has resolved its entities, exposed on its own so a
/// test — and the debug view — can ask about two positions without spawning anything.
#[must_use]
pub fn segment_line_of_sight(collision: &CollisionWorld, from: Vec3, to: Vec3) -> LineOfSightCheck {
    match collision.segment_blocked(from, to, CollisionMask::LINE_OF_SIGHT) {
        Some(hit) => LineOfSightCheck {
            visible: false,
            from,
            to,
            blocker: Some(hit.id),
            blocker_label: collision.label(hit.id),
            blocked_at: Some(hit.hit.point),
        },
        // Two points a millimetre apart have nothing between them, and
        // `segment_blocked` says so rather than dividing by a length of zero.
        None => LineOfSightCheck::clear(from, to),
    }
}

/// Whether `source` has line of sight to `target`.
///
/// The plain question, for a caller that does not need to know what blocked. An entity that
/// is not in the world has no line of sight to anything, which is the same answer the
/// ability system would reach one check earlier.
#[must_use]
pub fn has_line_of_sight(
    collision: &CollisionWorld,
    world: &GameWorld,
    source: EntityId,
    target: EntityId,
) -> bool {
    line_of_sight(collision, world, source, target).is_some_and(|check| check.visible)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::world::EntityDesc;

    fn world_with_two() -> (GameWorld, EntityId, EntityId) {
        let mut world = GameWorld::with_capacity(4);
        let caster = world.spawn(EntityDesc::player(Vec3::new(-5.0, 0.0, 0.0)));
        let target = world.spawn(EntityDesc::dummy(Vec3::new(5.0, 0.0, 0.0), 100.0));
        (world, caster, target)
    }

    fn wall_at(x: f32) -> CollisionWorld {
        let mut collision = CollisionWorld::with_capacity(4);
        collision.add_ground(0.0, "FLOOR");
        collision.add_box(
            Vec3::new(x - 0.5, 0.0, -5.0),
            Vec3::new(x + 0.5, 4.0, 5.0),
            "WALL",
        );
        collision.rebuild_broad_phase();
        collision
    }

    #[test]
    fn a_combat_point_is_above_the_feet_and_below_the_head() {
        let (world, caster, _) = world_with_two();
        let feet = world.position(caster).expect("a position");
        let height = world.height(caster);
        assert!(height > 0.0, "the player has a height");

        let chest = combat_position(&world, caster, CombatPoint::CHEST).expect("a chest");
        assert!(chest.y > feet.y);
        assert!(chest.y < feet.y + height);
        assert_eq!(chest.x, feet.x);
        assert_eq!(chest.z, feet.z);

        let head = combat_position(&world, caster, CombatPoint::Head).expect("a head");
        let centre = combat_position(&world, caster, CombatPoint::Center).expect("a centre");
        assert!(head.y > chest.y && chest.y > centre.y);
    }

    #[test]
    fn an_entity_that_is_not_in_the_world_has_no_combat_point() {
        let (mut world, caster, target) = world_with_two();
        world.despawn(target);
        assert_eq!(combat_position(&world, target, CombatPoint::CHEST), None);
        assert!(combat_position(&world, caster, CombatPoint::CHEST).is_some());
    }

    #[test]
    fn an_open_floor_leaves_the_line_clear() {
        let (world, caster, target) = world_with_two();
        let mut collision = CollisionWorld::with_capacity(4);
        collision.add_ground(0.0, "FLOOR");
        collision.rebuild_broad_phase();

        let check = line_of_sight(&collision, &world, caster, target).expect("both exist");
        assert!(check.visible);
        assert_eq!(check.blocker, None);
        assert_eq!(check.blocker_label, "");
        assert!(has_line_of_sight(&collision, &world, caster, target));
    }

    #[test]
    fn a_wall_between_them_blocks_and_names_itself() {
        let (world, caster, target) = world_with_two();
        let collision = wall_at(0.0);

        let check = line_of_sight(&collision, &world, caster, target).expect("both exist");
        assert!(!check.visible);
        assert_eq!(check.blocker_label, "WALL");
        assert!(check.blocked_at.is_some());
        assert!(!has_line_of_sight(&collision, &world, caster, target));
    }

    #[test]
    fn a_wall_behind_the_target_does_not_block() {
        let (world, caster, target) = world_with_two();
        // Beyond the target at x = 5.
        let collision = wall_at(9.0);
        assert!(has_line_of_sight(&collision, &world, caster, target));
    }

    #[test]
    fn a_missing_entity_has_no_line_of_sight_to_anything() {
        let (mut world, caster, target) = world_with_two();
        world.despawn(target);
        let collision = wall_at(20.0);
        assert_eq!(line_of_sight(&collision, &world, caster, target), None);
        assert!(!has_line_of_sight(&collision, &world, caster, target));
    }

    #[test]
    fn a_barrier_that_stops_no_movement_still_stops_sight() {
        use engine_collision::layer::CollisionFilter;
        use engine_collision::shape::Shape;
        use engine_collision::world::StaticDesc;

        let (world, caster, target) = world_with_two();
        let mut collision = CollisionWorld::with_capacity(4);
        collision.add_ground(0.0, "FLOOR");
        collision.add_static(
            StaticDesc::new(Shape::box_between(
                Vec3::new(-0.5, 0.0, -5.0),
                Vec3::new(0.5, 4.0, 5.0),
            ))
            .labelled("BOSS BARRIER")
            .with_filter(CollisionFilter::los_blocker()),
        );
        collision.rebuild_broad_phase();

        let check = line_of_sight(&collision, &world, caster, target).expect("both exist");
        assert!(!check.visible);
        assert_eq!(check.blocker_label, "BOSS BARRIER");
    }

    #[test]
    fn a_caster_has_sight_of_itself() {
        // The degenerate segment: the two combat points are the same point, and a ray with
        // no length has nothing between its ends.
        let (world, caster, _) = world_with_two();
        let collision = wall_at(0.0);
        assert!(has_line_of_sight(&collision, &world, caster, caster));
    }
}
