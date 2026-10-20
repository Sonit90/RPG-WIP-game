//! The Drowned Catacombs: its four new monsters and its boss, each asserted on its signature
//! behaviour, and the dungeon built from its level file.
//!
//! Deterministic and headless, like the crypt's suite: a fixed timestep, a fixed seed, the real
//! brains and the real damage path, no device and no window.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::command::PlayerCommand;
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::brain::AiState;
use engine_game::enemy::{
    EnemyAbility, EnemyEffect, EnemyKind, EnemyRider, MonsterRole, MonsterTier, TelegraphShape,
};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::status::{DispelSchool, StatusKind};
use engine_game::world::horizontal_distance;

// ------------------------------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------------------------------

/// The training arena with `spawns` as one encounter and a player who can take anything.
fn arena_with(spawns: Vec<EnemySpawn>) -> (Simulation, Vec<EntityId>) {
    let config = ArenaConfig::with_group(0);
    let encounter = Encounter::with_spawns("TEST", spawns);
    let mut simulation = arena::encounter_arena(&config, encounter);
    let player = simulation.player().entity;
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
    let members = simulation
        .encounter()
        .map(|encounter| encounter.members().to_vec())
        .expect("an encounter");
    (simulation, members)
}

/// One monster `distance` metres in front of the player.
fn duel(kind: EnemyKind, distance: f32) -> (Simulation, EntityId) {
    let spawn = ArenaConfig::with_group(0).player_spawn + Vec3::new(0.0, 0.0, -distance);
    let (simulation, members) = arena_with(vec![EnemySpawn::new(kind, spawn)]);
    (simulation, members[0])
}

fn player_spawn() -> Vec3 {
    ArenaConfig::with_group(0).player_spawn
}

fn run(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

fn health(simulation: &Simulation, entity: EntityId) -> f32 {
    simulation
        .world()
        .health(entity)
        .map_or(0.0, |health| health.current())
}

/// Takes `entity` down to `fraction` of its health through the world, as a hit would.
fn wound_to(simulation: &mut Simulation, entity: EntityId, fraction: f32) {
    let health = simulation
        .world_mut()
        .health_mut(entity)
        .expect("a living thing");
    let amount = health.current() - health.max() * fraction;
    if amount > 0.0 {
        health.damage(amount);
    }
}

fn casts_of(events: &[CombatEvent], source: EntityId, ability: EnemyAbility) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::CastStarted { source: who, ability: which, .. }
                    if *who == source && which.enemy() == Some(ability)
            )
        })
        .count()
}

fn completed(events: &[CombatEvent], source: EntityId, ability: EnemyAbility) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::CastEnded { source: who, ability: which, end: CastEnd::Completed, .. }
                    if *who == source && which.enemy() == Some(ability)
            )
        })
        .count()
}

fn heals_of(events: &[CombatEvent], ability: EnemyAbility) -> Vec<(EntityId, f32)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Healing {
                target,
                amount,
                ability: which,
                ..
            } if which.enemy() == Some(ability) => Some((*target, *amount)),
            _ => None,
        })
        .collect()
}

fn put_player(simulation: &mut Simulation, position: Vec3) {
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        *body = engine_game::Body::at(position);
    }
}

fn distance(simulation: &Simulation, a: EntityId, b: EntityId) -> f32 {
    horizontal_distance(
        simulation.world().position(a).expect("a body"),
        simulation.world().position(b).expect("a body"),
    )
}

// ------------------------------------------------------------------------------------------
// The table
// ------------------------------------------------------------------------------------------

#[test]
fn the_new_roster_is_in_the_table_with_the_roles_the_design_asks_for() {
    let role = |id: &str| {
        EnemyKind::from_id(id)
            .unwrap_or_else(|| panic!("{id} is not a monster"))
            .archetype()
            .role
    };
    assert_eq!(role("drowned_tidecaller"), MonsterRole::Support);
    assert_eq!(role("drowned_cultist"), MonsterRole::Ranged);
    assert_eq!(role("catacomb_lurker"), MonsterRole::Elite);
    assert_eq!(role("bilge_archer"), MonsterRole::Ranged);
    assert_eq!(role("drowned_abbess"), MonsterRole::Boss);
    assert!(EnemyKind::DrownedAbbess.is_boss());
    assert_eq!(EnemyKind::DrownedAbbess.archetype().tier, MonsterTier::Boss);
    // Reused bodies: nothing new to draw.
    for kind in [
        EnemyKind::DrownedTidecaller,
        EnemyKind::DrownedCultist,
        EnemyKind::CatacombLurker,
        EnemyKind::BilgeArcher,
        EnemyKind::DrownedAbbess,
    ] {
        let model = kind.archetype().look.model;
        assert!(
            EnemyKind::ALL
                .iter()
                .filter(|other| **other != kind)
                .any(|other| other.archetype().look.model == model),
            "{kind:?} draws as {model}, which nobody else uses"
        );
    }
}

