//! The events presentation reads, asserted against the gameplay that produces them.
//!
//! ```text
//! ability / movement / zone  ->  CombatEvent  ->  (this test)  ->  VFX, numbers, camera
//! ```
//!
//! Phase 3's architectural claim is that an effect can be placed, sized and timed from the
//! event stream **alone** — never by reaching into the simulation and never by re-deriving
//! a rule. These tests are that claim: each one asserts that what an effect would need is
//! on the event, and that the number on the event is the number the gameplay used.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::event::CombatEvent;
use engine_game::firemage::{FireAbility, ZoneShape};
use engine_game::sim::Simulation;

/// An arena with `dummies` targets, one tick in, with the nearest target selected.
fn arena(dummies: usize) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(dummies));
    simulation.step_once();
    simulation.select_nearest_target();
    simulation
}

/// Every event the run produced, kept across advances.
///
/// The stream is cleared at the start of each batch, so a test that steps repeatedly has to
/// collect as it goes or it only ever sees the last tick.
fn collect(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

#[test]
fn a_damage_event_carries_where_it_landed() {
    // Without this, an impact effect has to look the target up — which fails for exactly
    // the case an effect matters most: the hit that killed it.
    let mut simulation = arena(1);
    let target = simulation.player().target.expect("a dummy is selected");
    let expected = simulation
        .world()
        .position(target)
        .expect("the dummy has a position");

    simulation.push_command(PlayerCommand::UseAbility(0));
    let events = collect(&mut simulation, 90);

    let damage = events
        .iter()
        .find_map(CombatEvent::as_damage)
        .copied()
        .expect("firebolt dealt damage");
    assert_eq!(damage.target, target);
    assert!(
        (damage.position - expected).length() < 1e-3,
        "the hit was placed at {:?}, the dummy stands at {expected:?}",
        damage.position
    );
}

#[test]
fn a_critical_strike_is_expressible_by_the_interface() {
    // The flag has to survive from the roll inside the executor to the event. Forced rather
    // than waited for: at the base chance a test that cast until it saw one would be a slow
    // test that occasionally failed.
    let mut simulation = arena(1);
    simulation.mage_mut().gear.crit_chance = 1.0;

    simulation.push_command(PlayerCommand::UseAbility(0));
    let events = collect(&mut simulation, 90);
    let damage = events
        .iter()
        .find_map(CombatEvent::as_damage)
        .copied()
        .expect("firebolt dealt damage");
    assert!(damage.critical, "a guaranteed crit was reported as normal");

    // And the other way: at zero chance nothing crits, so the flag is the roll rather than
    // a constant.
    let mut simulation = arena(1);
    simulation.mage_mut().gear.crit_chance = -1.0;
    simulation.push_command(PlayerCommand::UseAbility(0));
    let events = collect(&mut simulation, 90);
    let damage = events
        .iter()
        .find_map(CombatEvent::as_damage)
        .copied()
        .expect("firebolt dealt damage");
    assert!(!damage.critical);
}

#[test]
fn a_single_target_cast_announces_the_line_it_travelled() {
    let mut simulation = arena(1);
    let caster = simulation.player().entity;
    let target = simulation.player().target.expect("a dummy is selected");
    let origin = simulation.player_position();

    simulation.push_command(PlayerCommand::UseAbility(0));
    let events = collect(&mut simulation, 90);

    let launch = events
        .iter()
        .position(|event| matches!(event, CombatEvent::ProjectileLaunched { .. }))
        .expect("firebolt announced no projectile");
    let hit = events
        .iter()
        .position(|event| matches!(event, CombatEvent::Damage(_)))
        .expect("firebolt dealt no damage");
    assert!(
        launch < hit,
        "the bolt has to leave before the hit lands in the ordered stream"
    );

    let CombatEvent::ProjectileLaunched {
        source, from, to, ..
    } = events[launch]
    else {
        panic!("the event found is a projectile");
    };
    assert_eq!(source, caster);
    assert!((from - origin).length() < 1e-3, "it left from the caster");
    let destination = simulation
        .world()
        .position(target)
        .expect("the dummy has a position");
    assert!(
        (to - destination).length() < 1e-3,
        "it arrived at the target"
    );
}

#[test]
fn a_delayed_impact_is_announced_when_it_is_committed_to_and_when_it_lands() {
    let mut simulation = arena(4);
    let target = simulation.player().target.expect("a dummy is selected");
    let position = simulation
        .world()
        .position(target)
        .expect("the dummy has a position");
    let definition = FireAbility::Meteor.definition();

    simulation
        .cast_at(FireAbility::Meteor, position)
        .expect("in range");
    let mut events = Vec::from(simulation.events());
    events.extend(collect(&mut simulation, 240));

    let scheduled = events
        .iter()
        .find_map(|event| match *event {
            CombatEvent::ImpactScheduled {
                position,
                radius,
                delay,
                ..
            } => Some((position, radius, delay)),
            _ => None,
        })
        .expect("the meteor was never announced");
    assert!(
        (scheduled.1 - definition.radius).abs() < 1e-4,
        "the radius is the ability's"
    );
    assert!(
        (scheduled.2 - definition.impact_delay).abs() < 1e-4,
        "the delay is the ability's"
    );

    let landed = events
        .iter()
        .find_map(|event| match *event {
            CombatEvent::ImpactLanded {
                position, targets, ..
            } => Some((position, targets)),
            _ => None,
        })
        .expect("the meteor never landed");
    assert!(
        (landed.0 - scheduled.0).length() < 1e-4,
        "it landed somewhere other than where it was aimed"
    );
    assert!(landed.1 > 0, "it landed on a group and reported nobody");
}

#[test]
fn an_area_of_fire_announces_the_shape_that_actually_burns() {
    // The accuracy requirement, stated where it can be checked: the event carries the same
    // `ZoneShape` the containment test uses, so a band drawn from it cannot be a different
    // size from the band that deals damage.
    let mut simulation = arena(4);
    let definition = FireAbility::Firestorm.definition();
    let aim = simulation.player_position() + engine_core::math::Vec3::new(0.0, 0.0, -5.0);
    simulation
        .cast_at(FireAbility::Firestorm, aim)
        .expect("a line is always castable");

    let started = simulation
        .events()
        .iter()
        .find_map(|event| match *event {
            CombatEvent::AreaEffectStarted {
                id,
                shape,
                duration,
                ..
            } => Some((id, shape, duration)),
            _ => None,
        })
        .expect("firestorm started no area");

    let ZoneShape::Line { length, width, .. } = started.1 else {
        panic!("firestorm is a line, got {:?}", started.1);
    };
    assert!((length - definition.range).abs() < 1e-4);
    assert!((width - definition.width).abs() < 1e-4);
    assert!((started.2 - definition.duration).abs() < 1e-4);

    // And it ends, naming the same area, so an observer can take its picture down.
    let events = collect(&mut simulation, 60 * 10);
    let ended = events
        .iter()
        .find_map(|event| match *event {
            CombatEvent::AreaEffectEnded { id, .. } => Some(id),
            _ => None,
        })
        .expect("the fire burned forever");
    assert_eq!(
        ended, started.0,
        "the end names a different area from the start"
    );
}

#[test]
fn leaving_and_touching_the_ground_are_announced() {
    let mut simulation = arena(0);
    simulation.push_command(PlayerCommand::Jump);
    let events = collect(&mut simulation, 180);

    let jumped = events
        .iter()
        .position(|event| matches!(event, CombatEvent::Jumped { .. }))
        .expect("the jump was silent");
    let landed = events
        .iter()
        .position(|event| matches!(event, CombatEvent::Landed { .. }))
        .expect("the landing was silent");
    assert!(jumped < landed, "landed before it jumped");

    let CombatEvent::Landed { speed, .. } = events[landed] else {
        panic!("the event found is a landing");
    };
    assert!(speed > 0.0, "a landing arrived at no speed at all");
}

#[test]
fn the_presentation_events_cost_no_allocation_in_the_steady_state() {
    // They are pushed into the same stream everything else is, and the whole point of that
    // stream is that a busy tick is a `push` into memory that already exists.
    let mut simulation = arena(8);
    simulation.step_many(120);
    let capacity = simulation.event_stream().capacity();

    for _ in 0..30 {
        simulation.push_command(PlayerCommand::UseAbility(0));
        simulation.push_command(PlayerCommand::Jump);
        simulation.step_many(20);
    }
    assert_eq!(
        simulation.event_stream().capacity(),
        capacity,
        "announcing effects made the event stream grow"
    );
}
