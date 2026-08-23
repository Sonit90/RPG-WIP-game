//! Milestone 21: the whole gameplay pipeline runs with no renderer.
//!
//! ```text
//! Input -> Command -> Simulation -> Ability -> Damage -> CombatEvent -> Telemetry
//! ```
//!
//! No D3D12 device, no swap chain, no window, no GPU, no message pump. This file links
//! `engine-game` and `engine-core` and nothing else — if a future change makes the
//! simulation depend on the renderer, this test stops compiling, which is the point of
//! having it as a separate integration target rather than as a unit test.
//!
//! It is also the reason the simulation can be tested at all: a gameplay test that needed a
//! GPU would be a GPU test, and would skip on a machine without an adapter.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::time::Duration;

use engine_core::input::{InputEvent, InputState, KeyCode};
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::event::CombatEvent;
use engine_game::input_map::Bindings;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;

/// Applies one frame of key presses the way the engine loop does.
fn key_frame(input: &mut InputState, down: &[KeyCode]) {
    input.begin_frame();
    for key in down {
        input.apply(InputEvent::KeyDown {
            key: *key,
            repeat: false,
        });
    }
}

/// Feeds one frame of input through the bindings into the simulation.
fn feed(
    simulation: &mut Simulation,
    input: &InputState,
    yaw: f32,
    commands: &mut Vec<PlayerCommand>,
) {
    commands.clear();
    Bindings::default().write_commands(engine_game::Hero::default(), input, yaw, commands);
    simulation.push_commands(commands.iter().copied());
}

#[test]
fn a_keypress_becomes_damage_without_any_graphics_being_created() {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    let mut input = InputState::new();
    let mut commands = Vec::new();

    simulation.step_once();
    let target = simulation
        .player()
        .target
        .expect("the arena starts with a dummy targeted");
    let before = simulation
        .world()
        .health(target)
        .expect("the dummy exists")
        .current();

    // The `1` key: the whole chain from a platform-shaped event to a health change.
    // Firebolt is a one-second cast, so the frame is run out rather than stepped once.
    key_frame(&mut input, &[KeyCode::Digit1]);
    feed(&mut simulation, &input, 0.0, &mut commands);
    simulation.step_many(70);

    let after = simulation
        .world()
        .health(target)
        .expect("the dummy exists")
        .current();
    assert!(after < before, "no damage was dealt: {before} -> {after}");

    let events = simulation.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CombatEvent::AbilityUsed { .. })),
        "the ability use was not announced"
    );
    let damage: f32 = events
        .iter()
        .filter_map(CombatEvent::as_damage)
        .map(|damage| damage.amount)
        .sum();
    assert!((damage - (before - after)).abs() < 1e-3);
    assert!(
        simulation.meter().total_damage() > 0.0,
        "telemetry observed it"
    );
}

#[test]
fn movement_and_jumping_run_headlessly() {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    let mut input = InputState::new();
    let mut commands = Vec::new();
    let start = simulation.player_position();

    key_frame(&mut input, &[KeyCode::W, KeyCode::Space]);
    feed(&mut simulation, &input, 0.0, &mut commands);
    simulation.step_once();
    assert!(
        !simulation
            .player_body()
            .expect("the player exists")
            .grounded,
        "the jump did not leave the ground"
    );

    // Keep running forward for a second of simulated time.
    key_frame(&mut input, &[KeyCode::W]);
    for _ in 0..60 {
        feed(&mut simulation, &input, 0.0, &mut commands);
        simulation.step_once();
    }

    let body = simulation.player_body().expect("the player exists");
    assert!(body.position.z < start.z - 3.0, "{start:?} -> {body:?}");
    assert!(body.grounded, "the character never landed");
    // Not exactly zero any more. Collision resolution keeps the capsule a skin width
    // outside every surface it rests on, because a character that settles *exactly* on a
    // face alternates between touching it and being one ulp inside it — which is a visible
    // shudder. Five millimetres is that skin.
    assert!(
        body.position.y >= 0.0 && body.position.y < 0.02,
        "landed at {}",
        body.position.y
    );
}