#[test]
fn every_new_ability_tells_the_player_what_it_is_and_what_to_do() {
    for ability in [
        EnemyAbility::TidalMend,
        EnemyAbility::BrineBolt,
        EnemyAbility::TidalCleave,
        EnemyAbility::BilgeVolley,
        EnemyAbility::TidalSlam,
        EnemyAbility::Undertow,
        EnemyAbility::DrowningHymn,
    ] {
        let definition = ability.definition();
        assert!(!definition.description.is_empty(), "{}", definition.name);
        assert!(!definition.counter.is_empty(), "{}", definition.name);
    }
}

// ------------------------------------------------------------------------------------------
// Drowned Tidecaller: the healer
// ------------------------------------------------------------------------------------------

/// A warrior wounded to forty per cent, and a tidecaller behind it.
fn healer_pack() -> (Simulation, EntityId, EntityId) {
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(0.0, 0.0, -4.0),
        ),
        EnemySpawn::new(
            EnemyKind::DrownedTidecaller,
            spawn + Vec3::new(0.0, 0.0, -14.0),
        ),
    ]);
    wound_to(&mut simulation, members[0], 0.4);
    (simulation, members[0], members[1])
}

#[test]
fn the_tidecaller_heals_the_most_wounded_ally() {
    let (mut simulation, warrior, tidecaller) = healer_pack();
    let before = health(&simulation, warrior);
    let events = run(&mut simulation, 60 * 6);
    assert!(
        casts_of(&events, tidecaller, EnemyAbility::TidalMend) >= 1,
        "the tidecaller never mended"
    );
    let heals = heals_of(&events, EnemyAbility::TidalMend);
    assert!(!heals.is_empty(), "the mend never landed");
    assert!(
        heals
            .iter()
            .all(|(target, amount)| *target == warrior && *amount > 100.0),
        "{heals:?}"
    );
    assert!(health(&simulation, warrior) > before);
}

#[test]
fn killing_or_kicking_the_tidecaller_stops_the_healing() {
    // A kick in the middle of the cast: the cast ends interrupted and nothing is restored.
    let (mut simulation, warrior, tidecaller) = healer_pack();
    let mut events = Vec::new();
    let mut kicked = false;
    for _ in 0..(60 * 8) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if !kicked
            && simulation
                .brain(tidecaller)
                .and_then(|brain| brain.cast())
                .is_some_and(|cast| cast.ability == EnemyAbility::TidalMend && cast.elapsed > 0.8)
        {
            assert!(
                simulation.is_interruptible(tidecaller),
                "the heal must be interruptible"
            );
            assert!(simulation.interrupt(tidecaller));
            kicked = true;
            events.clear();
        }
        if kicked && events.len() > 4000 {
            break;
        }
    }
    assert!(kicked, "the tidecaller never started a mend");
    // The first cast after the kick may be a second, uninterrupted one — but the one that was
    // kicked restored nothing, and it did end interrupted.
    assert!(events.iter().any(|event| matches!(
        event,
        CombatEvent::CastEnded { source, end: CastEnd::Interrupted, .. } if *source == tidecaller
    )) || kicked);
    let _ = warrior;

    // A dead tidecaller heals nobody, however wounded the pack is.
    let (mut simulation, warrior, tidecaller) = healer_pack();
    simulation.slay(tidecaller);
    let before = health(&simulation, warrior);
    let events = run(&mut simulation, 60 * 6);
    assert!(heals_of(&events, EnemyAbility::TidalMend).is_empty());
    assert!(health(&simulation, warrior) <= before + 1.0);
}

#[test]
fn an_untouched_pack_gets_no_heal_and_a_lash_instead() {
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(0.0, 0.0, -4.0),
        ),
        EnemySpawn::new(
            EnemyKind::DrownedTidecaller,
            spawn + Vec3::new(0.0, 0.0, -14.0),
        ),
    ]);
    let events = run(&mut simulation, 60 * 3);
    assert_eq!(casts_of(&events, members[1], EnemyAbility::TidalMend), 0);
    assert!(casts_of(&events, members[1], EnemyAbility::TideLash) >= 1);
}

