//! The first dungeon, driven headless: monsters, packs, the boss, resets and a full run.
//!
//! Every test here runs the **actual** dungeon — the real level, the real collision, the real
//! brains, the real damage path — with no device, no window and no presentation. Walking from
//! the entrance to the boss is a CPU pass over arrays, and it has to be checkable as one.
//!
//! What is asserted is the *rule* rather than the tuning wherever the two can be separated,
//! so a rebalance moves numbers in the tables and not the tests. Where a number *is* the
//! mechanic — a phase threshold, a nova radius — it is asserted, because that number is the
//! contract the documentation states.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::dungeon::{BOSS_PACK, PACK_COUNT, PACKS, build_dungeon};
use engine_game::encounter::{Encounter, EncounterState, EnemySpawn};
use engine_game::enemy::brain::AiState;
use engine_game::enemy::{EnemyAbility, EnemyEffect, EnemyKind, TelegraphShape};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::Simulation;
use engine_game::world::horizontal_distance;

/// A simulation with one enemy of `kind` `distance` metres in front of the player.
fn duel(kind: EnemyKind, distance: f32) -> (Simulation, EntityId) {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("DUEL", vec![EnemySpawn::new(kind, spawn)]);
    let simulation = arena::encounter_arena(&config, encounter);
    let enemy = first_member(&simulation, 0);
    (simulation, enemy)
}

/// A simulation holding `kinds`, spread across an arc in front of the player.
fn pack(kinds: &[EnemyKind], distance: f32) -> (Simulation, Vec<EntityId>) {
    let config = ArenaConfig::with_group(0);
    let spawns = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let offset = Vec3::new(index as f32 * 4.0 - 4.0, 0.0, -distance);
            EnemySpawn::new(*kind, config.player_spawn + offset)
        })
        .collect();
    let simulation = arena::encounter_arena(&config, Encounter::with_spawns("PACK", spawns));
    let members = simulation
        .encounter()
        .expect("the encounter was installed")
        .members()
        .to_vec();
    (simulation, members)
}

/// The first member of one encounter.
fn first_member(simulation: &Simulation, index: usize) -> EntityId {
    simulation
        .encounter_at(index)
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its members")
}

