//! A real fight, driven headless: enemies notice, chase, attack, telegraph, die and reset.
//!
//! Every test here runs the **actual** simulation — the real arena, the real brains, the
//! real damage path — with no device, no window and no presentation. That is the point of
//! the layering: "does the fight work" is a question about a CPU pass over arrays, and the
//! answer must not depend on anything being drawn.
//!
//! What each test asserts is the *rule*, not the tuning: that an enemy stops when it is in
//! range rather than at 2.4 m specifically, so a rebalance moves numbers without moving
//! tests.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::encounter::{Encounter, EncounterState, EnemySpawn};
use engine_game::enemy::brain::AiState;
use engine_game::enemy::{EnemyAbility, EnemyKind, TelegraphShape};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::world::horizontal_distance;

/// A simulation with one enemy of `kind` `distance` metres in front of the player.
fn duel(kind: EnemyKind, distance: f32) -> (Simulation, EntityId) {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("DUEL", vec![EnemySpawn::new(kind, spawn)]);
    let simulation = arena::encounter_arena(&config, encounter);
    let enemy = simulation
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    (simulation, enemy)
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

/// The player's health.
fn player_health(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .health(simulation.player().entity)
        .map_or(0.0, |health| health.current())
}

#[test]
fn an_enemy_notices_the_player_chases_and_attacks() {
    // The whole loop, in one test: Idle to Alert to Chase to something that hurts.
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 10.0);
    assert_eq!(state(&simulation, enemy), AiState::Idle);
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::NotStarted)
    );

    // 10 m is inside a brute's aggro radius, so it notices without being provoked.
    run(&mut simulation, 2);
    assert_eq!(state(&simulation, enemy), AiState::Alert);

    let start = simulation
        .world()
        .position(enemy)
        .expect("the enemy is in the world");
    run(&mut simulation, 120);
    let now = simulation
        .world()
        .position(enemy)
        .expect("the enemy is in the world");
    assert!(
        horizontal_distance(start, now) > 3.0,
        "it never moved: {start:?} -> {now:?}"
    );
    assert!(state(&simulation, enemy).is_combat());
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Active),
        "a pull starts the encounter"
    );

    // And within a few seconds the player has been hit by something.
    run(&mut simulation, 360);
    assert!(
        player_health(&simulation) < 500.0,
        "the player took no damage in six seconds of melee"
    );
    assert!(simulation.meter().total_damage_taken() > 0.0);
}

#[test]
fn an_enemy_stops_inside_its_range_instead_of_walking_through_the_player() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 12.0);
    run(&mut simulation, 600);

    let player = simulation.player_position();
    let position = simulation
        .world()
        .position(enemy)
        .expect("the enemy is in the world");
    let gap = horizontal_distance(player, position);
    let preferred = EnemyKind::Melee.archetype().preferred_range;
    assert!(gap > 0.5, "it walked into the player: {gap} m apart");
    assert!(
        gap < preferred + 4.0,
        "it stopped {gap} m away, well outside its {preferred} m range"
    );
}

#[test]
fn a_caster_telegraphs_before_it_hits_and_hits_what_it_drew() {
    let (mut simulation, enemy) = duel(EnemyKind::Caster, 18.0);
    let events = run(&mut simulation, 600);

    let telegraph = events
        .iter()
        .find_map(|event| match *event {
            CombatEvent::TelegraphStarted {
                source,
                anchor,
                shape,
                delay,
                ..
            } if source == enemy => Some((anchor, shape, delay)),
            _ => None,
        })
        .expect("the caster never telegraphed anything");
    let (anchor, shape, delay) = telegraph;
    assert!(
        delay > 0.5,
        "a telegraph the player cannot react to is not one"
    );
    assert!(matches!(shape, TelegraphShape::Circle { .. }));

    // Everything the blast damaged was inside the shape the telegraph announced.
    let ability = engine_game::AbilityId::from_enemy(EnemyAbility::GroundBlast);
    let hits: Vec<_> = events
        .iter()
        .filter_map(|event| event.as_damage())
        .filter(|damage| damage.ability == ability)
        .collect();
    assert!(!hits.is_empty(), "the blast landed on nobody");
    for hit in hits {
        assert!(
            shape.contains(anchor, hit.position, 0.6),
            "a hit landed outside the telegraph: {:?} against {anchor:?} {shape:?}",
            hit.position
        );
    }
}

