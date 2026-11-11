//! The FireMage's Ultimate, Moving Fire Beam, asserted against a real arena.
//!
//! What a unit test of `firemage::beam` cannot see: the cost and the cooldown, a channel that
//! ticks twenty-four times on a dummy, the turn rate against a moving aim, who stands inside
//! the rectangle, every way the channel can be broken, the movement slow, a client that
//! presents the channel without ever dealing damage, and a replay that is bit-for-bit the
//! same.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use std::f32::consts::PI;

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::entity::EntityId;
use engine_game::event::{CombatEvent, ControlKind};
use engine_game::firemage::{
    AbilityError, BEAM_COOLDOWN, BEAM_DURATION, BEAM_TICK_COEFFICIENT, BEAM_TICKS, BEAM_TURN_RATE,
    BeamEnd, FireAbility,
};
use engine_game::sim::Simulation;
use engine_game::world::EntityDesc;

const HUGE: f32 = 1.0e9;

/// The aim every test starts the beam with: dead ahead, ten metres out.
const AHEAD: Vec3 = Vec3::new(0.0, 0.0, -10.0);

/// An arena whose lone dummy stands at `first` and which also holds a dummy at each of
/// `others`. Every dummy has a billion health, so none of them dies mid-channel.
fn arena_with(first: Vec3, others: &[Vec3]) -> (Simulation, EntityId, Vec<EntityId>) {
    let config = ArenaConfig {
        single_dummy: first,
        dummy_health: HUGE,
        ..ArenaConfig::with_group(0)
    };
    let mut simulation = arena::training_arena(&config);
    let lone = arena::dummies(simulation.world())
        .next()
        .expect("the arena has its lone dummy");
    let rest = others
        .iter()
        .map(|position| {
            simulation
                .world_mut()
                .spawn(EntityDesc::dummy(*position, HUGE))
        })
        .collect();
    simulation.step_once();
    (simulation, lone, rest)
}

/// One dummy dead ahead of the mage.
fn one_dummy() -> (Simulation, EntityId) {
    let (simulation, lone, _) = arena_with(Vec3::new(0.0, 0.0, -10.0), &[]);
    (simulation, lone)
}

fn full_bar(simulation: &mut Simulation) {
    simulation.mage_mut().gain_energy(100.0);
}

fn run(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

/// Sixty ticks a second.
fn seconds(simulation: &mut Simulation, seconds: f32) -> Vec<CombatEvent> {
    run(simulation, (seconds * 60.0).round() as u32)
}

fn beam_hits(events: &[CombatEvent], target: EntityId) -> usize {
    events
        .iter()
        .filter(|event| {
            event.as_damage().is_some_and(|damage| {
                damage.target == target && damage.ability.fire() == Some(FireAbility::FireBeam)
            })
        })
        .count()
}

fn ended(events: &[CombatEvent]) -> Vec<BeamEnd> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::BeamChannelEnded { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect()
}

fn started(events: &[CombatEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, CombatEvent::BeamChannelStarted { .. }))
        .count()
}

/// Starts a channel at `AHEAD` with a full bar and lets the global cooldown pass.
fn channel(simulation: &mut Simulation) -> Vec<CombatEvent> {
    full_bar(simulation);
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
    simulation.step_once();
    let mut events = simulation.events().to_vec();
    assert!(simulation.is_channelling_beam());
    events.extend(seconds(simulation, 1.6));
    events
}

#[test]
fn the_beam_needs_a_full_bar_and_spends_all_of_it() {
    let (mut simulation, _) = one_dummy();
    simulation.mage_mut().gain_energy(99.0);
    assert_eq!(
        simulation.cast_at(FireAbility::FireBeam, AHEAD),
        Err(AbilityError::NotEnoughEnergy)
    );
    assert!(!simulation.is_channelling_beam());

    simulation.mage_mut().gain_energy(1.0);
    assert!(simulation.mage().energy.is_full());
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
    assert!(simulation.is_channelling_beam());
    assert_eq!(simulation.mage().energy.value(), 0.0, "all of it, at once");
}