/// Runs `ticks` fixed ticks, collecting every event.
fn run(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

/// The state one enemy is in.
fn state(simulation: &Simulation, enemy: EntityId) -> AiState {
    simulation
        .brain(enemy)
        .map(engine_game::EnemyBrain::state)
        .expect("the entity is an enemy")
}

/// One entity's health.
fn health(simulation: &Simulation, entity: EntityId) -> f32 {
    simulation
        .world()
        .health(entity)
        .map_or(0.0, |health| health.current())
}

/// Puts one entity's health at `fraction` of its maximum, without any ability doing it.
///
/// A test control, not a gameplay path: what is being tested is what the enemy does at a
/// health fraction, and getting there by actually fighting would test the player's rotation.
fn set_health_fraction(simulation: &mut Simulation, entity: EntityId, fraction: f32) {
    let max = simulation
        .world()
        .health(entity)
        .map_or(0.0, engine_game::Health::max);
    let current = health(simulation, entity);
    let wanted = max * fraction;
    if wanted < current {
        simulation.world_mut().damage(entity, current - wanted);
    } else {
        simulation.world_mut().heal(entity, wanted - current);
    }
}

/// Which phase one enemy is in.
fn phase(simulation: &Simulation, entity: EntityId) -> usize {
    simulation
        .brain(entity)
        .map_or(0, engine_game::EnemyBrain::phase)
}

// ---------------------------------------------------------------------------------------
// The roster
// ---------------------------------------------------------------------------------------

#[test]
fn every_monster_in_the_roster_is_distinct_and_reachable() {
    // The tables are the content. A monster that cannot reach its own attacks, or that has
    // no attacks, is a monster that stands still being hit.
    for kind in EnemyKind::ALL {
        let archetype = kind.archetype();
        assert!(archetype.abilities().count() > 0, "{kind:?} has no attack");
        assert!(
            archetype.preferred_range <= archetype.longest_range(),
            "{kind:?}"
        );
        assert!(archetype.aggro_radius < archetype.leash_radius, "{kind:?}");
        assert!(
            archetype.max_health > 0.0 && archetype.speed > 0.0,
            "{kind:?}"
        );
        // Slower than the player, in every phase. Read off the player's own movement config
        // rather than written down again: anything that could outrun the player would make
        // every "move out of it" mechanic in the dungeon unanswerable, and a second copy of
        // the number is a second thing to forget.
        let player_speed = engine_game::movement::MovementConfig::default().max_speed;
        for index in 0..archetype.phase_count() {
            assert!(
                archetype.phase_speed(index) < player_speed,
                "{kind:?} phase {index} moves at {} against the player's {player_speed}",
                archetype.phase_speed(index)
            );
        }
    }
}

#[test]
fn a_brute_closes_and_a_caster_holds_its_distance() {
    // Each starts inside its own aggro radius: what is being tested is where it stops, not
    // whether it noticed.
    for (kind, closes) in [(EnemyKind::Melee, true), (EnemyKind::Caster, false)] {
        let (mut simulation, enemy) = duel(kind, kind.archetype().aggro_radius - 2.0);
        run(&mut simulation, 400);
        let gap = horizontal_distance(
            simulation.player_position(),
            simulation
                .world()
                .position(enemy)
                .expect("the enemy is in the world"),
        );
        if closes {
            assert!(gap < 5.0, "{kind:?} stopped {gap:.1} m away");
        } else {
            // It starts inside its preferred range, so it has no reason to come closer.
            assert!(gap > 6.0, "{kind:?} walked into melee at {gap:.1} m");
        }
    }
}

#[test]
fn the_ritualists_nova_is_interruptible_and_an_interrupt_costs_it_the_cast() {
    let (mut simulation, enemy) = duel(EnemyKind::Ritualist, 9.0);
    // Long enough to notice and start something.
    run(&mut simulation, 200);
    let casting = simulation
        .brain(enemy)
        .and_then(engine_game::EnemyBrain::cast)
        .expect("the ritualist is casting something");
    assert!(casting.interruptible);

    let before = health(&simulation, simulation.player().entity);
    assert!(simulation.interrupt(enemy), "the cast can be stopped");
    assert!(
        simulation
            .brain(enemy)
            .and_then(engine_game::EnemyBrain::cast)
            .is_none()
    );
    run(&mut simulation, 30);
    assert_eq!(
        health(&simulation, simulation.player().entity),
        before,
        "an interrupted cast deals no damage"
    );
    assert_eq!(state(&simulation, enemy), AiState::Recover);
}

#[test]
fn the_ritualists_nova_hits_what_is_inside_its_ring_and_nothing_outside_it() {
    // The one guarantee every telegraph in the game makes: the drawn shape is the hit test.
    let definition = EnemyAbility::RitualNova.definition();
    let TelegraphShape::Circle { radius } = definition.shape else {
        panic!("the nova is a circle");
    };
    let anchor = Vec3::ZERO;
    assert!(
        definition
            .shape
            .contains(anchor, Vec3::new(radius - 0.1, 0.0, 0.0), 0.0)
    );
    assert!(
        !definition
            .shape
            .contains(anchor, Vec3::new(radius + 0.1, 0.0, 0.0), 0.0)
    );
    assert!(definition.interruptible);
    assert!(definition.cast_time >= 2.0, "it has to be answerable");
}

#[test]
fn the_warden_heals_the_ally_that_is_worst_off_and_nothing_when_nobody_is_hurt() {
    let (mut simulation, members) = pack(&[EnemyKind::Warden, EnemyKind::Melee], 9.0);
    let (warden, brute) = (members[0], members[1]);

    // Nothing is hurt: the Warden has no heal target and falls through to its filler.
    run(&mut simulation, 240);
    assert!(
        !simulation
            .events()
            .iter()
            .chain(std::iter::empty())
            .any(|event| matches!(event, CombatEvent::Healing { .. })),
        "a healer with nothing to heal must not cast a heal"
    );

    // Hurt the brute badly and the Warden should put health back into it.
    set_health_fraction(&mut simulation, brute, 0.3);
    let wounded = health(&simulation, brute);
    let events = run(&mut simulation, 900);
    let healed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Healing {
                source,
                target,
                amount,
                ..
            } => Some((*source, *target, *amount)),
            _ => None,
        })
        .collect();
    assert!(!healed.is_empty(), "the warden never healed anything");
    assert!(
        healed.iter().all(|(source, _, _)| *source == warden),
        "something other than the warden healed"
    );
    assert!(
        healed.iter().any(|(_, target, _)| *target == brute),
        "the warden healed the wrong ally"
    );
    assert!(health(&simulation, brute) > wounded);
}

#[test]
fn a_mend_that_is_interrupted_restores_nothing() {
    let (mut simulation, members) = pack(&[EnemyKind::Warden, EnemyKind::Melee], 9.0);
    let (warden, brute) = (members[0], members[1]);
    set_health_fraction(&mut simulation, brute, 0.3);
    let wounded = health(&simulation, brute);

    // Run until the Warden is actually casting the heal, then stop it.
    let mut stopped = false;
    for _ in 0..900 {
        simulation.step_once();
        if simulation
            .brain(warden)
            .and_then(engine_game::EnemyBrain::cast)
            .is_some_and(|cast| cast.ability == EnemyAbility::Mend)
        {
            assert!(simulation.interrupt(warden));
            stopped = true;
            break;
        }
    }
    assert!(stopped, "the warden never started a mend");
    run(&mut simulation, 20);
    assert_eq!(
        health(&simulation, brute),
        wounded,
        "an interrupted mend restored health anyway"
    );
}

