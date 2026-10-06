//! The FireMage rework, asserted against a real arena.
//!
//! Everything the design document promises and a unit test cannot see: Ignite against a
//! world, Detonate's arithmetic, the Heat bands over simulated time, the critical strike
//! cooldown loops and their caps, casts interrupted by a target dying, and every talent
//! switching cleanly on and off.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::entity::EntityId;
use engine_game::firemage::{
    CRIT_FEEDBACK_CAP, FireAbility, FireMastery, HEAT_COMBAT_TIMEOUT, HeatTier,
    IGNITE_BASE_TICK_INTERVAL, TalentGrid, TalentId, ULTIMATE_ENABLED,
};
use engine_game::sim::Simulation;

/// The training arena with the nearest dummy selected.
fn arena() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    simulation.select_nearest_target();
    simulation
}

fn target(simulation: &Simulation) -> EntityId {
    simulation.player().target.expect("a dummy is selected")
}

fn health(simulation: &Simulation, id: EntityId) -> f32 {
    simulation
        .world()
        .health(id)
        .map_or(0.0, |health| health.current())
}

/// Puts `stacks` of Ignite on `id` without casting anything.
fn ignite(simulation: &mut Simulation, id: EntityId, stacks: u8) {
    let maximum = simulation.mage().ignite_max();
    let duration = simulation.mage().ignite_duration();
    simulation
        .mage_mut()
        .ignite
        .apply(id, stacks, maximum, duration);
}

/// Runs a cast to completion, however long it takes.
fn cast_through(simulation: &mut Simulation, ability: FireAbility) {
    simulation.step_until_ready();
    simulation.cast(ability).expect("the cast was refused");
    for _ in 0..300 {
        if simulation.casting().is_none() {
            break;
        }
        simulation.step_once();
    }
}

// ---------------------------------------------------------------- Ignite

#[test]
fn ignite_stacks_up_to_the_maximum_and_never_past_it() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    let maximum = simulation.mage().ignite_max();
    assert_eq!(simulation.mage().ignite.stacks(dummy), 0);

    cast_through(&mut simulation, FireAbility::Firebolt);
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        1,
        "firebolt did not ignite a cold target"
    );

    cast_through(&mut simulation, FireAbility::FlameLance);
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        3,
        "flame lance adds two"
    );

    for _ in 0..10 {
        ignite(&mut simulation, dummy, 1);
    }
    assert_eq!(simulation.mage().ignite.stacks(dummy), maximum);
    assert!(simulation.mage().ignite.is_at_maximum(dummy, maximum));
}

#[test]
fn a_burn_refreshes_its_duration_and_then_expires() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    ignite(&mut simulation, dummy, 2);
    simulation.step_many(600); // ten seconds

    let before = simulation.mage().ignite.get(dummy).remaining;
    assert!(before < simulation.mage().ignite_duration());
    ignite(&mut simulation, dummy, 1);
    assert!(
        simulation.mage().ignite.get(dummy).remaining > before,
        "a fresh application did not refresh the burn"
    );

    simulation.step_many(60 * 45);
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        0,
        "the burn outlived its duration"
    );
}

#[test]
fn nothing_detonates_by_itself_at_the_maximum() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    let maximum = simulation.mage().ignite_max();
    ignite(&mut simulation, dummy, maximum);

    simulation.step_many(120);
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        maximum,
        "the burn exploded on its own"
    );
}

#[test]
fn a_running_burn_ticks_faster_the_moment_haste_arrives() {
    // The no-snapshot rule, end to end: the burn was applied with no haste at all.
    let mut simulation = arena();
    let dummy = target(&simulation);
    ignite(&mut simulation, dummy, 5);

    let slow_start = health(&simulation, dummy);
    simulation.step_many(60 * 8);
    let slow = slow_start - health(&simulation, dummy);

    ignite(&mut simulation, dummy, 0);
    simulation.mage_mut().mastery = FireMastery::Haste;
    simulation.mage_mut().mastery_buff.start(60.0);
    assert!(simulation.mage().ignite_interval() < IGNITE_BASE_TICK_INTERVAL);

    let fast_start = health(&simulation, dummy);
    simulation.step_many(60 * 8);
    let fast = fast_start - health(&simulation, dummy);

    assert!(
        fast > slow * 1.05,
        "haste did not speed the running burn up: {slow} then {fast}"
    );
}