#[test]
fn the_beam_has_a_ninety_second_cooldown_and_a_global_cooldown_at_the_start() {
    let (mut simulation, _) = one_dummy();
    full_bar(&mut simulation);
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
    assert!(simulation.mage().global_cooldown.is_active());

    // Let it run out, refill the bar and try again: the cooldown, not the bar, refuses.
    seconds(&mut simulation, BEAM_DURATION + 0.5);
    assert!(!simulation.is_channelling_beam());
    full_bar(&mut simulation);
    assert_eq!(
        simulation.cast_at(FireAbility::FireBeam, AHEAD),
        Err(AbilityError::OnCooldown)
    );

    // Ninety seconds from the cast, give or take the half second already spent.
    seconds(&mut simulation, BEAM_COOLDOWN - BEAM_DURATION - 0.5 - 1.0);
    full_bar(&mut simulation);
    assert_eq!(
        simulation.cast_at(FireAbility::FireBeam, AHEAD),
        Err(AbilityError::OnCooldown),
        "ready a second early"
    );
    seconds(&mut simulation, 1.5);
    full_bar(&mut simulation);
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
}

#[test]
fn haste_does_not_shorten_the_ninety_seconds() {
    let (mut simulation, _) = one_dummy();
    simulation.mage_mut().mastery_buff.start(f32::MAX);
    full_bar(&mut simulation);
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
    seconds(&mut simulation, 80.0);
    full_bar(&mut simulation);
    assert_eq!(
        simulation.cast_at(FireAbility::FireBeam, AHEAD),
        Err(AbilityError::OnCooldown)
    );
}

#[test]
fn a_channel_lasts_six_seconds_and_ticks_twenty_four_times_on_a_stationary_target() {
    let (mut simulation, dummy) = one_dummy();
    full_bar(&mut simulation);
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
    let mut events = simulation.events().to_vec();
    events.extend(seconds(&mut simulation, 7.0));

    assert_eq!(started(&events), 1);
    assert_eq!(ended(&events), [BeamEnd::Completed]);
    assert_eq!(beam_hits(&events, dummy), BEAM_TICKS as usize);

    let start = events
        .iter()
        .find_map(|event| match event {
            CombatEvent::BeamChannelStarted { tick, duration, .. } => Some((*tick, *duration)),
            _ => None,
        })
        .expect("a start");
    let end_tick = events
        .iter()
        .find_map(|event| match event {
            CombatEvent::BeamChannelEnded { tick, .. } => Some(*tick),
            _ => None,
        })
        .expect("an end");
    assert_eq!(start.1, BEAM_DURATION);
    assert!(
        (end_tick as i64 - start.0 as i64 - 360).abs() <= 2,
        "{} ticks between the start and the end",
        end_tick - start.0
    );
}

#[test]
fn every_tick_is_announced_as_a_tick_of_the_right_ability_and_type() {
    let (mut simulation, dummy) = one_dummy();
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    let events = seconds(&mut simulation, 7.0);
    let ticks: Vec<_> = events
        .iter()
        .filter_map(CombatEvent::as_damage)
        .filter(|damage| {
            damage.target == dummy && damage.ability.fire() == Some(FireAbility::FireBeam)
        })
        .collect();
    assert_eq!(ticks.len(), BEAM_TICKS as usize);
    assert!(ticks.iter().all(|damage| damage.periodic));
    assert!(
        ticks
            .iter()
            .all(|damage| damage.damage_type == engine_game::combat::DamageType::Fire)
    );
    // At base spell power a non-crit tick is the coefficient times a hundred.
    let plain = ticks
        .iter()
        .filter(|damage| !damage.critical)
        .map(|damage| damage.amount)
        .fold(f32::MAX, f32::min);
    assert!(
        (plain - BEAM_TICK_COEFFICIENT * 100.0).abs() < 1.0,
        "a plain tick dealt {plain}"
    );
}

#[test]
fn the_beam_is_worth_about_three_and_a_half_pyroblasts_on_one_target() {
    let pyroblast = FireAbility::Pyroblast.definition().coefficient;
    let beam = BEAM_TICK_COEFFICIENT * BEAM_TICKS as f32;
    let ratio = beam / pyroblast;
    assert!((3.4..=3.6).contains(&ratio), "{ratio} pyroblasts");
}