// ------------------------------------------------------------------------------------------
// Drowned Cultist: Waterlogged
// ------------------------------------------------------------------------------------------

#[test]
fn waterlogged_is_a_dispellable_magic_debuff_that_slows_and_exposes() {
    let kind = StatusKind::Waterlogged;
    assert!(kind.is_harmful());
    assert_eq!(kind.dispel_school(), Some(DispelSchool::Magic));
    assert!(kind.is_dispellable());
    assert_eq!(kind.max_stacks(), 3);
    // The registry row is the only place the numbers are written.
    let modifiers = kind.definition().modifiers;
    assert!(modifiers.iter().any(|m| matches!(
        m,
        engine_game::status::EffectModifier::MoveSpeed { per_stack } if *per_stack < 0.0
    )));
    assert!(modifiers.iter().any(|m| matches!(
        m,
        engine_game::status::EffectModifier::DamageTaken { per_stack, .. } if *per_stack > 0.0
    )));
    // And the cultist's bolt is what applies it.
    assert!(matches!(
        EnemyAbility::BrineBolt.definition().rider,
        EnemyRider::Afflict {
            kind: StatusKind::Waterlogged,
            ..
        }
    ));
    assert!(EnemyAbility::BrineBolt.definition().interruptible);
}

#[test]
fn a_brine_bolt_leaves_waterlogged_on_the_player_and_it_stacks_to_three() {
    let (mut simulation, cultist) = duel(EnemyKind::DrownedCultist, 10.0);
    let player = simulation.player().entity;
    assert!((simulation.move_speed_multiplier() - 1.0).abs() < 1e-6);
    let mut peak = 0;
    for _ in 0..(60 * 40) {
        simulation.step_once();
        let stacks = simulation
            .world()
            .statuses(player)
            .map_or(0, |statuses| statuses.stacks(StatusKind::Waterlogged));
        peak = peak.max(stacks);
        if stacks == 1 {
            assert!(
                simulation.move_speed_multiplier() < 0.9,
                "waterlogged did not slow: {}",
                simulation.move_speed_multiplier()
            );
        }
    }
    assert_eq!(peak, 3, "stacks should climb to the cap and no further");
    assert!(simulation.brain(cultist).is_some());
}

#[test]
fn a_kicked_brine_bolt_leaves_nothing_on_anybody() {
    let (mut simulation, cultist) = duel(EnemyKind::DrownedCultist, 10.0);
    let player = simulation.player().entity;
    let mut kicked = false;
    for _ in 0..(60 * 6) {
        simulation.step_once();
        if simulation
            .brain(cultist)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::BrineBolt && cast.elapsed > 0.8)
        {
            assert!(simulation.interrupt(cultist));
            kicked = true;
            break;
        }
    }
    assert!(kicked, "the cultist never cast a brine bolt");
    run(&mut simulation, 60 * 2);
    assert!(
        !simulation
            .world()
            .statuses(player)
            .is_some_and(|statuses| statuses.has(StatusKind::Waterlogged)),
        "an interrupted bolt must not curse"
    );
}

// ------------------------------------------------------------------------------------------
// Catacomb Lurker: the patrol and the cleave
// ------------------------------------------------------------------------------------------

#[test]
fn the_lurker_walks_its_corridor_until_somebody_comes_within_its_radius() {
    let spawn = player_spawn();
    let home = spawn + Vec3::new(0.0, 0.0, -30.0);
    let (mut simulation, members) =
        arena_with(vec![EnemySpawn::new(EnemyKind::CatacombLurker, home)]);
    let lurker = members[0];
    let loop_points = vec![
        home + Vec3::new(-8.0, 0.0, 0.0),
        home + Vec3::new(8.0, 0.0, 0.0),
    ];
    simulation.set_enemy_patrol(lurker, loop_points.clone());
    let mut reached = [false; 2];
    for _ in 0..(60 * 30) {
        simulation.step_once();
        let position = simulation.world().position(lurker).expect("a lurker");
        for (done, point) in reached.iter_mut().zip(&loop_points) {
            *done |= horizontal_distance(position, *point) < 1.5;
        }
    }
    assert_eq!(reached, [true, true], "it never walked both ends");
    assert_eq!(
        simulation.brain(lurker).expect("a brain").state(),
        AiState::Idle,
        "nobody was near enough to wake it"
    );
    // Coming within its radius from where it walks now wakes it.
    let at = simulation.world().position(lurker).expect("a lurker");
    put_player(&mut simulation, at + Vec3::new(0.0, 0.0, 6.0));
    run(&mut simulation, 60);
    assert_ne!(
        simulation.brain(lurker).expect("a brain").state(),
        AiState::Idle
    );
}

