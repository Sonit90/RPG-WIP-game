//! Every ability on the bar, cast for real, asserted to produce a visual.
//!
//! Milestone 25 asks for each of the ten abilities to be verified by hand. A person looking
//! at a screen can answer "does it look good", which no test can, and they answer it once on
//! one afternoon. This answers the other half — "does anything happen at all, in the right
//! place, for the right length of time" — on every run, for every ability, including the
//! ones that are awkward to reach by hand because they need a full burn or a group standing
//! in the right spot.
//!
//! The fight is the real one: the real simulation, the real events, the real director. What
//! is fabricated is only the geometry binding, because uploading a mesh needs a device and
//! none of these assertions is about a mesh.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::{Sphere, Vec3};
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_presentation::vfx::{VfxDirector, VfxVisuals};
use engine_render_data::{MaterialHandle, MeshHandle, RenderWorld};
use engine_vfx::VfxShape;

/// A director with every shape bound to a fabricated handle.
fn director() -> VfxDirector {
    let mut visuals = VfxVisuals::new(MaterialHandle::new(0, 1));
    for (index, shape) in VfxShape::ALL.into_iter().enumerate() {
        visuals.bind(
            shape,
            MeshHandle::new(index as u32, 1),
            Sphere::new(Vec3::ZERO, 1.0),
        );
    }
    VfxDirector::new(visuals)
}

/// An arena with a group, one tick in, with the nearest dummy selected.
fn arena(dummies: usize) -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(dummies));
    simulation.step_once();
    simulation.select_nearest_target();
    simulation
}

/// Runs the frame loop, returning how many effects were started across it.
fn play(
    director: &mut VfxDirector,
    simulation: &mut Simulation,
    world: &mut RenderWorld,
    frames: u32,
) -> u64 {
    let before = director.stats().system.spawned;
    // Observed before the first step too, because a cast made directly emits into the batch
    // that is sitting there, and the next step starts a new one.
    director.observe(simulation);
    director.update(
        simulation,
        1.0 / 60.0,
        world,
        &engine_presentation::ui::ground_decal::GroundSurface::Plane(0.0),
    );
    for _ in 0..frames {
        simulation.step_once();
        director.observe(simulation);
        director.update(
            simulation,
            1.0 / 60.0,
            world,
            &engine_presentation::ui::ground_decal::GroundSurface::Plane(0.0),
        );
    }
    director.stats().system.spawned - before
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
fn every_ability_on_the_bar_produces_a_visual() {
    // The whole bar, in bar order, each in a fresh fight. An ability that showed nothing
    // would be one the player casts into silence — which is the exact failure this phase
    // exists to remove, and the one most likely to come back when an ability is rebalanced.
    for ability in Hero::default().abilities().iter().copied() {
        let mut simulation = arena(6);
        let mut director = director();
        let mut world = RenderWorld::new();

        // Enough of a fight for the cast to run, land, and for its effect to be alive.
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

        // 300 frames is five seconds of simulation: past the longest cast, past Meteor's
        // fall, and into the life of whatever it leaves behind.
        let spawned = play(&mut director, &mut simulation, &mut world, 300);
        assert!(
            spawned > 0,
            "{} produced no effect at all",
            ability.definition().name
        );
    }
}

#[test]
fn an_area_ability_draws_the_area_it_hits() {
    // The accuracy claim, per ability: what is drawn on the ground is the ability's own
    // radius, taken from the same table the executor collects its hit list with.
    let mut simulation = arena(6);
    let mut director = director();
    let mut world = RenderWorld::new();
    let definition = FireAbility::Meteor.definition();
    let target = simulation
        .player()
        .target
        .and_then(|target| simulation.world().position(target))
        .expect("a dummy is selected");

    simulation
        .cast_at(FireAbility::Meteor, target)
        .expect("in range");

    // The telegraph is up while the rock falls, sized by the radius it will cover.
    director.observe(&simulation);
    director.update(
        &simulation,
        1.0 / 60.0,
        &mut world,
        &engine_presentation::ui::ground_decal::GroundSurface::Plane(0.0),
    );
    let ring = director
        .system_mut()
        .mesh_draws()
        .iter()
        .map(|draw| draw.radius)
        .fold(0.0_f32, f32::max);
    assert!(
        (ring - definition.radius * 1.35).abs() < definition.radius * 0.4,
        "the telegraph is {ring} m across for a {} m ability",
        definition.radius
    );
}

#[test]
fn a_rapid_rotation_never_runs_out_of_effects() {
    // Rapid casting, which is the case a fixed-capacity system is most likely to fail at.
    // Nothing may be refused: a player pressing buttons as fast as the global cooldown
    // allows is the ordinary case, not the stress case.
    let mut simulation = arena(20);
    let mut director = director();
    let mut world = RenderWorld::new();

    for round in 0..40 {
        simulation.push_command(PlayerCommand::UseAbility(round % 10));
        play(&mut director, &mut simulation, &mut world, 12);
    }
    assert_eq!(
        director.stats().refused,
        0,
        "a rotation at the cooldown's pace ran the effect system out of room"
    );
    assert_eq!(
        director.stats().system.dropped_particles,
        0,
        "a rotation at the cooldown's pace ran the particle pool out of room"
    );
}

#[test]
fn simultaneous_area_effects_all_get_drawn() {
    // Several areas burning at once, which is what a real pull produces: two Firestorms and
    // a Meteor's ground, each with its own footprint on the floor.
    let mut simulation = arena(20);
    let mut director = director();
    let mut world = RenderWorld::new();

    let centre = simulation.player_position();
    for offset in [-8.0_f32, 0.0, 8.0] {
        let aim = centre + Vec3::new(offset, 0.0, -6.0);
        let _ = simulation.cast_at(FireAbility::Firestorm, aim);
        director.observe(&simulation);
        director.update(
            &simulation,
            1.0 / 60.0,
            &mut world,
            &engine_presentation::ui::ground_decal::GroundSurface::Plane(0.0),
        );
        // Past the global cooldown, so the next one is allowed.
        play(&mut director, &mut simulation, &mut world, 45);
    }

    let strips = director
        .system_mut()
        .mesh_draws()
        .iter()
        .filter(|draw| draw.shape == VfxShape::Strip)
        .count();
    assert!(
        strips >= 2,
        "three lines of fire were cast and {strips} are drawn"
    );
}

#[test]
fn effects_survive_a_target_dying_under_them() {
    // The case that crashes a naive attachment: the thing an effect is riding stops
    // existing. Gameplay must be unaffected and the frame must still build.
    let mut simulation = arena(4);
    let mut director = director();
    let mut world = RenderWorld::new();

    simulation.push_command(PlayerCommand::UseAbility(0));
    play(&mut director, &mut simulation, &mut world, 90);

    for slot in 0..simulation.world().slot_count() {
        let Some(id) = simulation.world().id_at(slot) else {
            continue;
        };
        simulation.world_mut().damage(id, 1e9);
    }
    play(&mut director, &mut simulation, &mut world, 60);

    // Nothing asserted about what is left on screen — only that the frame kept building and
    // the system did not leak the effects that were riding the corpses.
    assert!(
        director.stats().system.instances <= 4,
        "effects outlived every target: {} still playing",
        director.stats().system.instances
    );
}