#[test]
fn a_telegraph_is_matched_by_exactly_one_ending() {
    let (mut simulation, _) = duel(EnemyKind::Caster, 18.0);
    let events = run(&mut simulation, 900);

    let mut open: Vec<u32> = Vec::new();
    for event in &events {
        match *event {
            CombatEvent::TelegraphStarted { id, .. } => {
                assert!(!open.contains(&id), "telegraph {id} started twice");
                open.push(id);
            }
            CombatEvent::TelegraphEnded { id, .. } => {
                let index = open
                    .iter()
                    .position(|open| *open == id)
                    .expect("a telegraph ended that never started");
                open.swap_remove(index);
            }
            _ => {}
        }
    }
    assert!(
        open.len() <= 1,
        "{} telegraphs are still up at the end of the fight",
        open.len()
    );
}

#[test]
fn an_interrupted_cast_deals_no_damage() {
    // Milestone 13: the foundation, asserted where it matters - an interrupt has to cost
    // the enemy its attack, not merely its cast bar.
    let (mut simulation, enemy) = duel(EnemyKind::Caster, 18.0);
    let mut interrupted = false;
    for _ in 0..900 {
        simulation.step_once();
        if simulation.is_interruptible(enemy) {
            assert!(simulation.interrupt(enemy), "it said it was interruptible");
            interrupted = true;
            break;
        }
    }
    assert!(
        interrupted,
        "the caster never started an interruptible cast"
    );

    // The interrupt is announced immediately, into the batch the frame is already reading:
    // a cast bar that only came down on the next tick would show a cast that had stopped.
    assert!(
        simulation.events().iter().any(|event| matches!(
            event,
            CombatEvent::CastEnded {
                end: CastEnd::Interrupted,
                ..
            }
        )),
        "an interrupt has to be announced, or no cast bar can come down"
    );
    assert!(
        simulation.events().iter().any(|event| matches!(
            event,
            CombatEvent::TelegraphEnded {
                resolved: false,
                ..
            }
        )),
        "the telegraph has to come off the floor with the cast"
    );

    // The cast is gone, the enemy is in recovery, and nothing landed from it.
    assert!(
        simulation
            .brain(enemy)
            .and_then(engine_game::EnemyBrain::cast)
            .is_none()
    );
    assert_eq!(state(&simulation, enemy), AiState::Recover);
    assert!(!simulation.is_interruptible(enemy));
}

#[test]
fn damage_generates_threat_and_threat_picks_the_target() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 25.0);
    // 25 m is outside a brute's aggro radius: it has noticed nothing.
    run(&mut simulation, 60);
    assert_eq!(state(&simulation, enemy), AiState::Idle);

    // A hit from out there pulls it anyway, which is what threat is for.
    simulation.world_mut().damage(enemy, 20.0);
    simulation.player_mut().target = Some(enemy);
    let player = simulation.player().entity;
    simulation.step_once();
    // The damage above went straight to the world, so feed the threat the way combat does.
    let _ = player;

    let (mut simulation, enemy) = duel(EnemyKind::Melee, 25.0);
    simulation.player_mut().target = Some(enemy);
    simulation
        .cast(engine_game::FireAbility::Firebolt)
        .expect("firebolt is in range at 25 m");
    run(&mut simulation, 120);

    let brain = simulation.brain(enemy).expect("an enemy");
    assert!(!brain.threat().is_empty(), "being shot generated no threat");
    assert_eq!(
        brain.target(),
        Some(simulation.player().entity),
        "the only attacker is the target"
    );
    assert!(brain.state() != AiState::Idle, "a ranged pull must pull");
}

#[test]
fn a_leashed_enemy_goes_home_forgets_and_can_be_pulled_again() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0);
    run(&mut simulation, 60);
    assert!(state(&simulation, enemy).is_combat());
    let home = simulation
        .brain(enemy)
        .map(engine_game::EnemyBrain::home)
        .expect("an enemy");

    // The player leaves the county. The enemy chases, runs out of leash and turns round.
    let far = home + Vec3::new(0.0, 0.0, 120.0);
    simulation.push_command(engine_game::PlayerCommand::Teleport { position: far });
    run(&mut simulation, 1_200);

    assert_eq!(
        state(&simulation, enemy),
        AiState::Idle,
        "it never went home"
    );
    let position = simulation.world().position(enemy).expect("an enemy");
    assert!(
        horizontal_distance(position, home) < 2.0,
        "it stopped {:.1} m from home",
        horizontal_distance(position, home)
    );
    assert!(
        simulation
            .brain(enemy)
            .expect("an enemy")
            .threat()
            .is_empty(),
        "a leashed enemy must forgive"
    );
}