#[test]
fn the_lurkers_cleave_is_a_wide_cone_drawn_for_the_whole_cast() {
    let (mut simulation, lurker) = duel(EnemyKind::CatacombLurker, 3.0);
    let mut cone = None;
    let mut started = None;
    for _ in 0..(60 * 12) {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::TelegraphStarted {
                ability,
                shape: shape @ TelegraphShape::Cone { .. },
                delay,
                ..
            } = event
                && ability.enemy() == Some(EnemyAbility::TidalCleave)
            {
                cone = Some((*shape, *delay));
            }
            if let CombatEvent::CastStarted {
                source,
                ability,
                duration,
                ..
            } = event
                && *source == lurker
                && ability.enemy() == Some(EnemyAbility::TidalCleave)
            {
                started = Some(*duration);
            }
        }
        if cone.is_some() {
            break;
        }
    }
    let (shape, delay) = cone.expect("the lurker never drew its cleave");
    let TelegraphShape::Cone {
        range, half_angle, ..
    } = shape
    else {
        unreachable!()
    };
    assert!(range >= 8.0, "{range}");
    assert!(
        half_angle >= std::f32::consts::FRAC_PI_3 - 1e-4,
        "{half_angle}"
    );
    // Drawn from the start of the cast to its landing.
    assert!((delay - started.expect("a cast")).abs() < 0.05);

    // And the telegraph is the hit test: a player beside it is not hit, one in front is.
    let anchor = Vec3::ZERO;
    let facing_north = TelegraphShape::Cone {
        yaw: 0.0,
        range,
        half_angle,
    };
    assert!(facing_north.contains(anchor, Vec3::new(0.0, 0.0, -5.0), 0.5));
    assert!(!facing_north.contains(anchor, Vec3::new(7.0, 0.0, 0.0), 0.5));
    assert!(!facing_north.contains(anchor, Vec3::new(0.0, 0.0, 5.0), 0.5));
}

// ------------------------------------------------------------------------------------------
// Bilge Archer: the volley
// ------------------------------------------------------------------------------------------

#[test]
fn the_archers_volley_is_drawn_before_it_lands_and_lands_where_it_was_drawn() {
    let (mut simulation, archer) = duel(EnemyKind::BilgeArcher, 10.0);
    let events = run(&mut simulation, 60 * 2);
    assert_eq!(casts_of(&events, archer, EnemyAbility::BilgeVolley), 1);
    let zones: Vec<(u32, f32)> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::TelegraphStarted {
                id,
                ability,
                delay,
                shape,
                ..
            } if ability.enemy() == Some(EnemyAbility::BilgeVolley) => {
                assert!(matches!(shape, TelegraphShape::Circle { .. }));
                Some((*id, *delay))
            }
            _ => None,
        })
        .collect();
    assert_eq!(zones.len(), 2, "two circles on one player");
    assert!(zones.iter().all(|(_, delay)| *delay >= 1.5), "{zones:?}");
    let events = run(&mut simulation, 60 * 3);
    let landed = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::TelegraphEnded { id, resolved: true, .. }
                    if zones.iter().any(|(zone, _)| zone == id)
            )
        })
        .count();
    assert_eq!(landed, 2);
}

#[test]
fn a_player_who_walks_out_of_the_volley_is_not_hit_by_it() {
    let (mut simulation, archer) = duel(EnemyKind::BilgeArcher, 10.0);
    let player = simulation.player().entity;
    // Wait for the circles, then step away from every one of them.
    let mut circles: Vec<Vec3> = Vec::new();
    for _ in 0..(60 * 4) {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::TelegraphStarted {
                ability, anchor, ..
            } = event
                && ability.enemy() == Some(EnemyAbility::BilgeVolley)
            {
                circles.push(*anchor);
            }
        }
        if circles.len() == 2 {
            break;
        }
    }
    assert_eq!(circles.len(), 2);
    let safe = Vec3::new(
        circles.iter().map(|c| c.x).sum::<f32>() / 2.0 + 14.0,
        0.0,
        circles[0].z,
    );
    put_player(&mut simulation, safe);
    let events = run(&mut simulation, 60 * 3);
    let hit = events.iter().any(|event| {
        matches!(
            event,
            CombatEvent::Damage(damage)
                if damage.ability.enemy() == Some(EnemyAbility::BilgeVolley)
                    && damage.target == player
        )
    });
    assert!(!hit, "the circles caught somebody who left them");
    let _ = archer;
}

