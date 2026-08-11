//! Milestone 22: the same commands from the same state produce the same state.
//!
//! Determinism is not a nice property here, it is the precondition for the things Phase 3
//! is a foundation for: a recorded replay, a server that re-simulates a client's input, and
//! a bug report that reproduces. There is no RNG in the simulation, so the only ways
//! determinism can break are a dependency on wall-clock time, on the frame rate, or on
//! iteration order over a hash container. Each of those is asserted against below.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::time::Duration;

use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::event::CombatEvent;
use engine_game::sim::Simulation;

/// The sequence the milestone asks for: move, move, jump, then the three abilities.
fn script() -> Vec<(u32, PlayerCommand)> {
    let ability = |slot: usize| PlayerCommand::UseAbility(slot);
    vec![
        (
            10,
            PlayerCommand::Move {
                forward: 1.0,
                right: 0.0,
                yaw: 0.0,
                face_camera: false,
            },
        ),
        (
            20,
            PlayerCommand::Move {
                forward: 0.5,
                right: 1.0,
                yaw: 0.7,
                face_camera: false,
            },
        ),
        (30, PlayerCommand::Jump),
        (40, ability(0)),
        (60, ability(1)),
        (90, ability(2)),
        (
            120,
            PlayerCommand::Move {
                forward: 0.0,
                right: 0.0,
                yaw: 0.7,
                face_camera: false,
            },
        ),
    ]
}

/// A comparable summary of everything the simulation ended up holding.
#[derive(Debug, PartialEq)]
struct Snapshot {
    tick: u64,
    time_ms: i64,
    player: (i64, i64, i64),
    velocity: (i64, i64, i64),
    grounded: bool,
    state: &'static str,
    heat: i64,
    energy: i64,
    cooldowns: Vec<i64>,
    health: Vec<i64>,
    burning: Vec<i64>,
    total_damage: i64,
    dps: i64,
    per_ability: Vec<i64>,
    events: usize,
}

/// Fixed-point, because two `f32` values that differ in the last bit are not the same state
/// and comparing them directly is what makes a determinism test useless.
fn fixed(value: f32) -> i64 {
    (f64::from(value) * 1e4).round() as i64
}

fn snapshot(simulation: &Simulation) -> Snapshot {
    let world = simulation.world();
    let body = simulation.player_body().expect("the player exists");
    Snapshot {
        tick: simulation.tick_count(),
        time_ms: (simulation.time() * 1e3).round() as i64,
        player: (
            fixed(body.position.x),
            fixed(body.position.y),
            fixed(body.position.z),
        ),
        velocity: (
            fixed(body.velocity.x),
            fixed(body.velocity.y),
            fixed(body.velocity.z),
        ),
        grounded: body.grounded,
        state: body.state.as_str(),
        heat: fixed(simulation.mage().heat.value()),
        energy: fixed(simulation.mage().energy.value()),
        cooldowns: engine_game::firemage::FireAbility::ALL
            .into_iter()
            .map(|ability| fixed(simulation.mage().abilities.recharge_remaining(ability)))
            .collect(),
        health: arena::dummies(world)
            .filter_map(|id| world.health(id))
            .map(|health| fixed(health.current()))
            .collect(),
        burning: arena::dummies(world)
            .map(|id| i64::from(simulation.mage().ignite.stacks(id)))
            .collect(),
        total_damage: (simulation.meter().total_damage() * 1e4).round() as i64,
        dps: (simulation.meter().dps() * 1e4).round() as i64,
        per_ability: simulation
            .meter()
            .abilities()
            .iter()
            .map(|totals| (totals.damage * 1e4).round() as i64)
            .collect(),
        events: simulation.event_stream().total_recorded() as usize,
    }
}

/// Runs the script for `ticks` fixed ticks, one tick at a time.
fn run_scripted(ticks: u32) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    let script = script();
    for tick in 0..ticks {
        for (at, command) in &script {
            if *at == tick {
                simulation.push_command(*command);
            }
        }
        simulation.step_once();
    }
    simulation
}

#[test]
fn the_same_script_twice_produces_the_same_state() {
    let first = run_scripted(600);
    let second = run_scripted(600);
    assert_eq!(snapshot(&first), snapshot(&second));
}

#[test]
fn the_same_script_produces_the_same_events_in_the_same_order() {
    // State equality is not enough: two runs that arrive at the same health by different
    // routes would replicate differently over a network.
    let mut first = arena::training_arena(&ArenaConfig::default());
    let mut second = arena::training_arena(&ArenaConfig::default());
    let script = script();
    let mut first_events: Vec<String> = Vec::new();
    let mut second_events: Vec<String> = Vec::new();

    for (simulation, log) in [
        (&mut first, &mut first_events),
        (&mut second, &mut second_events),
    ] {
        for tick in 0..600 {
            for (at, command) in &script {
                if *at == tick {
                    simulation.push_command(*command);
                }
            }
            simulation.step_once();
            log.extend(simulation.events().iter().map(describe));
        }
    }

    assert!(!first_events.is_empty(), "the script produced no events");
    assert_eq!(first_events, second_events);
}