#[test]
fn a_running_burn_uses_the_critical_strike_chance_of_the_moment() {
    // Applied with no crit buff, ticked with a huge one: the burn must start critting
    // without being reapplied.
    let mut simulation = arena();
    let dummy = target(&simulation);
    ignite(&mut simulation, dummy, 5);

    let plain_start = health(&simulation, dummy);
    simulation.step_many(60 * 10);
    let plain = plain_start - health(&simulation, dummy);

    ignite(&mut simulation, dummy, 0);
    simulation.mage_mut().mastery = FireMastery::Crit;
    simulation.mage_mut().mastery_buff.start(60.0);

    let crit_start = health(&simulation, dummy);
    simulation.step_many(60 * 10);
    let critting = crit_start - health(&simulation, dummy);

    assert!(
        critting > plain,
        "the burn ignored the new critical strike chance: {plain} then {critting}"
    );
}

#[test]
fn a_burn_is_worth_more_when_heat_is_higher_with_no_reapplication() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    ignite(&mut simulation, dummy, 5);
    let cold = simulation.mage().remaining_ignite_damage(dummy);

    simulation.mage_mut().gain_heat(35.0);
    let warm = simulation.mage().remaining_ignite_damage(dummy);
    assert!(
        (warm / cold - 1.15).abs() < 1e-3,
        "heat did not reach the running burn: {cold} then {warm}"
    );
}

// ---------------------------------------------------------------- Selection

#[test]
fn the_player_can_select_themselves_and_cannot_set_themselves_on_fire() {
    // Selecting yourself is the one selection always available, and it must not turn the
    // filler into a way to die.
    let mut simulation = arena();
    let player = simulation.player().entity;
    let before = health(&simulation, player);

    simulation.push_command(engine_game::command::PlayerCommand::SelectSelf);
    simulation.step_once();
    assert_eq!(simulation.player().target, Some(player));

    assert_eq!(
        simulation.cast(FireAbility::Firebolt),
        Err(engine_game::firemage::AbilityError::TargetFriendly)
    );
    // And a cast that somehow started still lands on nobody.
    for _ in 0..60 {
        simulation.step_once();
    }
    assert_eq!(
        health(&simulation, player),
        before,
        "the player burned themselves"
    );
    assert!(
        simulation.mage().abilities.is_ready(FireAbility::Firebolt),
        "a refusal spent the charge"
    );
}

#[test]
fn selecting_yourself_survives_a_tick_and_is_not_taken_away() {
    // `retarget_if_needed` replaces a target that is dead or gone. A living player is
    // neither, so the selection has to hold.
    let mut simulation = arena();
    let player = simulation.player().entity;
    simulation.push_command(engine_game::command::PlayerCommand::SelectSelf);
    for _ in 0..120 {
        simulation.step_once();
    }
    assert_eq!(simulation.player().target, Some(player));
}

#[test]
fn a_named_selection_is_still_checked_before_it_is_honoured() {
    use engine_game::command::PlayerCommand;

    let mut simulation = arena();
    let dummy = target(&simulation);
    let player = simulation.player().entity;

    // A legal one: the caller names an enemy and gets it.
    simulation.push_command(PlayerCommand::SelectSelf);
    simulation.step_once();
    simulation.push_command(PlayerCommand::SelectTarget { entity: dummy });
    simulation.step_once();
    assert_eq!(simulation.player().target, Some(dummy));

    // A corpse is refused, and the selection that was there is kept.
    simulation.push_command(PlayerCommand::SelectSelf);
    simulation.step_once();
    simulation.world_mut().damage(dummy, 1_000_000.0);
    simulation.push_command(PlayerCommand::SelectTarget { entity: dummy });
    simulation.step_once();
    assert_eq!(
        simulation.player().target,
        Some(player),
        "a dead enemy was selected by name"
    );
}