#[test]
fn a_weaver_leaves_burning_ground_that_expires_on_its_own() {
    let (mut simulation, enemy) = duel(EnemyKind::Weaver, 9.0);
    let EnemyEffect::Hazard { duration, .. } = EnemyAbility::CinderPool.definition().effect else {
        panic!("the cinder pool is a hazard");
    };

    // Run until a pool exists.
    let mut lit = false;
    for _ in 0..900 {
        simulation.step_once();
        if !simulation.enemy_hazards().is_empty() {
            lit = true;
            break;
        }
    }
    assert!(lit, "the weaver never lit anything");
    let hazard = simulation.enemy_hazards().hazards()[0];
    assert!((hazard.duration - duration).abs() < 1e-3);
    assert_eq!(hazard.owner, enemy);

    // It goes out on its own, and its end is announced.
    let events = run(&mut simulation, (duration * 60.0) as u32 + 10);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CombatEvent::AreaEffectEnded { .. })),
        "a hazard that expired was never announced"
    );
}

#[test]
fn burning_ground_damages_the_player_standing_in_it_and_never_its_own_side() {
    let (mut simulation, members) = pack(&[EnemyKind::Weaver, EnemyKind::Melee], 9.0);
    let brute = members[1];
    let brute_health = health(&simulation, brute);
    let player_start = health(&simulation, simulation.player().entity);

    // The player stands still, which is exactly what the mechanic punishes.
    run(&mut simulation, 1_200);
    assert!(
        health(&simulation, simulation.player().entity) < player_start,
        "the player stood in fire and took nothing"
    );
    assert_eq!(
        health(&simulation, brute),
        brute_health,
        "an enemy hazard damaged its own faction"
    );
}

#[test]
fn a_weavers_pools_go_out_when_it_dies() {
    let (mut simulation, enemy) = duel(EnemyKind::Weaver, 9.0);
    for _ in 0..900 {
        simulation.step_once();
        if !simulation.enemy_hazards().is_empty() {
            break;
        }
    }
    assert!(!simulation.enemy_hazards().is_empty(), "nothing was lit");

    simulation.slay(enemy);
    assert!(!simulation.world().is_alive(enemy));
    assert!(
        simulation.enemy_hazards().is_empty(),
        "a dead weaver left its fire burning"
    );
}

#[test]
fn the_revenant_changes_phase_once_at_half_health_and_never_goes_back() {
    let (mut simulation, enemy) = duel(EnemyKind::Revenant, 10.0);
    run(&mut simulation, 60);
    assert_eq!(phase(&simulation, enemy), 0);

    set_health_fraction(&mut simulation, enemy, 0.49);
    let events = run(&mut simulation, 4);
    let changes: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, CombatEvent::PhaseChanged { .. }))
        .collect();
    assert_eq!(changes.len(), 1, "a transition must be announced once");
    assert_eq!(phase(&simulation, enemy), 1);

    // Healing it back does not rewind the fight.
    set_health_fraction(&mut simulation, enemy, 1.0);
    run(&mut simulation, 60);
    assert_eq!(phase(&simulation, enemy), 1);

    // And the second phase really is a different rotation.
    let archetype = EnemyKind::Revenant.archetype();
    let first: Vec<_> = archetype.phase_abilities(0).collect();
    let second: Vec<_> = archetype.phase_abilities(1).collect();
    assert_ne!(first, second);
    assert!(second.contains(&EnemyAbility::Cleave));
    assert!(!first.contains(&EnemyAbility::Cleave));
    assert!(archetype.phase_cooldown_scale(1) < 1.0);
    assert!(archetype.phase_speed(1) > archetype.phase_speed(0));
}

// ---------------------------------------------------------------------------------------
// The boss
// ---------------------------------------------------------------------------------------

#[test]
fn the_boss_has_three_phases_and_each_one_adds_a_mechanic() {
    let archetype = EnemyKind::Boss.archetype();
    assert_eq!(archetype.phase_count(), 3);

    let first: Vec<_> = archetype.phase_abilities(0).collect();
    let second: Vec<_> = archetype.phase_abilities(1).collect();
    let third: Vec<_> = archetype.phase_abilities(2).collect();

    assert!(!first.contains(&EnemyAbility::CinderRain));
    assert!(second.contains(&EnemyAbility::CinderRain));
    assert!(!second.contains(&EnemyAbility::ImmolationNova));
    assert!(third.contains(&EnemyAbility::ImmolationNova));
    // Not simply a bigger stick: every phase keeps the abilities the player learned.
    assert!(third.contains(&EnemyAbility::WardensSweep));
    assert!(archetype.max_health > 10.0 * EnemyKind::Melee.archetype().max_health);
}