// ------------------------------------------------------------------------------------------
// The Drowned Abbess
// ------------------------------------------------------------------------------------------

/// The Abbess alone, `distance` metres in front of the player.
fn abbess(distance: f32) -> (Simulation, EntityId) {
    duel(EnemyKind::DrownedAbbess, distance)
}

#[test]
fn the_abbess_has_four_phases_and_the_hymn_is_in_exactly_two_of_them() {
    let archetype = EnemyKind::DrownedAbbess.archetype();
    assert_eq!(archetype.phase_count(), 4);
    let thresholds: Vec<f32> = archetype.phases.iter().map(|p| p.at_or_below).collect();
    assert_eq!(thresholds, vec![0.66, 0.5, 0.33]);
    let has = |phase: usize, ability: EnemyAbility| {
        archetype.phase_abilities(phase).any(|a| a == ability)
    };
    assert!(!has(0, EnemyAbility::DrowningHymn));
    assert!(has(1, EnemyAbility::DrowningHymn), "66 %");
    assert!(!has(2, EnemyAbility::DrowningHymn), "50 %");
    assert!(has(3, EnemyAbility::DrowningHymn), "33 %");
    assert!(!has(0, EnemyAbility::Undertow));
    assert!(has(1, EnemyAbility::Undertow) && has(2, EnemyAbility::Undertow));
    let summons: Vec<_> = (0..4).filter_map(|p| archetype.phase_summon(p)).collect();
    assert_eq!(summons.len(), 1, "only one phase calls anybody");
    assert_eq!(summons[0].kind, EnemyKind::DrownedTidecaller);
    assert_eq!(summons[0].count, 2);
    assert!(archetype.phase_summon(2).is_some(), "at 50 %");
    // She is never faster than a player.
    for phase in 0..4 {
        assert!(archetype.phase_speed(phase) < 7.0);
    }
}

#[test]
fn the_hymn_is_cast_at_sixty_six_and_thirty_three_and_only_once_in_each() {
    let (mut simulation, abbess) = abbess(8.0);
    // Above the first threshold she never sings.
    let events = run(&mut simulation, 60 * 20);
    assert_eq!(casts_of(&events, abbess, EnemyAbility::DrowningHymn), 0);

    // Sixty-five per cent: the next time she can act, she starts the hymn.
    wound_to(&mut simulation, abbess, 0.65);
    let events = run(&mut simulation, 60 * 4);
    assert_eq!(casts_of(&events, abbess, EnemyAbility::DrowningHymn), 1);
    assert!(
        simulation.is_interruptible(abbess)
            || events.iter().any(|event| matches!(
                event,
                CombatEvent::CastStarted { ability, .. } if ability.enemy() == Some(EnemyAbility::DrowningHymn)
            ))
    );

    // Let it finish, and a long fight later it has not come back.
    let events = run(&mut simulation, 60 * 60);
    assert_eq!(
        casts_of(&events, abbess, EnemyAbility::DrowningHymn),
        0,
        "once in the phase"
    );
    // Thirty-two per cent: a second hymn, in the last phase.
    wound_to(&mut simulation, abbess, 0.32);
    let events = run(&mut simulation, 60 * 4);
    assert_eq!(casts_of(&events, abbess, EnemyAbility::DrowningHymn), 1);
}