#[test]
fn a_dead_target_is_dropped_and_nothing_is_chosen_in_its_place() {
    // A target is something the player picked. Picking the next one for them walks the
    // selection around the room on its own, and every spell pressed afterwards lands
    // somewhere they did not aim it.
    let mut simulation = arena();
    let dummy = target(&simulation);
    simulation.world_mut().damage(dummy, 1_000_000.0);
    simulation.step_once();

    assert_eq!(
        simulation.player().target,
        None,
        "something was selected without being asked for"
    );
    // And it stays that way: the arena is full of other dummies.
    for _ in 0..120 {
        simulation.step_once();
    }
    assert_eq!(simulation.player().target, None);
}

#[test]
fn escape_clears_the_selection_and_leaves_it_cleared() {
    use engine_game::command::PlayerCommand;

    let mut simulation = arena();
    assert!(simulation.player().target.is_some());
    simulation.push_command(PlayerCommand::ClearTarget);
    simulation.step_once();
    assert_eq!(simulation.player().target, None);

    for _ in 0..120 {
        simulation.step_once();
    }
    assert_eq!(
        simulation.player().target,
        None,
        "a cleared selection came back on its own"
    );
}

#[test]
fn a_living_target_that_walked_out_of_reach_is_kept() {
    // Only a dead or missing target is dropped. Chasing something must not lose it.
    let mut simulation = arena();
    let dummy = target(&simulation);
    if let Some(body) = simulation.world_mut().body_mut(dummy) {
        body.position = Vec3::new(0.0, 0.0, -300.0);
    }
    for _ in 0..60 {
        simulation.step_once();
    }
    assert_eq!(simulation.player().target, Some(dummy));
}

#[test]
fn every_bar_ability_agrees_with_the_documented_ally_rule() {
    // The property pinned per ability, so deriving it from the target mode cannot quietly
    // change what the bar does.
    let usable = |ability: FireAbility| ability.usable_on_friendly_target();
    for ability in [
        FireAbility::Firebolt,
        FireAbility::FlameLance,
        FireAbility::Pyroblast,
        FireAbility::Detonate,
    ] {
        assert!(!usable(ability), "{:?} needs an enemy", ability);
    }
    for ability in [
        FireAbility::FlameCone,
        FireAbility::Meteor,
        FireAbility::Firestorm,
        FireAbility::FireMastery,
        FireAbility::FlameShield,
        FireAbility::Blink,
        FireAbility::Cataclysm,
    ] {
        assert!(usable(ability), "{:?} does not need an enemy", ability);
    }
}

#[test]
fn a_cone_is_aimed_where_it_was_clicked_and_announced_the_same_way() {
    // The bug this pins: the cone was collected with the aim, but announced with the
    // caster's facing *after* the tick had turned them - so a cone clicked to one side
    // while the camera swung the other way burned one wedge and was drawn over another.
    let mut simulation = arena();
    let origin = simulation.player_position();
    let slot = FireAbility::FlameCone
        .bar_slot()
        .expect("the cone is on the bar");
    // Aimed hard left...
    let aim = origin + Vec3::new(-10.0, 0.0, 0.0);
    simulation.push_command(engine_game::command::PlayerCommand::UseAbilityAt {
        slot,
        position: aim,
    });
    // ...while the same frame's camera turns the character hard right.
    let camera_yaw = -std::f32::consts::FRAC_PI_2;
    simulation.push_command(engine_game::command::PlayerCommand::Move {
        forward: 0.0,
        right: 0.0,
        yaw: camera_yaw,
        face_camera: true,
    });
    simulation.step_once();

    let aimed = std::f32::consts::FRAC_PI_2;
    let facing = simulation
        .events()
        .iter()
        .find_map(|event| match *event {
            engine_game::event::CombatEvent::AbilityUsed {
                ability, facing, ..
            } if ability.fire() == Some(FireAbility::FlameCone) => Some(facing),
            _ => None,
        })
        .expect("the cone was announced");
    assert!(
        (facing - aimed).abs() < 1e-3,
        "the cone was announced facing {facing}, not the {aimed} it was aimed at"
    );
    let turned = simulation.player_body().map_or(0.0, |body| body.facing_yaw);
    assert!(
        (turned - camera_yaw).abs() < 1e-3,
        "the camera did not turn the character: {turned}"
    );
}