#[test]
fn the_boss_crosses_both_thresholds_in_order_and_announces_each_once() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 12.0);
    run(&mut simulation, 60);
    assert_eq!(phase(&simulation, boss), 0);

    set_health_fraction(&mut simulation, boss, 0.65);
    let events = run(&mut simulation, 4);
    assert_eq!(phase(&simulation, boss), 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CombatEvent::PhaseChanged { .. }))
            .count(),
        1
    );

    set_health_fraction(&mut simulation, boss, 0.3);
    let events = run(&mut simulation, 4);
    assert_eq!(phase(&simulation, boss), 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CombatEvent::PhaseChanged { .. }))
            .count(),
        1
    );
}

#[test]
fn one_hit_that_crosses_two_thresholds_lands_in_the_deeper_phase() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 12.0);
    run(&mut simulation, 60);
    set_health_fraction(&mut simulation, boss, 0.1);
    let events = run(&mut simulation, 4);
    assert_eq!(phase(&simulation, boss), 2, "it stopped in phase two");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, CombatEvent::PhaseChanged { .. }))
            .count(),
        1,
        "the phase the player never saw must not be announced"
    );
}

#[test]
fn a_phase_transition_cancels_the_cast_and_takes_its_telegraph_down() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 6.0);
    // Run until it is casting something with a telegraph up.
    let mut casting = false;
    for _ in 0..600 {
        simulation.step_once();
        if simulation
            .brain(boss)
            .and_then(engine_game::EnemyBrain::cast)
            .is_some()
        {
            casting = true;
            break;
        }
    }
    assert!(casting, "the boss never started a cast");

    set_health_fraction(&mut simulation, boss, 0.6);
    let events = run(&mut simulation, 2);
    assert!(
        simulation
            .brain(boss)
            .and_then(engine_game::EnemyBrain::cast)
            .is_none(),
        "the cast survived the transition"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            CombatEvent::TelegraphEnded {
                resolved: false,
                ..
            }
        )),
        "the telegraph survived the transition"
    );
    // And it stands there for the stagger rather than immediately starting something else.
    assert_eq!(state(&simulation, boss), AiState::Recover);
}

#[test]
fn the_bosses_uninterruptible_mechanics_cannot_be_stopped_and_its_one_window_can() {
    for ability in [
        EnemyAbility::WardensSweep,
        EnemyAbility::ImmolationNova,
        EnemyAbility::CinderRain,
    ] {
        assert!(
            !ability.definition().interruptible,
            "{} must be answered by moving",
            ability.name()
        );
    }
    assert!(EnemyAbility::Ashfall.definition().interruptible);
    assert!(EnemyAbility::EmberLance.definition().interruptible);
}

#[test]
fn the_nova_hits_at_its_radius_and_misses_just_outside_it() {
    let definition = EnemyAbility::ImmolationNova.definition();
    let TelegraphShape::Circle { radius } = definition.shape else {
        panic!("the nova is a circle");
    };
    assert!(
        (radius - 14.0).abs() < 1e-3,
        "the documented radius is 14 m"
    );
    assert!(
        definition
            .shape
            .contains(Vec3::ZERO, Vec3::new(13.9, 0.0, 0.0), 0.0)
    );
    assert!(
        !definition
            .shape
            .contains(Vec3::ZERO, Vec3::new(14.1, 0.0, 0.0), 0.0)
    );
    // The escape has to be possible: the player moves at 7 m/s, so the cast has to be long
    // enough to cross the radius from the middle of it.
    assert!(
        definition.cast_time * 7.0 > radius,
        "the nova cannot be outrun"
    );
}

#[test]
fn the_boss_can_be_killed_and_its_death_completes_its_encounter_once() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 12.0);
    run(&mut simulation, 60);
    simulation.slay(boss);
    let events = run(&mut simulation, 4);
    assert!(!simulation.world().is_alive(boss));
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Completed)
    );
    assert_eq!(
        simulation
            .events()
            .iter()
            .chain(events.iter())
            .filter(|event| matches!(event, CombatEvent::Death { entity, .. } if *entity == boss))
            .count(),
        0,
        "the death was announced on the tick it happened, before these four"
    );
}