#[test]
fn the_beam_adds_a_stack_of_ignite_every_second() {
    let (mut simulation, dummy) = one_dummy();
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    seconds(&mut simulation, 0.9);
    assert_eq!(simulation.mage().ignite.stacks(dummy), 0);
    seconds(&mut simulation, 0.2);
    assert_eq!(simulation.mage().ignite.stacks(dummy), 1);
    seconds(&mut simulation, 2.0);
    assert_eq!(simulation.mage().ignite.stacks(dummy), 3);
    seconds(&mut simulation, 3.0);
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        5,
        "one a second up to the cap, which the beam never passes"
    );
}

#[test]
fn the_beam_turns_no_faster_than_seventy_five_degrees_a_second() {
    let (mut simulation, _) = one_dummy();
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    simulation.step_once();
    assert!(simulation.beam().yaw().abs() < 1e-3, "born pointing ahead");

    // The cursor swings to directly behind the mage.
    simulation.push_command(PlayerCommand::ChannelAim {
        position: Vec3::new(0.0, 0.0, 10.0),
    });
    seconds(&mut simulation, 1.0);
    let turned = simulation.beam().yaw().abs();
    assert!(
        (turned - BEAM_TURN_RATE).abs() < 0.03,
        "{turned} rad after a second, expected {BEAM_TURN_RATE}"
    );
    seconds(&mut simulation, 1.0);
    assert!(
        simulation.beam().yaw().abs() < PI - 0.1,
        "arrived after two seconds"
    );
    seconds(&mut simulation, 0.5);
    assert!(
        (simulation.beam().yaw().abs() - PI).abs() < 0.03,
        "2.4 s to turn round, got {}",
        simulation.beam().yaw()
    );
}

#[test]
fn the_beam_is_born_pointing_at_the_aim_whichever_way_the_mage_faces() {
    let (mut simulation, _) = one_dummy();
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, Vec3::new(10.0, 0.0, 0.0))
        .expect("cast");
    let yaw = simulation.beam().yaw();
    assert!(
        (yaw - (-PI / 2.0)).abs() < 1e-3,
        "a quarter turn to the right is yaw {yaw}"
    );
}

#[test]
fn only_what_is_inside_the_rectangle_is_hit() {
    let inside = Vec3::new(0.0, 0.0, -10.0);
    let (mut simulation, lone, others) = arena_with(
        inside,
        &[
            Vec3::new(1.0, 0.0, -20.0),  // inside, near the end
            Vec3::new(0.0, 0.0, -25.0),  // past the 22 m
            Vec3::new(4.0, 0.0, -10.0),  // beside it
            Vec3::new(0.0, 0.0, 5.0),    // behind the mage
            Vec3::new(-3.5, 0.0, -10.0), // beside it, the other way
        ],
    );
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    let events = seconds(&mut simulation, 7.0);
    assert_eq!(beam_hits(&events, lone), BEAM_TICKS as usize);
    assert_eq!(beam_hits(&events, others[0]), BEAM_TICKS as usize);
    for (index, name) in [
        (1, "past the end"),
        (2, "beside it"),
        (3, "behind the mage"),
        (4, "beside it on the left"),
    ] {
        assert_eq!(beam_hits(&events, others[index]), 0, "{name} was hit");
    }
}

#[test]
fn a_sweep_hits_what_the_beam_passes_over_and_only_while_it_is_over_it() {
    // A dummy ahead and one off to the right. The beam holds ahead for a second, then the
    // cursor swings ninety degrees to the right, which the beam takes 1.2 s to follow.
    let (mut simulation, ahead, others) =
        arena_with(Vec3::new(0.0, 0.0, -12.0), &[Vec3::new(12.0, 0.0, 0.0)]);
    let right = others[0];
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    let mut events = simulation.events().to_vec();
    events.extend(seconds(&mut simulation, 1.0));
    simulation.push_command(PlayerCommand::ChannelAim {
        position: Vec3::new(20.0, 0.0, 0.0),
    });
    events.extend(seconds(&mut simulation, 6.0));
    let at_ahead = beam_hits(&events, ahead);
    let at_right = beam_hits(&events, right);
    assert!((3..=8).contains(&at_ahead), "{at_ahead} ticks ahead");
    assert!(
        (10..=20).contains(&at_right),
        "{at_right} ticks to the right"
    );
}