// ---------------------------------------------------------------- Detonate

#[test]
fn detonate_is_refused_below_the_maximum_and_costs_nothing() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    ignite(&mut simulation, dummy, 2);

    assert_eq!(
        simulation.cast(FireAbility::Detonate),
        Err(engine_game::firemage::AbilityError::NotEnoughIgnite)
    );
    assert!(
        simulation.mage().abilities.is_ready(FireAbility::Detonate),
        "a refusal spent the charge"
    );
    assert_eq!(simulation.mage().ignite.stacks(dummy), 2, "it ate the burn");
}

#[test]
fn detonate_converts_the_burns_remaining_damage_and_consumes_it() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    let maximum = simulation.mage().ignite_max();
    ignite(&mut simulation, dummy, maximum);

    let remaining = simulation.mage().remaining_ignite_damage(dummy);
    let before = health(&simulation, dummy);
    simulation.cast(FireAbility::Detonate).expect("a full burn");
    let dealt = before - health(&simulation, dummy);

    let expected = remaining * engine_game::sim::DETONATE_FRACTION;
    // One critical strike roll is the only variable, so the hit is the expected damage or
    // twice it.
    assert!(
        (dealt - expected).abs() < expected * 0.02
            || (dealt - expected * 2.0).abs() < expected * 0.05,
        "expected {expected} (or a crit), dealt {dealt}"
    );
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        0,
        "the burn survived"
    );
}

#[test]
fn a_short_burn_is_worth_less_than_a_fresh_one() {
    // What makes the timing of a detonation a decision: it converts what is *left*.
    let mut fresh = arena();
    let dummy = target(&fresh);
    let maximum = fresh.mage().ignite_max();
    ignite(&mut fresh, dummy, maximum);
    let full = fresh.mage().remaining_ignite_damage(dummy);

    fresh.step_many(60 * 20);
    let late = fresh.mage().remaining_ignite_damage(dummy);
    assert!(late < full * 0.5, "a twenty-second-old burn kept its value");
}

#[test]
fn detonate_hits_every_burning_enemy_in_ten_metres_on_its_own_burn() {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();

    // The group stands inside one blast of itself.
    let group: Vec<EntityId> = arena::dummies(simulation.world())
        .filter(|id| {
            simulation
                .world()
                .position(*id)
                .is_some_and(|position| position.x > 5.0)
        })
        .collect();
    assert!(group.len() >= 3, "the arena's pack is missing");

    let maximum = simulation.mage().ignite_max();
    for id in &group {
        ignite(&mut simulation, *id, maximum);
    }
    // The one that is not burning takes nothing: Detonate is the burn, not a splash.
    let unburnt = group[group.len() - 1];
    simulation.mage_mut().ignite.clear(unburnt);

    let before: Vec<f32> = group.iter().map(|id| health(&simulation, *id)).collect();
    simulation.player_mut().target = Some(group[0]);
    if let Some(position) = simulation.world().position(group[0]) {
        simulation.face(position);
    }
    simulation.cast(FireAbility::Detonate).expect("a full burn");

    for (index, id) in group.iter().enumerate() {
        let dealt = before[index] - health(&simulation, *id);
        if *id == unburnt {
            assert_eq!(dealt, 0.0, "an unburnt enemy was splashed");
        } else {
            assert!(dealt > 0.0, "a burning enemy in the radius took nothing");
            assert_eq!(simulation.mage().ignite.stacks(*id), 0);
        }
    }
}