#[test]
fn an_enemy_that_dies_stops_fighting_and_the_encounter_completes() {
    let (mut simulation, enemy) = duel(EnemyKind::Caster, 16.0);
    run(&mut simulation, 120);
    assert!(state(&simulation, enemy).is_combat());

    simulation.world_mut().damage(enemy, 1e9);
    let events = run(&mut simulation, 4);

    assert_eq!(state(&simulation, enemy), AiState::Dead);
    assert!(
        simulation
            .brain(enemy)
            .and_then(engine_game::EnemyBrain::cast)
            .is_none()
    );
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Completed),
        "the last death completes the fight"
    );

    // And it does nothing further, however long the player stands on it.
    let before = player_health(&simulation);
    let after_events = run(&mut simulation, 600);
    assert_eq!(player_health(&simulation), before, "a corpse dealt damage");
    assert!(
        !after_events
            .iter()
            .any(|event| matches!(event, CombatEvent::TelegraphStarted { .. })),
        "a corpse telegraphed an attack"
    );
    let _ = events;
}

#[test]
fn a_dead_player_cannot_act_and_a_reset_puts_everything_back() {
    let config = ArenaConfig::with_group(0);
    let encounter = arena::demo_encounter(&config);
    let mut simulation = arena::encounter_arena(&config, encounter);
    let player = simulation.player().entity;

    simulation.world_mut().damage(player, 1e9);
    simulation.step_once();
    assert!(!simulation.player_alive());
    assert!(
        simulation.cast(engine_game::FireAbility::Firebolt).is_err(),
        "a dead player cast a spell"
    );

    simulation.reset_encounter();
    simulation.step_once();
    assert!(
        simulation.player_alive(),
        "the reset did not revive the player"
    );
    assert_eq!(player_health(&simulation), 500.0);
    for (entity, brain) in simulation.enemies() {
        assert_eq!(brain.state(), AiState::Idle, "an enemy was not reset");
        assert!(brain.threat().is_empty());
        assert!(simulation.world().is_alive(entity));
        assert!(
            horizontal_distance(
                simulation.world().position(entity).expect("an enemy"),
                brain.home()
            ) < 0.1,
            "an enemy was not put back"
        );
    }
}

#[test]
fn a_pack_all_fights_and_all_of_it_can_die() {
    // Milestone 18: several enemies at once, all targeting, all attacking, all dying.
    let config = ArenaConfig::with_group(0);
    let mut simulation = arena::encounter_arena(&config, arena::pack_encounter(&config, 5));
    let members: Vec<EntityId> = simulation
        .encounter()
        .map(|encounter| encounter.members().to_vec())
        .expect("a pack");
    assert_eq!(members.len(), 5);

    // Walk into them.
    let target = simulation.player_position() + Vec3::new(0.0, 0.0, -16.0);
    simulation.push_command(engine_game::PlayerCommand::Teleport { position: target });
    run(&mut simulation, 600);

    let fighting = members
        .iter()
        .filter(|member| {
            simulation
                .brain(**member)
                .is_some_and(|brain| brain.state().is_combat())
        })
        .count();
    assert!(fighting >= 3, "only {fighting} of 5 joined the fight");
    assert!(simulation.meter().total_damage_taken() > 0.0);

    for member in &members {
        simulation.world_mut().damage(*member, 1e9);
    }
    run(&mut simulation, 4);
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Completed)
    );
}

/// A pack of `count` brutes standing `distance` metres in front of the player.
///
/// Far enough that nothing in it notices the player on its own, so the only thing that can
/// wake it is being attacked.
fn sleeping_pack(count: usize, distance: f32) -> (Simulation, Vec<EntityId>) {
    let config = ArenaConfig::with_group(0);
    let origin = config.player_spawn;
    let spawns = (0..count)
        .map(|index| {
            let offset = Vec3::new(index as f32 * 3.0 - 3.0, 0.0, -distance);
            EnemySpawn::new(EnemyKind::Melee, origin + offset)
        })
        .collect();
    let mut simulation = arena::encounter_arena(&config, Encounter::with_spawns("PACK", spawns));
    let members = simulation
        .encounter()
        .map(|encounter| encounter.members().to_vec())
        .expect("the encounter spawned its members");
    run(&mut simulation, 60);
    for member in &members {
        assert_eq!(
            state(&simulation, *member),
            AiState::Idle,
            "the pack must start asleep"
        );
    }
    (simulation, members)
}