#[test]
fn a_stun_breaks_the_channel_and_the_ticks_stop() {
    let (mut simulation, dummy) = one_dummy();
    let mut events = channel(&mut simulation);
    assert!(simulation.break_channel(ControlKind::Stun));
    // Read before the next step, which starts a new batch.
    events.extend_from_slice(simulation.events());
    events.extend(seconds(&mut simulation, 4.0));
    assert_eq!(ended(&events), [BeamEnd::Stunned]);
    assert!(!simulation.is_channelling_beam());
    let hits = beam_hits(&events, dummy);
    assert!(hits > 0 && hits < BEAM_TICKS as usize, "{hits} ticks");
    assert!(
        !simulation.break_channel(ControlKind::Stun),
        "nothing left to break"
    );
}

#[test]
fn a_silence_breaks_it_and_a_taunt_does_not() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    assert!(!simulation.break_channel(ControlKind::Taunt));
    assert!(simulation.is_channelling_beam());
    assert!(simulation.break_channel(ControlKind::Silence));
    assert!(!simulation.is_channelling_beam());
    assert_eq!(ended(simulation.events()), [BeamEnd::Silenced]);
}

#[test]
fn death_ends_the_channel() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    simulation.kill_player();
    let events = seconds(&mut simulation, 0.5);
    assert_eq!(ended(&events), [BeamEnd::Died]);
    assert!(!simulation.is_channelling_beam());
}

#[test]
fn casting_something_else_breaks_it_after_the_global_cooldown() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    simulation.select_nearest_target();
    // The global cooldown has passed (channel() waited it out).
    assert_eq!(simulation.cast(FireAbility::Firebolt), Ok(0));
    assert!(!simulation.is_channelling_beam());
    let events = simulation.events().to_vec();
    assert_eq!(ended(&events), [BeamEnd::Interrupted]);
    // And the Firebolt really was started.
    assert!(simulation.casting().is_some());
}

#[test]
fn a_refused_press_does_not_cost_the_mage_their_beam() {
    let (mut simulation, _) = one_dummy();
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    seconds(&mut simulation, 0.2);
    // Inside the global cooldown: refused, and the channel carries on.
    simulation.select_nearest_target();
    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(AbilityError::GlobalCooldown)
    );
    assert!(simulation.is_channelling_beam());
}

#[test]
fn blink_is_refused_while_channelling() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    let before = simulation.player_position();
    assert_eq!(
        simulation.cast(FireAbility::Blink),
        Err(AbilityError::Channelling)
    );
    assert!(simulation.is_channelling_beam());
    seconds(&mut simulation, 0.1);
    assert!((simulation.player_position() - before).length() < 0.1);
}

#[test]
fn pressing_the_ultimate_again_ends_it_early_and_the_cooldown_stays() {
    let (mut simulation, dummy) = one_dummy();
    let mut events = channel(&mut simulation);
    assert_eq!(simulation.cast_at(FireAbility::FireBeam, AHEAD), Ok(0));
    events.extend_from_slice(simulation.events());
    events.extend(seconds(&mut simulation, 5.0));
    assert_eq!(ended(&events), [BeamEnd::Cancelled]);
    assert!(!simulation.is_channelling_beam());
    assert!(beam_hits(&events, dummy) < BEAM_TICKS as usize);
    assert!(!simulation.mage().abilities.is_ready(FireAbility::FireBeam));
    assert!(
        simulation.mage().energy.value() < 5.0,
        "cancelling refunded the bar"
    );
}