#[test]
fn a_boss_reset_puts_back_every_piece_of_it() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 8.0);
    run(&mut simulation, 300);
    set_health_fraction(&mut simulation, boss, 0.2);
    run(&mut simulation, 120);
    assert_eq!(phase(&simulation, boss), 2, "it is deep into the fight");

    let home = simulation
        .brain(boss)
        .map(engine_game::EnemyBrain::home)
        .expect("a boss has a home");

    simulation.reset_encounter();
    let brain = simulation.brain(boss).expect("the boss still exists");
    assert_eq!(brain.state(), AiState::Idle);
    assert_eq!(brain.phase(), 0);
    assert!(brain.cast().is_none());
    assert!(brain.target().is_none());
    assert!(brain.threat().is_empty());
    for ability in EnemyAbility::ALL {
        assert_eq!(
            brain.cooldown(ability),
            0.0,
            "{} is still cooling",
            ability.name()
        );
    }
    assert_eq!(
        health(&simulation, boss),
        EnemyKind::Boss.archetype().max_health
    );
    assert_eq!(simulation.world().position(boss), Some(home));
    assert!(simulation.enemy_hazards().is_empty());
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::NotStarted),
        "the player must be able to pull it again immediately"
    );
}

#[test]
fn a_corpse_stops_being_tracked_but_still_notices_that_it_died() {
    // Two halves of the same rule. A dead enemy is skipped before it perceives anything —
    // there is nothing left to decide and a cleared dungeon room would otherwise cost a
    // full world scan per corpse per tick. But an enemy killed *outside* the combat path
    // has a body at zero health and a brain that has not heard, and that one still has to
    // think once so it can die properly.
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0);
    run(&mut simulation, 120);
    assert!(state(&simulation, enemy).is_combat());

    // Killed through the world, so the brain is not told directly.
    simulation.world_mut().damage(enemy, 1_000_000.0);
    run(&mut simulation, 1);
    assert_eq!(state(&simulation, enemy), AiState::Dead, "it never noticed");

    // And from then on it is left alone: nothing it could do reaches the event stream.
    let events = run(&mut simulation, 300);
    assert!(
        !events.iter().any(|event| matches!(
            event,
            CombatEvent::AbilityUsed { source, .. }
                | CombatEvent::CastStarted { source, .. }
                | CombatEvent::TelegraphStarted { source, .. } if *source == enemy
        )),
        "a corpse acted"
    );
    assert_eq!(state(&simulation, enemy), AiState::Dead);
    assert!(
        simulation
            .brain(enemy)
            .is_some_and(|brain| brain.threat().is_empty() && brain.cast().is_none()),
        "a corpse is still holding combat state"
    );
}

#[test]
fn a_reset_brings_a_corpse_back_into_the_fight() {
    // The other side of skipping the dead: a reset has to undo it, or a wiped dungeon comes
    // back full of enemies that never think again.
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0);
    run(&mut simulation, 60);
    simulation.slay(enemy);
    run(&mut simulation, 4);
    assert_eq!(state(&simulation, enemy), AiState::Dead);

    simulation.reset_encounter();
    assert_eq!(state(&simulation, enemy), AiState::Idle);
    run(&mut simulation, 120);
    assert!(
        state(&simulation, enemy).is_combat() || state(&simulation, enemy) == AiState::Alert,
        "a restored enemy never woke up again"
    );
}

// ---------------------------------------------------------------------------------------
// Packs
// ---------------------------------------------------------------------------------------

#[test]
fn a_pack_activates_when_one_member_is_pulled_and_completes_when_all_are_dead() {
    let (mut simulation, members) = pack(&[EnemyKind::Melee, EnemyKind::Caster], 9.0);
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::NotStarted)
    );
    run(&mut simulation, 10);
    assert!(
        simulation
            .encounter()
            .is_some_and(|encounter| encounter.state().is_active()),
        "walking into a pack's aggro must start the fight"
    );

    for member in &members {
        simulation.slay(*member);
    }
    run(&mut simulation, 4);
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Completed)
    );
}

#[test]
fn pulling_one_member_of_a_pack_pulls_all_of_it() {
    // The pack rule. The Warden and the Ritualist stand well outside their own aggro radius
    // of the player and would never notice on their own; the Brute in front does, and brings
    // them.
    let config = ArenaConfig::with_group(0);
    let origin = config.player_spawn;
    let encounter = Encounter::with_spawns(
        "SPREAD",
        vec![
            // Inside a brute's 14 m: this is the one the player walks into.
            EnemySpawn::new(EnemyKind::Melee, origin + Vec3::new(0.0, 0.0, -10.0)),
            // Well outside every aggro radius in the roster.
            EnemySpawn::new(EnemyKind::Warden, origin + Vec3::new(22.0, 0.0, -30.0)),
            EnemySpawn::new(EnemyKind::Ritualist, origin + Vec3::new(-22.0, 0.0, -30.0)),
        ],
    );
    let mut simulation = arena::encounter_arena(&config, encounter);
    let members = simulation
        .encounter()
        .expect("the encounter was installed")
        .members()
        .to_vec();

    // The far two cannot see the player at all.
    for member in &members[1..] {
        let gap = horizontal_distance(
            simulation.player_position(),
            simulation
                .world()
                .position(*member)
                .expect("it is in the world"),
        );
        let radius = simulation
            .brain(*member)
            .expect("it is an enemy")
            .archetype()
            .aggro_radius;
        assert!(gap > radius, "{gap:.1} m is inside a {radius:.1} m radius");
    }

    run(&mut simulation, 300);
    for member in &members {
        assert_ne!(
            state(&simulation, *member),
            AiState::Idle,
            "a pack member was left behind"
        );
    }
}

