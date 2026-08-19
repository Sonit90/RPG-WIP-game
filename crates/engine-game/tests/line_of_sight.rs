//! Targeted-ability line of sight, asserted against a real simulation.
//!
//! One rule, from four directions: the geometry (what blocks and what does not), the ability
//! configuration (who asks for a check and who never does), the cast (which is checked twice,
//! at the start and at the landing) and the level (doors, barriers and decoration).
//!
//! The scene every test uses is deliberately plain:
//!
//! ```text
//!            -Z
//!   target  #3  (0, -20)
//!            |
//!   [ wall spanning x -6..6 at z = -10 ]
//!            |
//!   caster       (0, 0)   facing -Z
//! ```

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::entity::EntityId;
use engine_game::firemage::{AbilityError, FireAbility, TargetMode};
use engine_game::los::{self, CombatPoint};
use engine_game::sim::{CastEnd, LineOfSightPhase, SimConfig, Simulation};
use engine_game::world::EntityDesc;

/// Where the caster stands.
const CASTER: Vec3 = Vec3::new(0.0, 0.0, 0.0);

/// Where the target stands: twenty metres away, inside every relevant ability's range.
const TARGET: Vec3 = Vec3::new(0.0, 0.0, -20.0);

/// An empty room with a caster facing a dummy, and the dummy selected.
fn scene() -> (Simulation, EntityId) {
    let mut simulation = Simulation::new(SimConfig::default());
    let player = simulation.world_mut().spawn(EntityDesc::player(CASTER));
    let target = simulation.world_mut().spawn(EntityDesc::dummy(TARGET, 1e6));
    simulation.set_player_entity(player);
    simulation.player_mut().target = Some(target);

    let collision = simulation.collision_world_mut();
    collision.add_ground(0.0, "FLOOR");
    collision.rebuild_broad_phase();

    simulation.face(TARGET);
    (simulation, target)
}

/// Adds a wall across the room at `z`, tall enough to matter.
fn wall(simulation: &mut Simulation, z: f32, label: &'static str) {
    let collision = simulation.collision_world_mut();
    collision.add_box(
        Vec3::new(-6.0, 0.0, z - 0.5),
        Vec3::new(6.0, 4.0, z + 0.5),
        label,
    );
    collision.rebuild_broad_phase();
}

/// Health remaining on `id`.
fn health(simulation: &Simulation, id: EntityId) -> f32 {
    simulation
        .world()
        .health(id)
        .map_or(0.0, |health| health.current())
}

/// Runs the simulation until the cast in progress finishes, or gives up.
fn finish_cast(simulation: &mut Simulation) {
    for _ in 0..600 {
        if simulation.casting().is_none() {
            return;
        }
        simulation.step_once();
    }
    panic!("the cast never ended");
}

// ------------------------------------------------------------------ basic line of sight

#[test]
fn a_target_in_the_open_can_be_cast_at() {
    let (mut simulation, target) = scene();
    assert!(
        simulation.cast(FireAbility::Firebolt).is_ok(),
        "an empty room refused a cast"
    );
    finish_cast(&mut simulation);
    assert!(health(&simulation, target) < 1e6, "the bolt did not land");
}

#[test]
fn a_wall_between_the_two_refuses_the_cast() {
    let (mut simulation, target) = scene();
    wall(&mut simulation, -10.0, "DUNGEON WALL");

    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(AbilityError::NoLineOfSight)
    );
    assert_eq!(
        simulation.recent_error(),
        Some(AbilityError::NoLineOfSight),
        "the refusal was not reported through the existing error channel"
    );
    assert_eq!(health(&simulation, target), 1e6, "something still landed");

    let record = simulation
        .last_line_of_sight()
        .expect("the check was recorded");
    assert!(!record.visible());
    assert_eq!(record.blocker_label(), "DUNGEON WALL");
    assert_eq!(record.phase, LineOfSightPhase::Activation);
    assert!(
        record.in_range,
        "the target was refused for the wrong thing"
    );
}

#[test]
fn several_walls_refuse_the_cast_and_the_nearest_is_named() {
    let (mut simulation, _) = scene();
    wall(&mut simulation, -14.0, "FAR WALL");
    wall(&mut simulation, -5.0, "NEAR WALL");

    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(AbilityError::NoLineOfSight)
    );
    assert_eq!(
        simulation
            .last_line_of_sight()
            .expect("a check")
            .blocker_label(),
        "NEAR WALL"
    );
}

#[test]
fn neither_the_caster_nor_anybody_standing_between_them_blocks_the_ray() {
    // Characters are not in the collision world at all, so a body can never be a blocker —
    // not the caster's own, and not a third one standing directly in the line.
    let (mut simulation, target) = scene();
    simulation
        .world_mut()
        .spawn(EntityDesc::dummy(Vec3::new(0.0, 0.0, -10.0), 100.0));

    assert!(simulation.cast(FireAbility::Firebolt).is_ok());
    finish_cast(&mut simulation);
    assert!(health(&simulation, target) < 1e6);
}