#[test]
fn the_mage_walks_at_forty_per_cent_while_channelling_and_cannot_jump() {
    let walk = PlayerCommand::Move {
        forward: 1.0,
        right: 0.0,
        yaw: 0.0,
        face_camera: false,
    };
    // The distance covered in two seconds of walking, with and without the beam.
    let distance = |channelling: bool| {
        let (mut simulation, _) = one_dummy();
        if channelling {
            full_bar(&mut simulation);
            simulation
                .cast_at(FireAbility::FireBeam, Vec3::new(0.0, 0.0, 10.0))
                .expect("cast");
        }
        let start = simulation.player_position();
        simulation.push_command(walk);
        seconds(&mut simulation, 2.0);
        (simulation.player_position() - start).length()
    };
    let free = distance(false);
    let slowed = distance(true);
    let ratio = slowed / free;
    assert!(free > 5.0, "the control run barely moved: {free}");
    assert!(
        (0.3..=0.5).contains(&ratio),
        "walked {slowed} m channelling against {free} m free: {ratio}"
    );

    // And a jump does nothing.
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    simulation.push_command(PlayerCommand::Jump);
    let events = seconds(&mut simulation, 0.5);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::Jumped { .. })),
        "jumped out of the beam"
    );
}

#[test]
fn the_beam_cannot_be_steered_when_there_is_no_channel() {
    let (mut simulation, _) = one_dummy();
    simulation.push_command(PlayerCommand::ChannelAim {
        position: Vec3::new(5.0, 0.0, 5.0),
    });
    seconds(&mut simulation, 0.1);
    assert!(!simulation.is_channelling_beam());
    assert!(simulation.beam().aim().is_none());
}

#[test]
fn a_non_finite_aim_is_ignored() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    let before = simulation.beam().aim();
    simulation.push_command(PlayerCommand::ChannelAim {
        position: Vec3::new(f32::NAN, 0.0, 0.0),
    });
    seconds(&mut simulation, 0.1);
    assert_eq!(simulation.beam().aim(), before);
}

#[test]
fn steering_reports_are_sent_at_most_ten_times_a_second() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    let mut reports = 0;
    for step in 0..120 {
        // A cursor that never stops moving.
        simulation.push_command(PlayerCommand::ChannelAim {
            position: Vec3::new((step as f32 * 0.15).sin() * 12.0, 0.0, -10.0),
        });
        simulation.step_once();
        reports += simulation
            .events()
            .iter()
            .filter(|event| matches!(event, CombatEvent::BeamYaw { .. }))
            .count();
    }
    assert!(
        reports > 5,
        "{reports} reports: the steering was never reported"
    );
    assert!(reports <= 20, "{reports} reports in two seconds");
}

#[test]
fn a_hero_swap_or_a_reset_ends_the_channel_with_an_announcement() {
    let (mut simulation, _) = one_dummy();
    channel(&mut simulation);
    simulation.reset_dummies();
    assert!(!simulation.is_channelling_beam());
    assert_eq!(ended(simulation.events()), [BeamEnd::Reset]);
}