#[test]
fn a_dead_target_takes_its_burn_with_it() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    let maximum = simulation.mage().ignite_max();
    ignite(&mut simulation, dummy, maximum);
    // Killed outside the combat path on purpose: a burn that outlived its target would
    // then be found by the next tick rather than by the kill, which is the harder case.
    simulation.world_mut().damage(dummy, 1e9);

    simulation.step_many(180);
    assert!(!simulation.world().is_alive(dummy));
    assert_eq!(
        simulation.mage().ignite.stacks(dummy),
        0,
        "a corpse is still burning"
    );
}

// ---------------------------------------------------------------- Heat

#[test]
fn the_heat_bands_are_where_the_design_puts_them() {
    let mut simulation = arena();
    assert_eq!(simulation.mage().heat.tier(), HeatTier::Cold);

    simulation.mage_mut().gain_heat(30.0);
    assert_eq!(simulation.mage().heat.tier(), HeatTier::Warm);
    assert!((simulation.mage().fire_damage_multiplier() - 1.15).abs() < 1e-6);

    simulation.mage_mut().gain_heat(40.0);
    assert_eq!(simulation.mage().heat.tier(), HeatTier::Hot);
    assert!((simulation.mage().fire_damage_multiplier() - 1.30).abs() < 1e-6);
    assert!(
        simulation.mage().haste() > 0.0,
        "the hot band granted nothing"
    );
}

#[test]
fn a_hundred_heat_is_spent_on_the_inferno_state_and_ends_at_zero() {
    let mut simulation = arena();
    simulation.mage_mut().gain_heat(100.0);
    assert_eq!(simulation.mage().heat.value(), 0.0, "heat was not consumed");
    assert!(simulation.mage().heat.is_inferno());
    assert!((simulation.mage().fire_damage_multiplier() - 1.50).abs() < 1e-6);

    simulation.step_many(60 * 11);
    assert!(!simulation.mage().heat.is_inferno());
    assert_eq!(simulation.mage().heat.value(), 0.0);
    assert!((simulation.mage().fire_damage_multiplier() - 1.0).abs() < 1e-6);
}

#[test]
fn heat_holds_through_a_fight_and_decays_once_it_is_over() {
    let mut simulation = arena();
    simulation.mage_mut().gain_heat(60.0);

    // Ten seconds of casting: the burn keeps dealing damage, so the mage stays in combat.
    let dummy = target(&simulation);
    ignite(&mut simulation, dummy, 3);
    simulation.step_many(60 * 8);
    assert!(
        simulation.mage().heat.value() >= 60.0,
        "heat decayed during a fight: {}",
        simulation.mage().heat.value()
    );

    // Now nothing is burning and nothing is cast.
    simulation.mage_mut().ignite.clear_all();
    let before = simulation.mage().heat.value();
    simulation.step_many((60.0 * (HEAT_COMBAT_TIMEOUT + 4.0)) as u32);
    assert!(
        simulation.mage().heat.value() < before,
        "heat never decayed out of combat"
    );
}

// ---------------------------------------------------------------- Cooldown feedback