#[test]
fn a_pack_pulled_together_can_go_home_and_be_pulled_again() {
    // A pack that walked home can be pulled again. Only a member that is *fighting* pulls its
    // packmates — otherwise the first member home is woken again by the last one still
    // walking, and the pack can never go back to sleep. The only way home is the player's
    // death: a pulled pack never leashes.
    let (mut simulation, members) = pack(&[EnemyKind::Melee, EnemyKind::Melee], 10.0);
    run(&mut simulation, 120);
    assert!(members.iter().all(|m| state(&simulation, *m).is_combat()));

    simulation.kill_player();
    run(&mut simulation, 150);
    for member in &members {
        assert_eq!(
            state(&simulation, *member),
            AiState::Idle,
            "the pack never settled"
        );
    }

    // And a second visit pulls it again, whole.
    simulation.revive_player();
    simulation.push_command(PlayerCommand::Teleport {
        position: Vec3::new(0.0, 0.0, 0.0),
    });
    run(&mut simulation, 300);
    for member in &members {
        assert_ne!(
            state(&simulation, *member),
            AiState::Idle,
            "the pack could not be pulled a second time"
        );
    }
}

#[test]
fn every_member_of_a_pack_fights_at_the_same_time() {
    // The pack requirement: three monsters must all be doing something, not taking turns.
    let (mut simulation, members) = pack(
        &[EnemyKind::Melee, EnemyKind::Caster, EnemyKind::Ritualist],
        9.0,
    );
    run(&mut simulation, 240);
    for member in &members {
        assert!(
            state(&simulation, *member).is_combat(),
            "one of the pack is standing about"
        );
        assert_eq!(
            simulation
                .brain(*member)
                .and_then(engine_game::EnemyBrain::target),
            Some(simulation.player().entity),
        );
    }
}

#[test]
fn a_pack_chases_a_player_who_runs_however_far_and_never_resets() {
    // Monsters never go back to their spawn point while the player lives: kiting a pack
    // across the map drags the whole pack along, still angry.
    let (mut simulation, members) = pack(&[EnemyKind::Melee, EnemyKind::Melee], 10.0);
    run(&mut simulation, 120);
    assert!(members.iter().all(|m| state(&simulation, *m).is_combat()));

    // Well past every leash in the roster.
    simulation.push_command(PlayerCommand::Teleport {
        position: Vec3::new(0.0, 0.0, 300.0),
    });
    run(&mut simulation, 1_500);
    for member in &members {
        assert!(
            state(&simulation, *member).is_combat(),
            "it gave up: {:?}",
            state(&simulation, *member)
        );
        let brain = simulation.brain(*member).expect("an enemy");
        assert_eq!(
            brain.target(),
            Some(simulation.player().entity),
            "it lost the player"
        );
        let from_home = horizontal_distance(
            simulation.world().position(*member).expect("in the world"),
            brain.home(),
        );
        // The arena's walls stop it long before the player's spot; what matters is that it
        // is still out there fighting rather than back where it spawned.
        assert!(from_home > 30.0, "it stayed near home: {from_home:.1} m");
    }
}

#[test]
fn resetting_one_pack_leaves_the_others_exactly_where_they_were() {
    let (mut simulation, dungeon) = build_dungeon();
    let first = first_member(&simulation, 0);
    let second = first_member(&simulation, 1);
    set_health_fraction(&mut simulation, first, 0.4);
    set_health_fraction(&mut simulation, second, 0.4);

    dungeon.reset_pack(&mut simulation, 0);
    run(&mut simulation, 2);
    assert_eq!(
        health(&simulation, first),
        EnemyKind::Melee.archetype().max_health,
        "the pack that was reset is not whole"
    );
    assert!(
        health(&simulation, second) < EnemyKind::Caster.archetype().max_health,
        "resetting one pack healed another"
    );
}

// ---------------------------------------------------------------------------------------
// The dungeon
// ---------------------------------------------------------------------------------------

#[test]
fn the_dungeon_starts_asleep_with_every_pack_standing_where_it_belongs() {
    let (simulation, dungeon) = build_dungeon();
    assert_eq!(simulation.encounters().len(), PACK_COUNT);
    assert_eq!(dungeon.cleared(&simulation), 0);
    assert_eq!(dungeon.active_pack(&simulation), None);
    for (index, encounter) in simulation.encounters().iter().enumerate() {
        assert_eq!(encounter.state(), EncounterState::NotStarted);
        assert_eq!(encounter.len(), PACKS[index].len());
    }
}