#[test]
fn one_shotting_a_pack_member_still_wakes_the_rest() {
    // The alarm a corpse cannot raise for itself. An enemy killed outright never leaves Idle,
    // so "somebody in this pack is awake" is false on every tick — and without the killing
    // blow itself being the pull, a player could delete a five-pack one member at a time.
    let (mut simulation, members) = sleeping_pack(3, 30.0);
    let victim = members[0];

    // Softened to a sliver, then killed by one cast: the killing blow travels the real damage
    // path, which is the only place the killer's identity exists.
    simulation.world_mut().damage(victim, 319.0);
    simulation.player_mut().target = Some(victim);
    simulation
        .cast(engine_game::FireAbility::Firebolt)
        .expect("firebolt reaches 30 m");
    let events = run(&mut simulation, 120);

    assert!(
        events.iter().any(|event| matches!(
            *event,
            CombatEvent::Death { entity, killer, .. }
                if entity == victim && killer == simulation.player().entity
        )),
        "the player never killed it"
    );
    assert!(!simulation.world().is_alive(victim));

    let player = simulation.player().entity;
    for member in members.iter().skip(1) {
        let brain = simulation.brain(*member).expect("an enemy");
        assert_ne!(
            brain.state(),
            AiState::Idle,
            "a pack member slept through its neighbour being killed"
        );
        assert_eq!(
            brain.target(),
            Some(player),
            "the pack woke up angry at nobody"
        );
    }
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Active),
        "a kill is a pull, and a pull starts the fight"
    );
}

#[test]
fn a_pack_woken_by_a_kill_actually_comes_and_fights() {
    // Aggro that never arrives is not aggro: the survivors have to cross thirty metres and
    // hit somebody.
    let (mut simulation, members) = sleeping_pack(3, 30.0);
    let victim = members[0];
    simulation.world_mut().damage(victim, 319.0);
    simulation.player_mut().target = Some(victim);
    simulation
        .cast(engine_game::FireAbility::Firebolt)
        .expect("firebolt reaches 30 m");
    run(&mut simulation, 900);

    assert!(
        simulation.meter().total_damage_taken() > 0.0,
        "the woken pack never reached the player"
    );
}

#[test]
fn a_pack_wiped_by_one_shot_each_completes_and_does_not_stay_pulled() {
    // The other end of the same rule: the alarm must not outlive the pack. A corpse's dying
    // shout is spent once, and an encounter with nobody left in it is over.
    let (mut simulation, members) = sleeping_pack(2, 30.0);
    for member in &members {
        simulation.world_mut().damage(*member, 319.0);
    }
    for member in &members {
        simulation.player_mut().target = Some(*member);
        simulation
            .cast(engine_game::FireAbility::Firebolt)
            .expect("firebolt reaches 30 m");
        run(&mut simulation, 90);
    }

    for member in &members {
        assert!(!simulation.world().is_alive(*member));
        assert_eq!(state(&simulation, *member), AiState::Dead);
    }
    assert_eq!(
        simulation.encounter().map(engine_game::Encounter::state),
        Some(EncounterState::Completed),
        "a pack killed to the last member is a fight that is over"
    );
}

#[test]
fn the_meter_counts_what_the_player_dealt_and_not_what_it_took() {
    // The latent bug an enemy reveals: without an owner, damage *taken* would be reported
    // as damage *dealt* and the DPS readout would be a lie.
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 6.0);
    simulation.player_mut().target = Some(enemy);
    run(&mut simulation, 600);

    assert!(
        simulation.meter().total_damage_taken() > 0.0,
        "the brute never landed a hit"
    );
    assert_eq!(
        simulation.meter().total_damage(),
        0.0,
        "the player cast nothing and yet the meter counted damage"
    );
}

#[test]
fn two_runs_of_the_same_fight_are_identical() {
    // Milestone 22, at the scale that matters: the same script, twice, compared by the
    // events it produced. AI uses no randomness at all, so this is exact.
    let script = |simulation: &mut Simulation| -> Vec<String> {
        let mut log = Vec::new();
        for tick in 0..900 {
            if tick == 30 {
                simulation.select_nearest_target();
            }
            if tick % 60 == 0 {
                let _ = simulation.cast(engine_game::FireAbility::Firebolt);
            }
            simulation.step_once();
            for event in simulation.events() {
                log.push(format!("{event:?}"));
            }
        }
        log
    };

    let config = ArenaConfig::with_group(0);
    let mut first = arena::encounter_arena(&config, arena::demo_encounter(&config));
    let mut second = arena::encounter_arena(&config, arena::demo_encounter(&config));
    let first = script(&mut first);
    let second = script(&mut second);

    assert!(!first.is_empty(), "the script produced no events");
    assert_eq!(first.len(), second.len());
    assert_eq!(first, second);
}