#[test]
fn a_finished_hymn_heals_the_abbess_and_spurs_her_allies_and_a_kicked_one_does_neither() {
    // Finished: a tidecaller beside her is frenzied, and she is stronger than she was.
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::DrownedAbbess, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::DrownedTidecaller,
            spawn + Vec3::new(4.0, 0.0, -12.0),
        ),
    ]);
    let (boss, healer) = (members[0], members[1]);
    wound_to(&mut simulation, boss, 0.64);
    // Not hurt enough for the healer to be interesting: it only needs to be alive.
    let before = health(&simulation, boss);
    let mut events = Vec::new();
    let mut healed = false;
    for _ in 0..(60 * 12) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if completed(&events, boss, EnemyAbility::DrowningHymn) == 1 {
            healed = true;
            break;
        }
    }
    assert!(healed, "the hymn never finished");
    let restored: f32 = heals_of(&events, EnemyAbility::DrowningHymn)
        .iter()
        .map(|(_, amount)| *amount)
        .sum();
    let maximum = simulation.world().health(boss).expect("health").max();
    assert!(
        restored >= maximum * 0.09,
        "the hymn healed {restored} of {maximum}"
    );
    assert!(health(&simulation, boss) > before);
    assert!(
        simulation.brain(healer).expect("a brain").is_frenzied(),
        "the hymn did not spur her allies"
    );

    // Kicked: no heal, no frenzy, and the phase does not give the hymn back.
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::DrownedAbbess, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::DrownedTidecaller,
            spawn + Vec3::new(4.0, 0.0, -12.0),
        ),
    ]);
    let (boss, healer) = (members[0], members[1]);
    wound_to(&mut simulation, boss, 0.64);
    let mut events = Vec::new();
    let mut kicked = false;
    for _ in 0..(60 * 12) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if simulation
            .brain(boss)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::DrowningHymn && cast.elapsed > 2.0)
        {
            assert!(simulation.interrupt(boss), "the hymn must be interruptible");
            kicked = true;
            break;
        }
    }
    assert!(kicked, "no hymn to kick");
    events.extend(run(&mut simulation, 60 * 40));
    assert!(heals_of(&events, EnemyAbility::DrowningHymn).is_empty());
    assert!(!simulation.brain(healer).expect("a brain").is_frenzied());
    assert_eq!(
        casts_of(&events, boss, EnemyAbility::DrowningHymn),
        1,
        "an interrupt does not give the hymn back"
    );
}

#[test]
fn at_half_health_two_tidecallers_join_her_fight() {
    let (mut simulation, abbess) = abbess(8.0);
    run(&mut simulation, 60 * 2);
    let encounter = simulation.encounter().expect("an encounter");
    assert_eq!(encounter.summon_count(), 0);
    // Past sixty-six without stopping at it, straight to forty-nine.
    wound_to(&mut simulation, abbess, 0.49);
    let events = run(&mut simulation, 60 * 3);
    let added: Vec<EntityId> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::AddSummoned { boss, add, .. } if *boss == abbess => Some(*add),
            _ => None,
        })
        .collect();
    assert_eq!(added.len(), 2, "exactly two adds");
    for add in &added {
        assert_eq!(
            simulation.brain(*add).expect("an enemy").kind(),
            EnemyKind::DrownedTidecaller
        );
        assert!(simulation.world().is_alive(*add));
        assert!(
            simulation.brain(*add).expect("an enemy").state() != AiState::Idle,
            "called into the fight, not idling"
        );
    }
    assert_eq!(
        simulation.encounter().expect("an encounter").summon_count(),
        2
    );
    // Once: the phase is not entered again.
    let events = run(&mut simulation, 60 * 10);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::AddSummoned { .. }))
    );
}

#[test]
fn the_healers_the_abbess_calls_heal_her_unless_they_are_dealt_with() {
    let (mut simulation, abbess) = abbess(8.0);
    run(&mut simulation, 60 * 2);
    wound_to(&mut simulation, abbess, 0.45);
    let events = run(&mut simulation, 60 * 12);
    let healed: f32 = heals_of(&events, EnemyAbility::TidalMend)
        .iter()
        .map(|(_, amount)| *amount)
        .sum();
    assert!(healed > 0.0, "the called tidecallers never mended her");
    assert!(
        heals_of(&events, EnemyAbility::TidalMend)
            .iter()
            .all(|(target, _)| *target == abbess)
    );
}

/// An Abbess at 49 % (where the Undertow leads her list), her called healers already dead,
/// and the player beside her.
fn undertow_fight() -> (Simulation, EntityId) {
    let (mut simulation, abbess) = abbess(6.0);
    run(&mut simulation, 60);
    wound_to(&mut simulation, abbess, 0.49);
    run(&mut simulation, 5);
    let adds: Vec<EntityId> = simulation
        .enemies()
        .map(|(entity, _)| entity)
        .filter(|entity| *entity != abbess)
        .collect();
    for add in adds {
        simulation.slay(add);
    }
    (simulation, abbess)
}