#[test]
fn a_client_presents_the_channel_and_deals_no_damage_of_its_own() {
    let (mut simulation, dummy) = one_dummy();
    simulation.set_remote_authority(true);
    let player = simulation.player().entity;
    let before = simulation
        .world()
        .health(dummy)
        .expect("the dummy has health")
        .current();

    // The server says the channel began.
    simulation.inject_event(CombatEvent::BeamChannelStarted {
        entity: player,
        yaw: 0.0,
        start_time: 0.0,
        duration: BEAM_DURATION,
        tick: 0,
    });
    assert!(simulation.is_channelling_beam());
    // Slowed the same way the server will slow it, so the prediction agrees.
    assert!((simulation.channel_move_multiplier() - 0.4).abs() < 1e-6);

    // The player steers: the press is the server's to decide, and the client turns its
    // picture towards it without exceeding the rate.
    simulation.push_command(PlayerCommand::ChannelAim {
        position: Vec3::new(0.0, 0.0, 10.0),
    });
    let requests = simulation.take_remote_requests();
    assert!(
        requests
            .iter()
            .any(|request| matches!(request, PlayerCommand::ChannelAim { .. })),
        "the aim never reached the server"
    );
    let events = seconds(&mut simulation, 1.0);
    let turned = simulation.beam().yaw().abs();
    assert!((turned - BEAM_TURN_RATE).abs() < 0.03, "{turned}");

    seconds(&mut simulation, 6.0);
    let health = simulation
        .world()
        .health(dummy)
        .expect("the dummy has health")
        .current();
    assert_eq!(health, before, "a client dealt beam damage");
    assert!(
        !events.iter().any(|event| event.as_damage().is_some()),
        "a client announced damage"
    );

    // A report from the authority corrects a prediction that is badly wrong, and keeps one
    // that is close.
    simulation.inject_event(CombatEvent::BeamChannelStarted {
        entity: player,
        yaw: 0.0,
        start_time: 0.0,
        duration: BEAM_DURATION,
        tick: 0,
    });
    simulation.inject_event(CombatEvent::BeamYaw {
        entity: player,
        yaw: 0.05,
        turn_target_yaw: 0.05,
        time: 0.0,
        tick: 0,
    });
    assert_eq!(
        simulation.beam().yaw(),
        0.0,
        "snapped for a 3 degree difference"
    );
    simulation.inject_event(CombatEvent::BeamYaw {
        entity: player,
        yaw: 1.0,
        turn_target_yaw: 1.0,
        time: 0.0,
        tick: 0,
    });
    assert_eq!(simulation.beam().yaw(), 1.0);

    // And its ending takes the picture down.
    simulation.inject_event(CombatEvent::BeamChannelEnded {
        entity: player,
        reason: BeamEnd::Completed,
        tick: 0,
    });
    assert!(!simulation.is_channelling_beam());
}

#[test]
fn a_lost_ending_does_not_leave_a_client_channelling_for_ever() {
    let (mut simulation, _) = one_dummy();
    simulation.set_remote_authority(true);
    let player = simulation.player().entity;
    simulation.inject_event(CombatEvent::BeamChannelStarted {
        entity: player,
        yaw: 0.0,
        start_time: 0.0,
        duration: BEAM_DURATION,
        tick: 0,
    });
    seconds(&mut simulation, BEAM_DURATION + 1.5);
    assert!(!simulation.is_channelling_beam());
}

#[test]
fn a_replay_of_the_same_inputs_is_the_same_fight() {
    let play = || {
        let (mut simulation, lone, others) = arena_with(
            Vec3::new(0.0, 0.0, -12.0),
            &[Vec3::new(8.0, 0.0, -8.0), Vec3::new(-6.0, 0.0, -14.0)],
        );
        full_bar(&mut simulation);
        simulation
            .cast_at(FireAbility::FireBeam, AHEAD)
            .expect("cast");
        let mut events = simulation.events().to_vec();
        for step in 0..420 {
            if step % 6 == 0 {
                simulation.push_command(PlayerCommand::ChannelAim {
                    position: Vec3::new((step as f32 * 0.02).sin() * 14.0, 0.0, -10.0),
                });
            }
            simulation.step_once();
            events.extend_from_slice(simulation.events());
        }
        let health: Vec<f32> = std::iter::once(lone)
            .chain(others)
            .map(|id| simulation.world().health(id).map_or(0.0, |h| h.current()))
            .collect();
        (format!("{events:?}"), health)
    };
    let (first_events, first_health) = play();
    let (second_events, second_health) = play();
    assert_eq!(first_events, second_events);
    assert_eq!(first_health, second_health);
    assert!(first_health.iter().any(|health| *health < HUGE));
}

#[test]
fn eight_enemies_in_the_beam_all_take_every_tick() {
    let line: Vec<Vec3> = (1..8)
        .map(|index| Vec3::new(0.0, 0.0, -10.0 - index as f32 * 1.4))
        .collect();
    let (mut simulation, lone, others) = arena_with(Vec3::new(0.0, 0.0, -10.0), &line);
    full_bar(&mut simulation);
    simulation
        .cast_at(FireAbility::FireBeam, AHEAD)
        .expect("cast");
    let events = seconds(&mut simulation, 7.0);
    for target in std::iter::once(lone).chain(others) {
        assert_eq!(beam_hits(&events, target), BEAM_TICKS as usize);
    }
}