#[test]
fn the_full_rotation_runs_and_the_meter_agrees_with_the_health_lost() {
    // Firebolt, Ignite, the cone, then let the burn run: the whole Phase 3 combat feature
    // set in one headless run, with the meter cross-checked against the world.
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();

    let dummies: Vec<_> = arena::dummies(simulation.world()).collect();
    let before: f32 = dummies
        .iter()
        .filter_map(|id| simulation.world().health(*id))
        .map(engine_game::combat::Health::current)
        .sum();

    // Firebolt, Flame Lance and Flame Cone: a cast, a cast with charges, and an instant.
    for slot in [0, 1, 2] {
        simulation.push_command(PlayerCommand::UseAbility(slot));
        simulation.step_many(120);
    }
    simulation.step_many(600); // ten seconds: the burns run out

    let after: f32 = dummies
        .iter()
        .filter_map(|id| simulation.world().health(*id))
        .map(engine_game::combat::Health::current)
        .sum();
    let lost = f64::from(before - after);

    assert!(lost > 0.0, "the rotation dealt nothing");
    // A relative tolerance, not an absolute one: dummies hold 200,000 health and an f32
    // that large resolves to about a hundredth, so summing their bars cannot agree with the
    // meter's f64 total to the last decimal.
    assert!(
        (simulation.meter().total_damage() - lost).abs() <= lost * 1e-4 + 1e-2,
        "the meter says {} and the dummies lost {lost}",
        simulation.meter().total_damage()
    );
    assert!(simulation.meter().dps() > 0.0);
    for slot in [0, 1, 2] {
        let ability = engine_game::ability::AbilityId::from_slot(slot).expect("the slot exists");
        assert!(
            simulation.meter().ability(ability).casts >= 1,
            "slot {slot} was not credited with its cast"
        );
    }
}

#[test]
fn the_synergy_is_observable_from_the_event_stream_alone() {
    // What a future VFX or network layer would see: Ignite announced, stacked to the
    // maximum, and then spent by a detonation that hits harder than its own base damage.
    use engine_game::firemage::FireAbility;

    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    simulation.select_nearest_target();
    let target = simulation.player().target.expect("a dummy is selected");

    simulation.push_command(PlayerCommand::UseAbility(0));
    simulation.step_many(70);
    assert!(
        simulation.events().iter().any(|event| matches!(
            event,
            CombatEvent::StatusApplied {
                status: StatusKind::Burning,
                ..
            }
        )),
        "ignite did not announce the burn"
    );
    assert!(
        simulation.mage().ignite.stacks(target) >= 1,
        "firebolt did not ignite an unburnt target"
    );

    // Reaching the maximum must not detonate anything by itself.
    let maximum = simulation.mage().ignite_max();
    let duration = simulation.mage().ignite_duration();
    simulation
        .mage_mut()
        .ignite
        .apply(target, maximum, maximum, duration);
    simulation.step_many(60);
    assert_eq!(
        simulation.mage().ignite.stacks(target),
        maximum,
        "the burn exploded on its own"
    );

    let before = simulation
        .world()
        .health(target)
        .expect("the dummy exists")
        .current();
    simulation
        .cast(FireAbility::Detonate)
        .expect("a full burn is what it needs");
    let after = simulation
        .world()
        .health(target)
        .expect("the dummy exists")
        .current();

    assert_eq!(
        simulation.mage().ignite.stacks(target),
        0,
        "the detonation left the burn behind"
    );
    // Detonate is worth what the burn had left, so the explosion is measured against the
    // burn rather than against a flat number the ability no longer has.
    assert!(
        before - after > 0.0,
        "the detonation dealt nothing: {}",
        before - after
    );
}

#[test]
fn a_thousand_dummies_simulate_without_a_device() {
    // The Milestone 24 benchmark scenario, as a correctness test: the simulation has to
    // survive the scale before it is worth measuring.
    let config = ArenaConfig::with_group(1_000);
    let mut simulation = arena::training_arena(&config);
    // The player, a thousand dummies, the lone one, and the friendly one in the main hall.
    assert_eq!(simulation.world().len(), 1_003);

    for _ in 0..60 {
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.advance(Duration::from_micros(16_667));
    }
    assert!(simulation.tick_count() >= 55);
    assert_eq!(simulation.world().len(), 1_003, "nothing was lost");
}

#[test]
fn the_dummy_reset_returns_the_arena_to_its_starting_state() {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();

    let dummies: Vec<_> = arena::dummies(simulation.world()).collect();
    let starting: Vec<_> = dummies
        .iter()
        .filter_map(|id| simulation.world().position(*id))
        .collect();

    for slot in [0, 1, 2] {
        simulation.push_command(PlayerCommand::UseAbility(slot));
        simulation.step_many(2);
    }
    simulation.step_many(120);
    assert!(simulation.meter().total_damage() > 0.0);

    simulation.push_command(PlayerCommand::ResetDummies);
    simulation.step_once();

    assert_eq!(simulation.meter().total_damage(), 0.0);
    assert_eq!(simulation.meter().dps(), 0.0);
    for (id, position) in dummies.iter().zip(&starting) {
        let health = simulation.world().health(*id).expect("the dummy exists");
        assert_eq!(
            health.current(),
            health.max(),
            "{id:?} is not at full health"
        );
        assert_eq!(simulation.world().position(*id).as_ref(), Some(position));
        assert!(
            simulation
                .world()
                .statuses(*id)
                .expect("the dummy exists")
                .is_empty(),
            "{id:?} is still burning"
        );
    }
}