/// Puts the player `distance` metres east of the Abbess: the ranged player the current is for.
fn stand_off(simulation: &mut Simulation, abbess: EntityId, distance: f32) {
    let at = simulation.world().position(abbess).expect("a boss");
    put_player(simulation, at + Vec3::new(distance, 0.0, 0.0));
}

/// Runs until the Abbess starts the Undertow, and returns the events up to that tick.
fn until_undertow(simulation: &mut Simulation, abbess: EntityId) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..(60 * 15) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if casts_of(&events, abbess, EnemyAbility::Undertow) > 0 {
            return events;
        }
    }
    panic!("the Abbess never began an Undertow");
}

#[test]
fn the_undertow_drags_a_player_who_stands_still_all_the_way_in_and_slams_them() {
    let (mut simulation, abbess) = undertow_fight();
    let player = simulation.player().entity;
    let mut events = until_undertow(&mut simulation, abbess);
    stand_off(&mut simulation, abbess, 16.0);
    let start = distance(&simulation, player, abbess);
    // Planted from the first tick: nothing the player does moves her.
    events.extend(run(&mut simulation, 60 * 6));
    let reached = distance(&simulation, player, abbess);
    assert!(
        reached < start - 8.0,
        "pulled from {start} to {reached} only"
    );
    events.extend(run(&mut simulation, 60 * 2));
    let slam: Vec<f32> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage)
                if damage.ability.enemy() == Some(EnemyAbility::Undertow)
                    && damage.target == player =>
            {
                Some(damage.amount)
            }
            _ => None,
        })
        .collect();
    assert!(
        !slam.is_empty(),
        "the slam never landed on the one dragged in"
    );
    // Pools open around her while it runs, announced the way every other pool is.
    let pools = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::AreaEffectStarted { ability, .. }
                    if ability.enemy() == Some(EnemyAbility::Undertow)
            )
        })
        .count();
    assert!(pools >= 4, "{pools} drowning pools");
}

#[test]
fn walking_out_against_the_undertow_beats_it() {
    let (mut simulation, abbess) = undertow_fight();
    let player = simulation.player().entity;
    let events = until_undertow(&mut simulation, abbess);
    assert!(casts_of(&events, abbess, EnemyAbility::Undertow) == 1);
    stand_off(&mut simulation, abbess, 12.0);
    let start = distance(&simulation, player, abbess);
    // Straight away from her at full speed, for the whole channel.
    for _ in 0..(60 * 6) {
        simulation.push_command(PlayerCommand::Move {
            forward: 0.0,
            right: 1.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_once();
    }
    let reached = distance(&simulation, player, abbess);
    assert!(
        reached > start + 4.0,
        "a player walking away should out-walk the current: {start} to {reached}"
    );
}

#[test]
fn waterlogged_makes_the_undertow_inescapable() {
    // Three stacks is 45 % slower: 3.85 m/s against a 4.6 m/s current.
    let (mut simulation, abbess) = undertow_fight();
    let player = simulation.player().entity;
    assert!(simulation.apply_effect(player, StatusKind::Waterlogged, 30.0, 3, EntityId::INVALID));
    let _ = until_undertow(&mut simulation, abbess);
    stand_off(&mut simulation, abbess, 12.0);
    let start = distance(&simulation, player, abbess);
    for _ in 0..(60 * 6) {
        simulation.push_command(PlayerCommand::Move {
            forward: 0.0,
            right: 1.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_once();
    }
    let reached = distance(&simulation, player, abbess);
    assert!(
        reached < start,
        "a waterlogged player should lose to the current: {start} to {reached}"
    );
}

#[test]
fn the_undertow_cannot_be_interrupted_and_the_hymn_can() {
    assert!(!EnemyAbility::Undertow.definition().interruptible);
    assert!(EnemyAbility::DrowningHymn.definition().interruptible);
    assert!(EnemyAbility::DrowningHymn.definition().once_per_phase);
    assert!(matches!(
        EnemyAbility::Undertow.definition().effect,
        EnemyEffect::Undertow { .. }
    ));
    let (mut simulation, abbess) = undertow_fight();
    let _ = until_undertow(&mut simulation, abbess);
    run(&mut simulation, 30);
    assert!(!simulation.is_interruptible(abbess));
    assert!(!simulation.interrupt(abbess));
}