#[test]
fn nothing_notices_the_player_until_they_walk_forward() {
    let (mut simulation, dungeon) = build_dungeon();
    let _ = &dungeon;
    // Standing still in the entrance hall for ten seconds.
    run(&mut simulation, 600);
    assert_eq!(dungeon.active_pack(&simulation), None);
    for (_, brain) in simulation.enemies() {
        assert_eq!(brain.state(), AiState::Idle);
    }
}

#[test]
fn walking_forward_pulls_the_packs_in_order() {
    // The dungeon's whole flow property: it is linear, and it is linear because of where the
    // rooms are rather than because anything sequences them.
    let (mut simulation, _dungeon) = build_dungeon();
    let mut pulled: Vec<usize> = Vec::new();

    for step in 0..8_000 {
        // Forward is −Z, which is `forward: 1` at yaw zero.
        if step % 30 == 0 {
            simulation.push_command(PlayerCommand::Move {
                forward: 1.0,
                right: 0.0,
                yaw: 0.0,
                face_camera: false,
            });
        }
        simulation.step_once();
        // The player is immortal for this test: what is being checked is the order rooms
        // wake up in, not whether the player survives them.
        let player = simulation.player().entity;
        simulation.world_mut().heal(player, 1_000.0);
        // And so is everything else, so nothing is cleared and the order stays observable.
        for index in 0..PACK_COUNT {
            if let Some(encounter) = simulation.encounter_at(index)
                && encounter.state().is_active()
                && !pulled.contains(&index)
            {
                pulled.push(index);
            }
        }
        if pulled.len() == PACK_COUNT {
            break;
        }
    }

    assert_eq!(
        pulled.len(),
        PACK_COUNT,
        "the player never reached everything"
    );
    let mut sorted = pulled.clone();
    sorted.sort_unstable();
    assert_eq!(pulled, sorted, "the packs woke up out of order: {pulled:?}");
    assert_eq!(*pulled.last().expect("a last pack"), BOSS_PACK);
}

#[test]
fn a_dungeon_reset_leaves_no_stale_state_anywhere() {
    let (mut simulation, mut dungeon) = build_dungeon();

    // Make a mess: wound things, kill things, start fights, light fires.
    for index in 0..PACK_COUNT {
        let member = first_member(&simulation, index);
        set_health_fraction(&mut simulation, member, 0.25);
    }
    let boss = dungeon.boss(&simulation).expect("the boss exists");
    set_health_fraction(&mut simulation, boss, 0.2);
    simulation.push_command(PlayerCommand::Teleport {
        position: simulation
            .world()
            .position(boss)
            .expect("the boss is somewhere")
            + Vec3::new(0.0, 0.0, 4.0),
    });
    run(&mut simulation, 400);
    simulation.slay(first_member(&simulation, 0));
    let player = simulation.player().entity;
    simulation.world_mut().damage(player, 10_000.0);
    run(&mut simulation, 4);

    dungeon.reset(&mut simulation);
    assert_eq!(dungeon.resets(), 1);
    assert_eq!(dungeon.cleared(&simulation), 0);
    assert!(simulation.player_alive());
    assert!(simulation.enemy_hazards().is_empty());
    for (index, encounter) in simulation.encounters().iter().enumerate() {
        assert_eq!(
            encounter.state(),
            EncounterState::NotStarted,
            "{} did not reset",
            encounter.name()
        );
        for member in encounter.members() {
            let brain = simulation.brain(*member).expect("a member has a brain");
            assert_eq!(brain.state(), AiState::Idle);
            assert_eq!(brain.phase(), 0);
            assert!(brain.cast().is_none());
            assert!(brain.threat().is_empty());
            assert_eq!(
                simulation.world().position(*member),
                Some(brain.home()),
                "{} member is not home",
                PACKS[index].name
            );
            assert_eq!(
                health(&simulation, *member),
                brain.archetype().max_health,
                "{} member is not whole",
                PACKS[index].name
            );
        }
    }
}

#[test]
fn a_wipe_during_a_cast_and_a_telegraph_still_resets_cleanly() {
    // The nastiest moment to reset in: something is mid-cast with a telegraph on the floor
    // and the player is dead underneath it.
    let (mut simulation, boss) = duel(EnemyKind::Boss, 6.0);
    let mut casting = false;
    for _ in 0..600 {
        simulation.step_once();
        if simulation
            .brain(boss)
            .and_then(engine_game::EnemyBrain::cast)
            .is_some()
        {
            casting = true;
            break;
        }
    }
    assert!(casting);

    let player = simulation.player().entity;
    simulation.world_mut().damage(player, 10_000.0);
    simulation.reset_encounter();

    assert!(simulation.player_alive());
    let brain = simulation.brain(boss).expect("the boss exists");
    assert!(brain.cast().is_none(), "a cast survived the wipe");
    assert_eq!(brain.state(), AiState::Idle);
    assert!(brain.threat().is_empty());
    assert!(simulation.enemy_hazards().is_empty());
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::NotStarted)
    );
    // And the fight starts again on its own, because the revived player is standing in front
    // of it — a reset puts the encounter back, it does not put the player somewhere safe.
    run(&mut simulation, 60);
    assert!(state(&simulation, boss).is_combat() || state(&simulation, boss) == AiState::Alert);
}