/// A stable textual form of an event, for comparing two runs.
fn describe(event: &CombatEvent) -> String {
    match *event {
        CombatEvent::AbilityUsed {
            source,
            ability,
            targets,
            tick,
        } => format!(
            "used {} by {}:{} on {targets} at {tick}",
            ability.slot(),
            source.index(),
            source.generation()
        ),
        CombatEvent::Damage(damage) => format!(
            "damage {} to {}:{} by {} amount {} overkill {} killed {} at {}",
            damage.ability.slot(),
            damage.target.index(),
            damage.target.generation(),
            damage.source.index(),
            fixed(damage.amount),
            fixed(damage.overkill),
            damage.killed,
            damage.tick
        ),
        CombatEvent::Healing {
            target,
            amount,
            ability,
            tick,
            ..
        } => format!(
            "heal {} to {} amount {} at {tick}",
            ability.slot(),
            target.index(),
            fixed(amount)
        ),
        CombatEvent::StatusApplied {
            target,
            status,
            duration,
            tick,
            ..
        } => format!(
            "status {} on {} for {} at {tick}",
            status.as_str(),
            target.index(),
            fixed(duration)
        ),
        CombatEvent::StatusRemoved {
            target,
            status,
            reason,
            tick,
        } => format!(
            "status {} off {} because {reason:?} at {tick}",
            status.as_str(),
            target.index()
        ),
        CombatEvent::Death {
            entity,
            killer,
            tick,
        } => format!("death {} by {} at {tick}", entity.index(), killer.index()),
        // Where a teleport put someone is part of the run: two replays that blinked into
        // different geometry are different runs however equal their health totals are.
        CombatEvent::Teleported {
            entity,
            from,
            to,
            tick,
        } => format!(
            "teleport {} from {} {} {} to {} {} {} at {tick}",
            entity.index(),
            fixed(from.x),
            fixed(from.y),
            fixed(from.z),
            fixed(to.x),
            fixed(to.y),
            fixed(to.z)
        ),
        // Cast events are part of the ordered stream a replay has to reproduce: a cast that
        // started on a different tick, or was cancelled for a different reason, is a
        // different run even if the health totals agree.
        CombatEvent::CastStarted {
            source,
            ability,
            duration,
            tick,
        } => format!(
            "cast-start {} {} for {duration} at {tick}",
            source.index(),
            ability.slot()
        ),
        CombatEvent::CastEnded {
            source,
            ability,
            end,
            tick,
        } => format!(
            "cast-end {} {} {end:?} at {tick}",
            source.index(),
            ability.slot()
        ),
    }
}

#[test]
fn the_frame_rate_does_not_change_the_outcome() {
    // The property the fixed timestep exists for. Both runs simulate two seconds; one is
    // delivered as 480 FPS frames and the other as 60 FPS frames.
    let mut fast = arena::training_arena(&ArenaConfig::default());
    let mut slow = arena::training_arena(&ArenaConfig::default());

    let cast = PlayerCommand::UseAbility(1);
    fast.push_command(cast);
    slow.push_command(cast);

    // Exactly the same total real time, delivered in chunks of eight to one. The totals
    // have to match to the nanosecond: whole ticks are what the accumulator hands out, so
    // two runs whose totals straddle a tick boundary legitimately differ by one tick, and
    // that would be a test about rounding rather than about the frame rate.
    const FAST_FRAME: Duration = Duration::from_nanos(2_083_333);
    const SLOW_FRAME: Duration = Duration::from_nanos(2_083_333 * 8);
    for _ in 0..960 {
        fast.advance(FAST_FRAME);
    }
    for _ in 0..120 {
        slow.advance(SLOW_FRAME);
    }

    let fast_snapshot = snapshot(&fast);
    let slow_snapshot = snapshot(&slow);
    assert_eq!(
        fast_snapshot.tick, slow_snapshot.tick,
        "two seconds must be the same number of ticks at any frame rate"
    );
    assert_eq!(fast_snapshot.health, slow_snapshot.health);
    assert_eq!(fast_snapshot.total_damage, slow_snapshot.total_damage);
    assert_eq!(fast_snapshot.cooldowns, slow_snapshot.cooldowns);
}

#[test]
fn a_reset_returns_to_a_state_a_fresh_arena_would_have() {
    // Resetting is only useful if the state afterwards is the state a tester thinks it is.
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    let fresh = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();

    for slot in [0, 1, 2] {
        simulation.push_command(PlayerCommand::UseAbility(slot));
        simulation.step_many(4);
    }
    simulation.step_many(300);
    simulation.push_command(PlayerCommand::ResetDummies);
    simulation.step_once();

    let after = snapshot(&simulation);
    let mut baseline = fresh;
    baseline.step_once();
    let expected = snapshot(&baseline);

    assert_eq!(after.health, expected.health);
    assert_eq!(after.burning, expected.burning);
    assert_eq!(after.cooldowns, expected.cooldowns);
    assert_eq!(after.heat, expected.heat);
    assert_eq!(after.energy, expected.energy);
    assert_eq!(after.total_damage, 0);
}
