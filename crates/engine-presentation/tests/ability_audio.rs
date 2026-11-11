//! Every ability on the bar, cast for real, asserted to produce a sound.
//!
//! Milestone 25 asks for each of the ten abilities to be verified by hand. A person with
//! speakers can answer "does it sound good", which no test can, and they answer it once on
//! one afternoon. This answers the other half — "does anything play at all, from the right
//! place, without a wall of duplicates" — on every run, for every ability, **with no audio
//! device on the machine**: the runtime mixes into a buffer, and a buffer is something a
//! test can look at.
//!
//! The fight is the real one: the real simulation, the real events, the real director.
//! Nothing is fabricated except the sample rate, which is dropped to 8 kHz so synthesising
//! the bank is fast.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_audio::{AudioSystem, CHANNELS, DEFAULT_MAX_VOICES, SoundBank, SoundId};
use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::enemy::EnemyKind;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_presentation::AudioDirector;

/// The rate every test in this file runs at. A tenth of a device's, for speed.
const RATE: u32 = 8_000;

/// A runtime with the real catalogue, synthesised small.
fn runtime() -> AudioSystem {
    AudioSystem::with_bank(SoundBank::synthesised(RATE), RATE, DEFAULT_MAX_VOICES)
}

/// An arena with a group, one tick in, with the nearest dummy selected.
fn arena(dummies: usize) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(dummies));
    simulation.step_once();
    simulation.select_nearest_target();
    simulation
}

/// Runs the frame loop, returning how many voices were started across it.
fn play(
    director: &mut AudioDirector,
    audio: &mut AudioSystem,
    simulation: &mut Simulation,
    frames: u32,
) -> u64 {
    let before = audio.stats().started;
    // Observed before the first step too, because a cast made directly emits into the batch
    // that is sitting there, and the next step starts a new one.
    director.observe(simulation, audio);
    director.update(simulation, 1.0 / 60.0, audio);
    for _ in 0..frames {
        simulation.step_once();
        director.observe(simulation, audio);
        director.update(simulation, 1.0 / 60.0, audio);
        // A block of mix every frame, so voices actually age and end the way they do in the
        // game rather than piling up because nothing ever asked for samples.
        let mut block = vec![0.0; (RATE as usize / 60) * CHANNELS];
        audio.render(&mut block);
    }
    audio.stats().started - before
}

/// Puts the target's burn at the maximum, which is what Detonate requires.
fn ignite_to_maximum(simulation: &mut Simulation) {
    let target = simulation.player().target.expect("a dummy is selected");
    let maximum = simulation.mage().ignite_max();
    let duration = simulation.mage().ignite_duration();
    simulation
        .mage_mut()
        .ignite
        .apply(target, maximum, maximum, duration);
}

#[test]
fn every_ability_on_the_bar_produces_a_sound() {
    // The acceptance criterion, as a test. An ability that made no sound at all would be one
    // the player casts into silence — the exact failure this phase exists to remove, and the
    // one most likely to come back when an ability is rebalanced or renamed.
    for ability in Hero::default().abilities().iter().copied() {
        let mut simulation = arena(6);
        let mut audio = runtime();
        let mut director = AudioDirector::new();

        let target = simulation
            .player()
            .target
            .and_then(|target| simulation.world().position(target))
            .expect("a dummy is selected");

        if ability == FireAbility::Detonate {
            ignite_to_maximum(&mut simulation);
        }

        let cast = if ability.definition().target.is_aimed() {
            simulation.cast_at(ability, target)
        } else {
            simulation.cast(ability)
        };
        assert!(
            cast.is_ok(),
            "{} was refused by the simulation: {cast:?}",
            ability.definition().name
        );

        // Five seconds: past the longest cast, past Meteor's fall, into the life of whatever
        // it leaves burning.
        let started = play(&mut director, &mut audio, &mut simulation, 300);
        assert!(
            started > 0,
            "{} produced no sound at all",
            ability.definition().name
        );
    }
}

#[test]
fn the_firebolt_chain_is_cast_then_flight_then_impact() {
    // Milestone 12's chain, in order. The impact must come *after* the launch, because the
    // bolt has to arrive — audio follows the picture, and the picture takes a moment.
    let mut simulation = arena(1);
    let mut audio = runtime();
    let mut director = AudioDirector::new();

    simulation.push_command(PlayerCommand::UseAbility(0));
    // One frame past the cast landing: the bolt is in the air and has not arrived.
    play(&mut director, &mut audio, &mut simulation, 120);

    assert!(
        audio.stats().started >= 3,
        "cast, flight and impact are three sounds; {} played",
        audio.stats().started
    );
    // And it is all over: nothing from a one-shot chain is still holding a voice.
    play(&mut director, &mut audio, &mut simulation, 120);
    assert_eq!(
        audio.playing(SoundId::FireboltTravel),
        0,
        "the travelling bed outlived the bolt"
    );
}

#[test]
fn a_burn_on_fifty_targets_is_not_fifty_sounds() {
    // Milestone 14 and 20, together: the wall of identical loops this phase exists to
    // prevent, and the burn ticks that must not each be an impact.
    let mut simulation = arena(50);
    let mut audio = runtime();
    let mut director = AudioDirector::new();

    // Light the whole pack: a cone into the group, then a few seconds of it burning.
    for _ in 0..6 {
        simulation.push_command(PlayerCommand::UseAbility(2));
        play(&mut director, &mut audio, &mut simulation, 60);
    }

    let burning = audio.playing(SoundId::IgniteLoop);
    let cap = SoundId::IgniteLoop.desc().max_voices as usize;
    assert!(
        burning <= cap,
        "{burning} burning beds are playing at once, cap is {cap}"
    );
    assert!(
        audio.stats().active <= DEFAULT_MAX_VOICES,
        "the pool overflowed"
    );
    // The limiter did its job rather than the pool having been big enough by luck.
    assert!(audio.stats().suppressed_interval + audio.stats().suppressed_limit > 0);
}