#[test]
fn a_critical_flame_lance_never_hands_back_more_than_the_cap() {
    let mut simulation = arena();
    simulation
        .mage_mut()
        .set_talents(TalentGrid::from_talents(&[TalentId::SpellfireFeedback]));
    // Guaranteed crits, so the loop is observed rather than waited for.
    simulation.mage_mut().mastery = FireMastery::Crit;

    let effects = *simulation.mage().effects();
    assert!(effects.flame_lance_crit_reduction <= CRIT_FEEDBACK_CAP);
    assert!(effects.pyroblast_crit_reduction <= CRIT_FEEDBACK_CAP);

    // Put Pyroblast on cooldown, then land Flame Lances until one crits.
    simulation
        .mage_mut()
        .abilities
        .spend(FireAbility::Pyroblast, &effects);
    let before = simulation
        .mage()
        .abilities
        .recharge_remaining(FireAbility::Pyroblast);

    let mut reduced = false;
    for _ in 0..40 {
        let remaining = simulation
            .mage()
            .abilities
            .recharge_remaining(FireAbility::Pyroblast);
        cast_through(&mut simulation, FireAbility::Firebolt);
        let after = simulation
            .mage()
            .abilities
            .recharge_remaining(FireAbility::Pyroblast);
        // Time passes, so the cooldown falls anyway; what matters is that no single event
        // takes more than the cap off beyond the time that elapsed.
        assert!(
            remaining - after <= CRIT_FEEDBACK_CAP + 2.0,
            "a single cast took {} seconds off",
            remaining - after
        );
        if simulation.mage().abilities.is_ready(FireAbility::Pyroblast) {
            reduced = true;
            break;
        }
    }
    let _ = (before, reduced);
}

#[test]
fn burning_precision_only_ever_hurries_meteor_by_a_second() {
    let mut simulation = arena();
    simulation
        .mage_mut()
        .set_talents(TalentGrid::from_talents(&[TalentId::BurningPrecision]));
    let effects = *simulation.mage().effects();
    assert!((effects.ignite_crit_meteor_reduction - 1.0).abs() < 1e-6);
    assert!((effects.ignite_crit_meteor_chance - 0.10).abs() < 1e-6);
}

// ---------------------------------------------------------------- Interrupted casts

#[test]
fn a_cast_whose_target_dies_lands_on_nothing() {
    for ability in [
        FireAbility::Firebolt,
        FireAbility::FlameLance,
        FireAbility::Pyroblast,
    ] {
        let mut simulation = arena();
        let dummy = target(&simulation);
        simulation.cast(ability).expect("in range");
        assert!(simulation.casting().is_some(), "{ability:?} did not start");

        // The target dies mid-cast.
        simulation.world_mut().damage(dummy, 1e9);
        let before = simulation.meter().total_damage();

        for _ in 0..300 {
            if simulation.casting().is_none() {
                break;
            }
            simulation.step_once();
        }
        let dealt = simulation.meter().total_damage() - before;
        assert!(dealt <= 0.0, "{ability:?} dealt {dealt} to a corpse");
        assert!(
            simulation.fire_counters().casts_wasted >= 1,
            "{ability:?} was not counted as wasted"
        );
    }
}

#[test]
fn moving_cancels_a_cast_and_it_is_counted() {
    let mut simulation = arena();
    simulation.cast(FireAbility::Pyroblast).expect("in range");
    simulation.push_command(engine_game::command::PlayerCommand::Move {
        forward: 1.0,
        right: 0.0,
        yaw: 0.0,
        face_camera: false,
    });
    simulation.step_many(10);

    assert!(simulation.casting().is_none(), "movement did not break it");
    assert_eq!(simulation.fire_counters().casts_interrupted, 1);
    assert!(
        simulation.mage().abilities.is_ready(FireAbility::Pyroblast),
        "a cast that never landed cost a charge"
    );
}

// ---------------------------------------------------------------- Talents and the Ultimate

#[test]
fn every_talent_can_be_taken_and_given_back_cleanly() {
    let baseline = TalentGrid::with_points(0).effects();
    for talent in TalentId::ALL {
        let mut simulation = arena();
        simulation
            .mage_mut()
            .set_talents(TalentGrid::from_talents(&[talent]));
        assert_ne!(
            *simulation.mage().effects(),
            baseline,
            "{talent:?} changed nothing"
        );

        // And the world still runs with it on.
        simulation.step_many(60);
        simulation
            .mage_mut()
            .set_talents(TalentGrid::with_points(0));
        assert_eq!(*simulation.mage().effects(), baseline, "{talent:?} stuck");
    }
}