#[test]
fn a_wall_behind_the_target_is_not_between_them() {
    // The endpoint rule: the ray is exactly as long as the distance to the target.
    let (mut simulation, _) = scene();
    wall(&mut simulation, -25.0, "BACK WALL");
    assert!(simulation.cast(FireAbility::Firebolt).is_ok());
}

#[test]
fn the_ray_leaves_the_chest_rather_than_the_feet() {
    // A body's position is at its feet. A knee-high kerb between the two must not stop a
    // spell, and it would if either endpoint were an entity's origin.
    let (mut simulation, _) = scene();
    let collision = simulation.collision_world_mut();
    collision.add_box(
        Vec3::new(-6.0, 0.0, -10.5),
        Vec3::new(6.0, 0.3, -9.5),
        "KERB",
    );
    collision.rebuild_broad_phase();

    assert!(simulation.cast(FireAbility::Firebolt).is_ok());

    let from = los::combat_position(
        simulation.world(),
        simulation.player().entity,
        CombatPoint::CHEST,
    )
    .expect("a caster");
    assert!(from.y > 0.5, "the ray started at the floor");
}

// ------------------------------------------------------------------ ability configuration

#[test]
fn only_abilities_that_ask_for_a_check_get_one() {
    for ability in FireAbility::ALL {
        let definition = ability.definition();
        assert_eq!(
            definition.checks_line_of_sight(),
            definition.requires_line_of_sight && definition.target == TargetMode::Single,
            "{} disagrees with its own configuration",
            definition.name
        );
        if definition.target != TargetMode::Single {
            assert!(
                !definition.checks_line_of_sight(),
                "{} is not aimed at a target and must never cast a ray",
                definition.name
            );
        }
    }
    // The filler is the worked example in the design document.
    assert!(FireAbility::Firebolt.definition().checks_line_of_sight());
    // And a movement is not.
    assert!(!FireAbility::Blink.definition().checks_line_of_sight());
}

#[test]
fn an_ability_that_does_not_require_line_of_sight_never_asks() {
    // The flag is what decides, not the target mode: a single-target ability that has not
    // opted in casts no ray, which is both the backwards-compatible answer and the reason
    // the check costs nothing for abilities that do not want it.
    let mut definition = *FireAbility::Firebolt.definition();
    assert!(definition.checks_line_of_sight());
    definition.requires_line_of_sight = false;
    assert!(!definition.checks_line_of_sight());

    // And the converse: the flag alone is not enough without a target to have a line to.
    let mut aimed = *FireAbility::Meteor.definition();
    aimed.requires_line_of_sight = true;
    assert!(!aimed.checks_line_of_sight());
}

#[test]
fn a_non_targeted_ability_works_through_a_wall_and_records_no_check() {
    let (mut simulation, _) = scene();
    wall(&mut simulation, -10.0, "DUNGEON WALL");

    // Aimed rather than targeted: the wall is not consulted, and no ray is cast.
    assert!(simulation.cast(FireAbility::FlameCone).is_ok());
    assert_eq!(simulation.last_line_of_sight(), None);

    // Nor by a placed ability, once the global cooldown from the last one is over.
    simulation.step_many(60);
    assert!(simulation.cast_at(FireAbility::Meteor, TARGET).is_ok());
    assert_eq!(simulation.last_line_of_sight(), None);
}

// ------------------------------------------------------------------ casts

#[test]
fn a_cast_that_keeps_its_line_lands() {
    let (mut simulation, target) = scene();
    assert!(simulation.cast(FireAbility::Pyroblast).is_ok());
    finish_cast(&mut simulation);

    assert!(health(&simulation, target) < 1e6, "the cast did not land");
    let record = simulation.last_line_of_sight().expect("a final check");
    assert_eq!(record.phase, LineOfSightPhase::CastCompletion);
    assert!(record.visible());
}

#[test]
fn a_target_that_goes_behind_a_wall_mid_cast_is_not_hit() {
    let (mut simulation, target) = scene();
    // A pillar off to one side, and a target that walks behind it while the cast runs.
    let collision = simulation.collision_world_mut();
    collision.add_box(
        Vec3::new(8.0, 0.0, -12.0),
        Vec3::new(12.0, 5.0, -8.0),
        "PILLAR",
    );
    collision.rebuild_broad_phase();

    assert!(simulation.cast(FireAbility::Pyroblast).is_ok());
    simulation.step_many(30);
    // Straight behind the pillar, seen from the origin.
    if let Some(body) = simulation.world_mut().body_mut(target) {
        body.position = Vec3::new(20.0, 0.0, -20.0);
    }
    finish_cast(&mut simulation);

    assert_eq!(
        health(&simulation, target),
        1e6,
        "a spell landed through a pillar"
    );
    assert_eq!(simulation.recent_error(), Some(AbilityError::NoLineOfSight));
    let record = simulation.last_line_of_sight().expect("a final check");
    assert_eq!(record.phase, LineOfSightPhase::CastCompletion);
    assert!(!record.visible());
    assert_eq!(record.blocker_label(), "PILLAR");
}