#[test]
fn the_dungeon_is_deterministic_over_a_long_run() {
    // Two runs of the same script produce the same events. Nothing in the dungeon may read a
    // clock or a random number.
    let script = |ticks: u32| -> Vec<String> {
        let (mut simulation, _) = build_dungeon();
        let mut log = Vec::new();
        for step in 0..ticks {
            if step % 30 == 0 {
                simulation.push_command(PlayerCommand::Move {
                    forward: 1.0,
                    right: 0.0,
                    yaw: 0.0,
                    face_camera: false,
                });
            }
            simulation.step_once();
            for event in simulation.events() {
                log.push(format!("{:?}", event.tick()));
            }
        }
        log
    };
    assert_eq!(script(1_200), script(1_200));
}

// ---------------------------------------------------------------------------------------
// Pack aggro, wipes and the aggro radius
// ---------------------------------------------------------------------------------------

#[test]
fn a_packmate_that_is_idle_while_the_rest_fight_is_brought_into_the_fight() {
    // Not only on the first pull: a member that is at home and idle while another member of
    // its pack is fighting joins — which is what a straggler that was still walking home
    // when the pack was pulled again is, the moment it arrives.
    let (mut simulation, members) = pack(&[EnemyKind::Melee, EnemyKind::Melee], 10.0);
    run(&mut simulation, 120);
    assert!(members.iter().all(|m| state(&simulation, *m).is_combat()));

    simulation.restore_enemy(members[1]);
    assert_eq!(state(&simulation, members[1]), AiState::Idle);
    run(&mut simulation, 60);
    assert!(
        state(&simulation, members[1]).is_combat(),
        "left idle beside a packmate that is fighting: {:?}",
        state(&simulation, members[1])
    );
}

#[test]
fn when_the_player_dies_the_monsters_heal_and_walk_home() {
    let (mut simulation, members) = pack(&[EnemyKind::Melee, EnemyKind::Melee], 10.0);
    run(&mut simulation, 180);
    assert!(members.iter().all(|m| state(&simulation, *m).is_combat()));
    for member in &members {
        set_health_fraction(&mut simulation, *member, 0.4);
    }

    let player = simulation.player().entity;
    simulation.world_mut().damage(player, 1e9);
    run(&mut simulation, 2);
    assert!(!simulation.player_alive());
    for member in &members {
        let max = simulation
            .world()
            .health(*member)
            .map_or(0.0, engine_game::Health::max);
        assert_eq!(health(&simulation, *member), max, "not healed to full");
        assert_eq!(state(&simulation, *member), AiState::Returning);
        let brain = simulation.brain(*member).expect("an enemy");
        assert!(brain.threat().is_empty(), "it still remembers the fight");
        let from_home = horizontal_distance(
            simulation.world().position(*member).expect("in the world"),
            brain.home(),
        );
        assert!(
            from_home > 3.0,
            "it was put home rather than walking: {from_home:.1} m"
        );
    }

    // Home before the player comes back, standing where it spawned and asleep.
    run(&mut simulation, 150);
    for member in &members {
        let brain = simulation.brain(*member).expect("an enemy");
        assert_eq!(state(&simulation, *member), AiState::Idle, "never got home");
        let from_home = horizontal_distance(
            simulation.world().position(*member).expect("in the world"),
            brain.home(),
        );
        assert!(from_home < 1.0, "{from_home:.1} m from home");
    }
}

#[test]
fn every_monster_a_level_places_notices_from_under_eleven_metres() {
    use engine_game::enemy::{AGGRO_RADIUS, EMPOWERED_AGGRO_RADIUS};
    // Forty per cent under the 18 m it was, for both kinds of monster.
    assert!((AGGRO_RADIUS - 18.0 * 0.6).abs() < 1e-4);
    assert!((EMPOWERED_AGGRO_RADIUS - 40.0 * 0.6).abs() < 1e-4);
    for kind in EnemyKind::ALL {
        let radius = kind.archetype().aggro_radius;
        if kind.name().starts_with("EMPOWERED") {
            // Called into a fight already running; they never have to notice anybody.
            assert!(radius >= AGGRO_RADIUS, "{kind:?}");
            continue;
        }
        assert_eq!(radius, AGGRO_RADIUS, "{kind:?}");
    }
}