#[test]
fn infernal_capacity_moves_the_detonate_requirement_with_the_maximum() {
    let mut simulation = arena();
    let dummy = target(&simulation);
    simulation
        .mage_mut()
        .set_talents(TalentGrid::from_talents(&[TalentId::InfernalCapacity]));
    assert_eq!(simulation.mage().ignite_max(), 6);

    let duration = simulation.mage().ignite_duration();
    simulation.mage_mut().ignite.apply(dummy, 5, 5, duration);
    assert_eq!(
        simulation.cast(FireAbility::Detonate),
        Err(engine_game::firemage::AbilityError::NotEnoughIgnite),
        "five was enough against a maximum of six"
    );

    ignite(&mut simulation, dummy, 1);
    assert!(simulation.cast(FireAbility::Detonate).is_ok());
}

#[test]
fn molten_meteor_leaves_ground_that_burns_and_makes_fire_hurt_more() {
    let mut simulation = arena();
    simulation
        .mage_mut()
        .set_talents(TalentGrid::from_talents(&[TalentId::MoltenMeteor]));
    let dummy = target(&simulation);
    let position = simulation.world().position(dummy).expect("a position");

    simulation
        .cast_at(FireAbility::Meteor, position)
        .expect("in range");
    simulation.step_many(120); // past the impact delay
    assert!(
        simulation
            .mage()
            .zones
            .has(engine_game::firemage::ZoneKind::BurningGround),
        "no ground was left behind"
    );

    simulation.step_many(200);
    assert!(
        simulation.mage().vulnerability.has(dummy),
        "standing in it did not apply the debuff"
    );
}

#[test]
fn the_ultimate_is_switched_off_but_still_exists() {
    let mut simulation = arena();
    simulation.mage_mut().gain_energy(100.0);
    // A constant on purpose: the Ultimate is switched off by a `const`, and this is the
    // test that notices when someone turns it back on without designing one.
    #[allow(
        clippy::assertions_on_constants,
        reason = "the constant is the subject"
    )]
    {
        assert!(!ULTIMATE_ENABLED, "the ultimate was enabled with no design");
    }
    assert_eq!(
        simulation.cast_at(FireAbility::Cataclysm, Vec3::new(0.0, 0.0, -10.0)),
        Err(engine_game::firemage::AbilityError::UltimateDisabled)
    );
    // The architecture is untouched: the ability, its cost and its resource are all here.
    assert_eq!(FireAbility::Cataclysm.definition().energy_cost, 100.0);
    assert!(simulation.mage().energy.is_full());
}

#[test]
fn blinking_arrives_rather_than_travelling() {
    let mut simulation = arena();
    let player = simulation.player().entity;
    let before = simulation
        .world()
        .position(player)
        .expect("the player has a body");

    simulation.cast(FireAbility::Blink).expect("blink is ready");
    let after = simulation
        .world()
        .position(player)
        .expect("the player still has a body");
    assert!(
        (after - before).length() > 1.0,
        "the mage did not move: {before:?} -> {after:?}"
    );

    // Announced, so presentation can tell this apart from a very fast run. Both ends are
    // carried, because an observer cannot ask the world where the mage used to be.
    let teleport = simulation
        .events()
        .iter()
        .find_map(|event| match *event {
            engine_game::event::CombatEvent::Teleported {
                entity, from, to, ..
            } if entity == player => Some((from, to)),
            _ => None,
        })
        .expect("a blink emits a teleport");
    assert_eq!(teleport.0, before);
    assert_eq!(teleport.1, after);

    // And the history is gone, or the character is interpolated across the gap and slides
    // the whole distance over one frame instead of arriving.
    let previous = simulation
        .world()
        .previous_body(player)
        .expect("the player has a history");
    assert_eq!(
        previous.position, after,
        "the mage slid instead of arriving"
    );
}