#[test]
fn the_end_of_a_cast_lost_to_a_wall_says_which_wall_it_was() {
    let (mut simulation, target) = scene();
    assert!(simulation.cast(FireAbility::Pyroblast).is_ok());
    simulation.step_many(30);
    wall(&mut simulation, -10.0, "BOSS WALL");
    finish_cast(&mut simulation);

    let ended = simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            engine_game::event::CombatEvent::CastEnded { end, .. } => Some(*end),
            _ => None,
        })
        .next_back()
        .expect("the cast ended");
    assert_eq!(ended, CastEnd::LineOfSightLost);
    assert_eq!(health(&simulation, target), 1e6);
}

#[test]
fn a_target_that_comes_back_into_view_before_the_cast_ends_is_hit() {
    // Only the final check decides. There is no per-frame polling, so a target that ducks
    // behind a pillar and steps back out is hit.
    let (mut simulation, target) = scene();
    let collision = simulation.collision_world_mut();
    collision.add_box(
        Vec3::new(8.0, 0.0, -12.0),
        Vec3::new(12.0, 5.0, -8.0),
        "PILLAR",
    );
    collision.rebuild_broad_phase();

    assert!(simulation.cast(FireAbility::Pyroblast).is_ok());
    simulation.step_many(30);
    if let Some(body) = simulation.world_mut().body_mut(target) {
        body.position = Vec3::new(20.0, 0.0, -20.0);
    }
    simulation.step_many(30);
    if let Some(body) = simulation.world_mut().body_mut(target) {
        body.position = TARGET;
    }
    finish_cast(&mut simulation);

    assert!(
        health(&simulation, target) < 1e6,
        "a target back in the open was not hit"
    );
    assert!(
        simulation
            .last_line_of_sight()
            .expect("a final check")
            .visible()
    );
}

// ------------------------------------------------------------------ the level

#[test]
fn a_closed_door_blocks_and_an_open_one_does_not() {
    // A door is not a kind of thing the line-of-sight system knows about: it is a shape that
    // is in the collision world while it is shut and out of it while it is open.
    let (mut simulation, _) = scene();
    let door = {
        let collision = simulation.collision_world_mut();
        let door = collision.add_box(
            Vec3::new(-3.0, 0.0, -10.5),
            Vec3::new(3.0, 4.0, -9.5),
            "DOOR",
        );
        collision.rebuild_broad_phase();
        door
    };
    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(AbilityError::NoLineOfSight)
    );

    let collision = simulation.collision_world_mut();
    assert!(collision.remove_static(door), "the door would not open");
    collision.rebuild_broad_phase();
    assert!(
        simulation.cast(FireAbility::Firebolt).is_ok(),
        "an open door still blocked the spell"
    );
}

#[test]
fn a_barrier_that_stops_nobody_walking_still_stops_the_spell() {
    let (mut simulation, _) = scene();
    let collision = simulation.collision_world_mut();
    collision.add_los_blocker(
        Vec3::new(-6.0, 0.0, -10.5),
        Vec3::new(6.0, 5.0, -9.5),
        "BOSS BARRIER",
    );
    collision.rebuild_broad_phase();

    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(AbilityError::NoLineOfSight)
    );
    assert_eq!(
        simulation
            .last_line_of_sight()
            .expect("a check")
            .blocker_label(),
        "BOSS BARRIER"
    );
}

#[test]
fn decoration_does_not_block_because_it_is_not_collision() {
    // Decorative geometry has no collision shape at all, so it is not in the world the ray
    // is fired through. The banner below is what the level would have added if it did have
    // one, on a layer no ability asks about.
    use engine_collision::layer::{CollisionFilter, CollisionLayer, CollisionMask};
    use engine_collision::shape::Shape;
    use engine_collision::world::StaticDesc;

    let (mut simulation, _) = scene();
    let collision = simulation.collision_world_mut();
    collision.add_static(
        StaticDesc::new(Shape::box_between(
            Vec3::new(-6.0, 0.0, -10.5),
            Vec3::new(6.0, 4.0, -9.5),
        ))
        .labelled("BANNER")
        .with_filter(CollisionFilter::new(
            CollisionLayer::DUMMY,
            CollisionMask::NONE,
        )),
    );
    collision.rebuild_broad_phase();

    assert!(
        simulation.cast(FireAbility::Firebolt).is_ok(),
        "decoration stopped a spell"
    );
}

#[test]
fn a_target_out_of_range_is_refused_for_the_range_and_not_for_the_wall() {
    // Ordering: the check that was already there answers first, so a familiar refusal does
    // not turn into a new one.
    let (mut simulation, target) = scene();
    wall(&mut simulation, -10.0, "DUNGEON WALL");
    if let Some(body) = simulation.world_mut().body_mut(target) {
        body.position = Vec3::new(0.0, 0.0, -200.0);
    }
    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(AbilityError::OutOfRange)
    );
}