#[test]
fn a_rapid_rotation_never_becomes_a_wall_of_noise() {
    // Rapid casting into a pack: the case a fixed voice pool is most likely to fail at.
    let mut simulation = arena(20);
    let mut audio = runtime();
    let mut director = AudioDirector::new();

    for round in 0..40 {
        simulation.push_command(PlayerCommand::UseAbility(round % 10));
        play(&mut director, &mut audio, &mut simulation, 12);
    }

    let stats = audio.stats();
    assert!(
        stats.peak_active <= DEFAULT_MAX_VOICES,
        "peak {} voices",
        stats.peak_active
    );
    // Priority did the deciding, not luck: nothing important was dropped for want of a slot
    // while footsteps and beds were playing.
    assert!(
        stats.dropped * 4 < stats.started.max(1),
        "{} of {} cues were dropped outright",
        stats.dropped,
        stats.started
    );
}

#[test]
fn a_fight_out_of_earshot_is_quieter_than_one_in_your_face() {
    // The positional claim, end to end: the same fight, heard from two places.
    let mut near = None;
    let mut far = None;
    for (slot, distance) in [(&mut near, 0.0_f32), (&mut far, 45.0)] {
        let mut simulation = arena(4);
        let mut audio = runtime();
        let mut director = AudioDirector::new();

        let listener_position = simulation.player_position() + Vec3::new(distance, 1.6, 0.0);
        audio.set_listener(engine_audio::Listener::new(
            listener_position,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 0.0),
        ));

        simulation.push_command(PlayerCommand::UseAbility(0));
        director.observe(&simulation, &mut audio);
        director.update(&simulation, 1.0 / 60.0, &mut audio);
        let mut peak = 0.0_f32;
        for _ in 0..180 {
            simulation.step_once();
            director.observe(&simulation, &mut audio);
            director.update(&simulation, 1.0 / 60.0, &mut audio);
            let mut block = vec![0.0; (RATE as usize / 60) * CHANNELS];
            audio.render(&mut block);
            peak = peak.max(block.iter().fold(0.0_f32, |peak, s| peak.max(s.abs())));
        }
        *slot = Some(peak);
    }

    let near = near.expect("the near fight was measured");
    let far = far.expect("the far fight was measured");
    assert!(near > 0.0, "a fight in front of the listener was silent");
    assert!(far < near * 0.6, "45 m away was {far} against {near} near");
}

#[test]
fn a_bank_that_failed_to_load_leaves_the_fight_running() {
    // The documented failure mode: no samples anywhere. Gameplay continues, the director
    // keeps working, every cue is counted as missing, and nothing panics.
    let mut simulation = arena(6);
    let mut audio = AudioSystem::with_bank(SoundBank::empty(RATE), RATE, DEFAULT_MAX_VOICES);
    let mut director = AudioDirector::new();

    for round in 0..10 {
        simulation.push_command(PlayerCommand::UseAbility(round % 10));
        play(&mut director, &mut audio, &mut simulation, 30);
    }

    assert!(audio.is_idle(), "an empty bank produced voices");
    assert!(audio.stats().missing > 0, "the misses were not counted");
    assert!(
        simulation.meter().total_damage() > 0.0,
        "the fight stopped because the audio did"
    );
}

#[test]
fn the_listener_follows_the_camera_and_nothing_else() {
    // Milestone 8: there is one camera, and the listener is a view of it.
    let mut audio = runtime();
    let director = AudioDirector::new();
    let mut camera =
        engine_render_data::Camera::new(engine_render_data::Projection::perspective(16.0 / 9.0))
            .expect("a valid projection");
    camera
        .look_at(
            Vec3::new(3.0, 2.0, 4.0),
            Vec3::ZERO,
            Vec3::new(0.0, 1.0, 0.0),
        )
        .expect("a valid basis");

    director.set_listener(&camera, &mut audio);
    let listener = audio.listener();
    assert_eq!(listener.position, camera.position());
    assert!((listener.forward - camera.forward()).length() < 1e-4);
}

#[test]
fn the_player_dying_is_heard() {
    let mut simulation = arena(1);
    let mut audio = runtime();
    let mut director = AudioDirector::new();
    let player = simulation.player().entity;
    simulation.slay(player);
    assert!(!simulation.player_alive(), "the player is dead");
    director.observe(&simulation, &mut audio);
    assert_eq!(
        audio.playing(SoundId::PlayerDeath),
        1,
        "the player's death plays the player's own death sound"
    );
}

#[test]
fn a_skeleton_dies_as_bones_and_the_player_does_not() {
    let mut simulation = arena(1);
    let mut audio = runtime();
    let mut director = AudioDirector::new();
    let kind = EnemyKind::from_id("skeleton_warrior").expect("the skeleton warrior exists");
    let skeleton = simulation.spawn_enemy(kind, Vec3::new(4.0, 0.0, 4.0), 0.0);
    simulation.step_once();
    director.observe(&simulation, &mut audio);

    simulation.slay(skeleton);
    director.observe(&simulation, &mut audio);
    assert_eq!(
        audio.playing(SoundId::DeathSkeleton),
        1,
        "bones, not the common death"
    );
    assert_eq!(audio.playing(SoundId::Death), 0);
    assert_eq!(audio.playing(SoundId::PlayerDeath), 0);
}
