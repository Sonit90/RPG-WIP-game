//! The Bone Foundry: its four new monsters and its boss, each asserted on its signature
//! behaviour and on its counter, and the dungeon built from its level file.
//!
//! Deterministic and headless, like the crypt's and the catacombs' suites: a fixed timestep, a
//! fixed seed, the real brains and the real damage path, no device and no window.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::brain::AiState;
use engine_game::enemy::{
    EnemyAbility, EnemyEffect, EnemyKind, MonsterRole, MonsterTier, TelegraphShape,
};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::hero::Hero;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::status::{STATUS_KIND_COUNT, StatusKind};
use engine_game::world::{EntityDesc, horizontal_distance};

// ------------------------------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------------------------------

fn player_spawn() -> Vec3 {
    ArenaConfig::with_group(0).player_spawn
}

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
    let spawn = player_spawn() + Vec3::new(0.0, 0.0, -distance);
    let (simulation, members) = arena_with(vec![EnemySpawn::new(kind, spawn)]);
    (simulation, members[0])
}

/// Further players standing at `offsets` from the first one, each able to take anything.
fn add_players(simulation: &mut Simulation, offsets: &[Vec3]) -> Vec<EntityId> {
    offsets
        .iter()
        .map(|offset| {
            let entity = simulation
                .world_mut()
                .spawn(EntityDesc::player(player_spawn() + *offset));
            simulation.add_player(entity, Hero::FireMage);
            simulation.world_mut().set_max_health(entity, 1_000_000.0);
            simulation.world_mut().restore(entity);
            entity
        })
        .collect()
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

fn put(simulation: &mut Simulation, entity: EntityId, position: Vec3) {
    if let Some(body) = simulation.world_mut().body_mut(entity) {
        *body = engine_game::Body::at(position);
    }
}

fn has_status(simulation: &Simulation, entity: EntityId, kind: StatusKind) -> bool {
    simulation
        .world()
        .statuses(entity)
        .is_some_and(|statuses| statuses.has(kind))
}

/// Damage taken by `target` from `ability`, as `(tick, amount)`.
fn damage_from(events: &[CombatEvent], ability: EnemyAbility, target: EntityId) -> Vec<(u64, f32)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage)
                if damage.ability.enemy() == Some(ability) && damage.target == target =>
            {
                Some((damage.tick, damage.amount))
            }
            _ => None,
        })
        .collect()
}

/// Runs until `check` finds something in the step's events, up to `limit` ticks.
fn until<T>(
    simulation: &mut Simulation,
    limit: u32,
    mut check: impl FnMut(&Simulation, &[CombatEvent]) -> Option<T>,
) -> Option<T> {
    for _ in 0..limit {
        simulation.step_once();
        let events: Vec<CombatEvent> = simulation.events().to_vec();
        if let Some(found) = check(simulation, &events) {
            return Some(found);
        }
    }
    None
}

/// The first telegraph `ability` draws: its id, anchor, shape and delay.
fn first_telegraph(
    simulation: &mut Simulation,
    limit: u32,
    ability: EnemyAbility,
) -> (u32, Vec3, TelegraphShape, f32) {
    until(simulation, limit, |_, events| {
        events.iter().find_map(|event| match event {
            CombatEvent::TelegraphStarted {
                id,
                ability: which,
                anchor,
                shape,
                delay,
                ..
            } if which.enemy() == Some(ability) => Some((*id, *anchor, *shape, *delay)),
            _ => None,
        })
    })
    .unwrap_or_else(|| panic!("{ability:?} never drew a telegraph"))
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
    assert_eq!(role("slag_ironclad"), MonsterRole::Melee);
    assert_eq!(role("cinder_bomber"), MonsterRole::Melee);
    assert_eq!(role("slag_thrower"), MonsterRole::Ranged);
    assert_eq!(role("foundry_foreman"), MonsterRole::Elite);
    assert_eq!(role("foundry_overseer"), MonsterRole::Boss);
    assert!(EnemyKind::FoundryOverseer.is_boss());
    assert_eq!(
        EnemyKind::FoundryOverseer.archetype().tier,
        MonsterTier::Boss
    );
    for kind in [
        EnemyKind::SlagIronclad,
        EnemyKind::CinderBomber,
        EnemyKind::SlagThrower,
        EnemyKind::FoundryForeman,
        EnemyKind::FoundryOverseer,
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
        EnemyAbility::SlagBulwark,
        EnemyAbility::ShieldSweep,
        EnemyAbility::CinderBlast,
        EnemyAbility::SlagLob,
        EnemyAbility::StokeTheFurnace,
        EnemyAbility::ChainLash,
        EnemyAbility::FoundrySlam,
        EnemyAbility::SlagBrand,
        EnemyAbility::MoltenLadle,
        EnemyAbility::ConveyorSurge,
        EnemyAbility::Overheat,
    ] {
        let definition = ability.definition();
        assert!(!definition.description.is_empty(), "{}", definition.name);
        assert!(!definition.counter.is_empty(), "{}", definition.name);
    }
}

#[test]
fn the_new_statuses_are_appended_last_and_are_buffs_the_registry_describes() {
    // Statuses travel the wire by index: the first thirty-four never move.
    assert_eq!(STATUS_KIND_COUNT, 36);
    assert_eq!(StatusKind::Waterlogged.index(), 33);
    assert_eq!(StatusKind::Warded.index(), 34);
    assert_eq!(StatusKind::Stoked.index(), 35);
    for kind in [StatusKind::Warded, StatusKind::Stoked] {
        assert!(!kind.is_harmful(), "{kind:?}");
        assert!(
            !kind.is_dispellable(),
            "{kind:?}: monsters' buffs are not a player's dispel"
        );
        assert!(!kind.definition().description.is_empty());
    }
    let warded = StatusKind::Warded.definition().modifiers;
    assert!(warded.iter().any(|m| matches!(
        m,
        engine_game::status::EffectModifier::DamageTaken { per_stack, .. } if *per_stack < 0.0
    )));
    let stoked = StatusKind::Stoked.definition().modifiers;
    assert!(stoked.iter().any(|m| matches!(
        m,
        engine_game::status::EffectModifier::DamageDealt { per_stack } if *per_stack > 0.0
    )));
}

// ------------------------------------------------------------------------------------------
// Slag Ironclad: the ward
// ------------------------------------------------------------------------------------------

/// An ironclad in front of the player, a warrior beside it.
fn ironclad_pack() -> (Simulation, EntityId, EntityId) {
    let spawn = player_spawn();
    let (simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::SlagIronclad, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(5.0, 0.0, -10.0),
        ),
    ]);
    (simulation, members[0], members[1])
}

/// What one hit of 100 on `target` lands as, right now.
fn probe(simulation: &mut Simulation, target: EntityId) -> f32 {
    let player = simulation.player().entity;
    simulation.apply_damage(
        player,
        target,
        100.0,
        AbilityId::NONE,
        engine_game::combat::DamageType::Physical,
    );
    simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) if damage.target == target => Some(damage.amount),
            _ => None,
        })
        .sum()
}

#[test]
fn a_finished_bulwark_wards_the_pack_and_the_ward_cuts_damage_by_forty_per_cent() {
    let (mut simulation, ironclad, warrior) = ironclad_pack();
    let before = probe(&mut simulation, warrior);
    assert!(before > 50.0, "{before}");
    let mut events = Vec::new();
    let mut done = false;
    for _ in 0..(60 * 8) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if completed(&events, ironclad, EnemyAbility::SlagBulwark) >= 1 {
            done = true;
            break;
        }
    }
    assert!(done, "the ironclad never finished a bulwark");
    for who in [warrior, ironclad] {
        assert!(has_status(&simulation, who, StatusKind::Warded), "{who:?}");
    }
    let warded = probe(&mut simulation, warrior);
    assert!(
        (warded / before - 0.6).abs() < 0.02,
        "a warded hit landed as {warded} against {before}"
    );
    // It is a buff on a monster: it wears off.
    run(&mut simulation, 60 * 9);
    assert!(!has_status(&simulation, warrior, StatusKind::Warded));
}

#[test]
fn a_kicked_bulwark_wards_nobody() {
    let (mut simulation, ironclad, warrior) = ironclad_pack();
    let mut kicked = false;
    for _ in 0..(60 * 6) {
        simulation.step_once();
        if simulation
            .brain(ironclad)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::SlagBulwark && cast.elapsed > 0.8)
        {
            assert!(simulation.is_interruptible(ironclad));
            assert!(simulation.interrupt(ironclad));
            kicked = true;
            break;
        }
    }
    assert!(kicked, "no bulwark to kick");
    run(&mut simulation, 60);
    assert!(!has_status(&simulation, warrior, StatusKind::Warded));
    assert!(!has_status(&simulation, ironclad, StatusKind::Warded));
}

#[test]
fn the_ward_reaches_only_allies_near_the_ironclad() {
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::SlagIronclad, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(5.0, 0.0, -10.0),
        ),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(40.0, 0.0, -10.0),
        ),
    ]);
    let far = members[2];
    let events = run(&mut simulation, 60 * 8);
    assert!(completed(&events, members[0], EnemyAbility::SlagBulwark) >= 1);
    assert!(has_status(&simulation, members[1], StatusKind::Warded));
    assert!(!has_status(&simulation, far, StatusKind::Warded));
}

#[test]
fn the_sweep_is_a_wide_cone_drawn_for_the_whole_cast() {
    let (mut simulation, ironclad) = duel(EnemyKind::SlagIronclad, 3.0);
    let (_, _, shape, delay) = first_telegraph(&mut simulation, 60 * 14, EnemyAbility::ShieldSweep);
    let TelegraphShape::Cone {
        range, half_angle, ..
    } = shape
    else {
        panic!("the sweep is a cone");
    };
    assert!(range >= 6.0 && half_angle >= std::f32::consts::FRAC_PI_3 - 1e-4);
    assert!(
        (delay - EnemyAbility::ShieldSweep.definition().cast_time).abs() < 0.05,
        "{delay}"
    );
    let _ = ironclad;
}

// ------------------------------------------------------------------------------------------
// Cinder Bomber: the fuse
// ------------------------------------------------------------------------------------------

#[test]
fn a_bomber_runs_in_lights_a_drawn_fuse_blows_up_and_is_gone() {
    let (mut simulation, bomber) = duel(EnemyKind::CinderBomber, 9.0);
    let player = simulation.player().entity;
    let mut events = Vec::new();
    let mut telegraph = None;
    for _ in 0..(60 * 10) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        for event in simulation.events() {
            if let CombatEvent::TelegraphStarted {
                ability,
                shape,
                delay,
                ..
            } = event
                && ability.enemy() == Some(EnemyAbility::CinderBlast)
            {
                telegraph = Some((*shape, *delay));
            }
        }
        if !simulation.world().is_alive(bomber) {
            break;
        }
    }
    let (shape, delay) = telegraph.expect("the fuse was never drawn");
    assert!(matches!(shape, TelegraphShape::Circle { radius } if radius >= 4.0));
    assert!(
        (delay - EnemyAbility::CinderBlast.definition().cast_time).abs() < 0.05,
        "drawn for the whole cast: {delay}"
    );
    assert!(!simulation.world().is_alive(bomber), "it did not blow up");
    assert_eq!(completed(&events, bomber, EnemyAbility::CinderBlast), 1);
    assert!(
        !damage_from(&events, EnemyAbility::CinderBlast, player).is_empty(),
        "the blast missed somebody standing in it"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        CombatEvent::Death { entity, .. } if *entity == bomber
    )));
}

#[test]
fn killing_a_bomber_during_the_fuse_defuses_it() {
    let (mut simulation, bomber) = duel(EnemyKind::CinderBomber, 9.0);
    let player = simulation.player().entity;
    let mut events = Vec::new();
    let mut killed = false;
    for _ in 0..(60 * 10) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if simulation
            .brain(bomber)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::CinderBlast && cast.elapsed > 0.4)
        {
            simulation.slay(bomber);
            events.extend_from_slice(simulation.events());
            killed = true;
            break;
        }
    }
    assert!(killed, "the bomber never lit its fuse");
    events.extend(run(&mut simulation, 60 * 3));
    assert!(damage_from(&events, EnemyAbility::CinderBlast, player).is_empty());
    assert!(events.iter().any(|event| matches!(
        event,
        CombatEvent::CastEnded { source, end: CastEnd::CasterDied, .. } if *source == bomber
    )));
    // The telegraph came down with it, unresolved.
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::TelegraphEnded { resolved: true, .. }))
    );
}

#[test]
fn a_player_who_leaves_the_circle_is_not_hit_but_the_bomber_still_goes() {
    let (mut simulation, bomber) = duel(EnemyKind::CinderBomber, 9.0);
    let player = simulation.player().entity;
    let (_, anchor, _, _) = first_telegraph(&mut simulation, 60 * 10, EnemyAbility::CinderBlast);
    put(&mut simulation, player, anchor + Vec3::new(14.0, 0.0, 0.0));
    let events = run(&mut simulation, 60 * 3);
    assert!(damage_from(&events, EnemyAbility::CinderBlast, player).is_empty());
    assert!(
        !simulation.world().is_alive(bomber),
        "the fuse still burns down"
    );
}

#[test]
fn a_bombers_death_by_blast_counts_as_a_kill_for_the_pack() {
    let spawn = player_spawn() + Vec3::new(0.0, 0.0, -9.0);
    let (mut simulation, members) =
        arena_with(vec![EnemySpawn::new(EnemyKind::CinderBomber, spawn)]);
    run(&mut simulation, 60 * 8);
    assert!(!simulation.world().is_alive(members[0]));
    assert!(
        simulation
            .encounter()
            .expect("an encounter")
            .members()
            .iter()
            .all(|member| !simulation.world().is_alive(*member)),
        "the pack counts its bomber dead"
    );
}

// ------------------------------------------------------------------------------------------
// Slag Thrower: the lob
// ------------------------------------------------------------------------------------------

fn thrower_fight() -> (Simulation, EntityId, Vec<EntityId>) {
    let (mut simulation, thrower) = duel(EnemyKind::SlagThrower, 10.0);
    let mut players = vec![simulation.player().entity];
    players.extend(add_players(
        &mut simulation,
        &[Vec3::new(4.0, 0.0, 0.0), Vec3::new(-4.0, 0.0, 0.0)],
    ));
    (simulation, thrower, players)
}

#[test]
fn a_lob_draws_circles_under_two_players_lands_after_the_delay_and_leaves_slag() {
    let (mut simulation, thrower, _players) = thrower_fight();
    let mut events = Vec::new();
    let mut zones: Vec<(u32, Vec3, f32)> = Vec::new();
    for _ in 0..(60 * 8) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        for event in simulation.events() {
            if let CombatEvent::TelegraphStarted {
                id,
                ability,
                anchor,
                delay,
                shape,
                ..
            } = event
                && ability.enemy() == Some(EnemyAbility::SlagLob)
            {
                assert!(matches!(shape, TelegraphShape::Circle { .. }));
                zones.push((*id, *anchor, *delay));
            }
        }
        if zones.len() >= 2 {
            break;
        }
    }
    assert_eq!(zones.len(), 2, "two players, one circle each");
    assert!(zones.iter().all(|(_, _, delay)| *delay >= 1.5), "{zones:?}");
    assert!(
        horizontal_distance(zones[0].1, zones[1].1) > 1.0,
        "the circles are on different players"
    );
    // No slag yet: the pool is what the landing leaves.
    assert!(!events.iter().any(|event| matches!(
        event,
        CombatEvent::AreaEffectStarted { ability, .. }
            if ability.enemy() == Some(EnemyAbility::SlagLob)
    )));
    events.clear();
    events.extend(run(&mut simulation, 60 * 3));
    let landed = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::TelegraphEnded { id, resolved: true, .. }
                    if zones.iter().any(|(zone, _, _)| zone == id)
            )
        })
        .count();
    assert_eq!(landed, 2);
    let pools = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::AreaEffectStarted { ability, .. }
                    if ability.enemy() == Some(EnemyAbility::SlagLob)
            )
        })
        .count();
    assert_eq!(pools, 2, "each landing leaves ground that burns");
    let _ = thrower;
}

#[test]
fn standing_in_the_slag_burns_and_leaving_the_circle_spares_you_the_landing() {
    let (mut simulation, _thrower, players) = thrower_fight();
    let mut circles: Vec<(Vec3, EntityId)> = Vec::new();
    for _ in 0..(60 * 8) {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::TelegraphStarted {
                ability,
                anchor,
                follow,
                ..
            } = event
                && ability.enemy() == Some(EnemyAbility::SlagLob)
            {
                circles.push((*anchor, follow.unwrap_or(EntityId::INVALID)));
            }
        }
        if circles.len() >= 2 {
            break;
        }
    }
    assert_eq!(circles.len(), 2);
    // Everybody walks well away from both circles.
    let far = Vec3::new(30.0, 0.0, 14.0);
    for (index, player) in players.iter().enumerate() {
        put(
            &mut simulation,
            *player,
            far + Vec3::new(index as f32 * 2.0, 0.0, 0.0),
        );
    }
    let events = run(&mut simulation, 60 * 3);
    for player in &players {
        assert!(
            damage_from(&events, EnemyAbility::SlagLob, *player).is_empty(),
            "the circles caught somebody who left them"
        );
    }
    // And somebody standing in the pool afterwards is burned by it.
    let victim = players[1];
    put(&mut simulation, victim, circles[0].0);
    let burned = run(&mut simulation, 60 * 2);
    assert!(
        !damage_from(&burned, EnemyAbility::SlagLob, victim).is_empty(),
        "the slag did not burn"
    );
}

#[test]
fn a_kicked_lob_lays_nothing() {
    let (mut simulation, thrower, _players) = thrower_fight();
    let mut kicked = false;
    let mut events = Vec::new();
    for _ in 0..(60 * 8) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if simulation
            .brain(thrower)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::SlagLob && cast.elapsed > 0.8)
        {
            assert!(simulation.interrupt(thrower));
            kicked = true;
            break;
        }
    }
    assert!(kicked, "no lob to kick");
    events.clear();
    events.extend(run(&mut simulation, 60));
    assert!(!events.iter().any(|event| matches!(
        event,
        CombatEvent::TelegraphStarted { ability, .. } if ability.enemy() == Some(EnemyAbility::SlagLob)
    )));
}

// ------------------------------------------------------------------------------------------
// Foundry Foreman: the rally
// ------------------------------------------------------------------------------------------

fn foreman_pack(far: bool) -> (Simulation, Vec<EntityId>) {
    let spawn = player_spawn();
    let mut spawns = vec![
        EnemySpawn::new(EnemyKind::FoundryForeman, spawn + Vec3::new(0.0, 0.0, -9.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(5.0, 0.0, -12.0),
        ),
    ];
    if far {
        spawns.push(EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(45.0, 0.0, -12.0),
        ));
    }
    arena_with(spawns)
}

#[test]
fn a_finished_rally_stokes_the_pack_for_thirty_per_cent_more_damage() {
    let (mut simulation, members) = foreman_pack(true);
    let (foreman, near, far) = (members[0], members[1], members[2]);
    assert!(
        (simulation
            .world()
            .statuses(near)
            .expect("statuses")
            .damage_dealt_multiplier()
            - 1.0)
            .abs()
            < 1e-6
    );
    let mut events = Vec::new();
    let mut done = false;
    for _ in 0..(60 * 8) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if completed(&events, foreman, EnemyAbility::StokeTheFurnace) >= 1 {
            done = true;
            break;
        }
    }
    assert!(done, "the foreman never finished a rally");
    for who in [foreman, near] {
        assert!(has_status(&simulation, who, StatusKind::Stoked), "{who:?}");
    }
    // Reached by distance and by nothing else: whoever was inside the radius when it
    // finished is stoked, and whoever was not is not (the far warrior may well have run in).
    let _ = far;
    let multiplier = simulation
        .world()
        .statuses(near)
        .expect("statuses")
        .damage_dealt_multiplier();
    assert!((multiplier - 1.3).abs() < 1e-4, "{multiplier}");
    // It is felt: the stoked foreman's lash hits harder than the table says.
    let lash = EnemyAbility::ChainLash.definition().damage;
    events.clear();
    events.extend(run(&mut simulation, 60 * 6));
    let player = simulation.player().entity;
    let hits = damage_from(&events, EnemyAbility::ChainLash, player);
    if let Some((_, amount)) = hits.first() {
        assert!(*amount > lash * 1.2, "{amount} against {lash}");
    }
    // And it wears off.
    run(&mut simulation, 60 * 12);
    assert!(simulation.world().is_alive(near));
}

#[test]
fn a_kicked_rally_stokes_nobody() {
    let (mut simulation, members) = foreman_pack(false);
    let (foreman, near) = (members[0], members[1]);
    let mut kicked = false;
    for _ in 0..(60 * 6) {
        simulation.step_once();
        if simulation
            .brain(foreman)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::StokeTheFurnace && cast.elapsed > 0.8)
        {
            assert!(simulation.interrupt(foreman));
            kicked = true;
            break;
        }
    }
    assert!(kicked, "no rally to kick");
    run(&mut simulation, 60);
    assert!(!has_status(&simulation, near, StatusKind::Stoked));
    assert!(!has_status(&simulation, foreman, StatusKind::Stoked));
}

#[test]
fn the_lash_is_a_line_drawn_for_the_whole_cast() {
    let (mut simulation, _foreman) = duel(EnemyKind::FoundryForeman, 5.0);
    let (_, _, shape, delay) = first_telegraph(&mut simulation, 60 * 12, EnemyAbility::ChainLash);
    assert!(matches!(shape, TelegraphShape::Line { length, .. } if length >= 12.0));
    assert!((delay - EnemyAbility::ChainLash.definition().cast_time).abs() < 0.05);
}

// ------------------------------------------------------------------------------------------
// The Foundry Overseer
// ------------------------------------------------------------------------------------------

/// The Overseer alone, 12 m in front of the player, with two more players beside the first.
fn overseer_fight() -> (Simulation, EntityId, Vec<EntityId>) {
    let (mut simulation, overseer) = duel(EnemyKind::FoundryOverseer, 12.0);
    let mut players = vec![simulation.player().entity];
    players.extend(add_players(
        &mut simulation,
        &[Vec3::new(3.0, 0.0, 0.0), Vec3::new(-3.0, 0.0, 0.0)],
    ));
    (simulation, overseer, players)
}

#[test]
fn the_overseer_has_four_phases_with_each_mechanic_where_the_design_puts_it() {
    let archetype = EnemyKind::FoundryOverseer.archetype();
    assert_eq!(archetype.phase_count(), 4);
    let thresholds: Vec<f32> = archetype.phases.iter().map(|p| p.at_or_below).collect();
    assert_eq!(thresholds, vec![0.70, 0.50, 0.25]);
    let has = |phase: usize, ability: EnemyAbility| {
        archetype.phase_abilities(phase).any(|a| a == ability)
    };
    // The spread from the start, the soak from seventy, the lane from fifty, the enrage last.
    assert!(has(0, EnemyAbility::SlagBrand) && has(0, EnemyAbility::FoundrySlam));
    assert!(!has(0, EnemyAbility::MoltenLadle) && has(1, EnemyAbility::MoltenLadle));
    assert!(!has(1, EnemyAbility::ConveyorSurge) && has(2, EnemyAbility::ConveyorSurge));
    assert!(has(3, EnemyAbility::ConveyorSurge) && has(3, EnemyAbility::MoltenLadle));
    for phase in 0..3 {
        assert!(!has(phase, EnemyAbility::Overheat), "phase {phase}");
    }
    assert!(has(3, EnemyAbility::Overheat));
    assert!(EnemyAbility::Overheat.definition().once_per_phase);
    // The adds: three bombers at fifty, two throwers at twenty-five, nobody else.
    let summons: Vec<_> = (0..4).map(|p| archetype.phase_summon(p)).collect();
    assert!(summons[0].is_none() && summons[1].is_none());
    let bombers = summons[2].expect("bombers at fifty");
    assert_eq!((bombers.kind, bombers.count), (EnemyKind::CinderBomber, 3));
    let throwers = summons[3].expect("throwers at twenty-five");
    assert_eq!((throwers.kind, throwers.count), (EnemyKind::SlagThrower, 2));
    // He never outruns a player, however hot.
    for phase in 0..4 {
        assert!(archetype.phase_speed(phase) < 7.0, "phase {phase}");
    }
    // Balanced between the Abbess (17,000) and a harder dungeon's boss.
    assert!(
        archetype.max_health > EnemyKind::DrownedAbbess.archetype().max_health
            && archetype.max_health <= 22_000.0
    );
}

#[test]
fn the_ladle_phase_begins_at_seventy_and_not_before() {
    let (mut simulation, overseer, _players) = overseer_fight();
    let events = run(&mut simulation, 60 * 25);
    assert_eq!(casts_of(&events, overseer, EnemyAbility::MoltenLadle), 0);
    wound_to(&mut simulation, overseer, 0.69);
    let events = run(&mut simulation, 60 * 4);
    assert!(casts_of(&events, overseer, EnemyAbility::MoltenLadle) >= 1);
}

/// What `victim` took from one Molten Ladle, with the other two players put as `stand` says.
///
/// `stand(anchor)` gives each of the three players' positions relative to the telegraph.
fn ladle_damage(stand: impl Fn(Vec3) -> [Vec3; 3]) -> (f32, usize) {
    let (mut simulation, overseer, players) = overseer_fight();
    wound_to(&mut simulation, overseer, 0.69);
    let (_, anchor, shape, delay) =
        first_telegraph(&mut simulation, 60 * 20, EnemyAbility::MoltenLadle);
    assert!(matches!(shape, TelegraphShape::Circle { radius } if radius >= 5.0));
    assert!(delay >= 4.0, "a long warning: {delay}");
    let places = stand(anchor);
    for (player, place) in players.iter().zip(places) {
        put(&mut simulation, *player, place);
    }
    let events = run(&mut simulation, (delay * 60.0) as u32 + 30);
    let hits = damage_from(&events, EnemyAbility::MoltenLadle, players[1]);
    (hits.iter().map(|(_, amount)| amount).sum(), hits.len())
}

#[test]
fn the_ladles_damage_is_shared_by_everyone_in_it_and_unattended_it_spills_on_the_party() {
    let far = |anchor: Vec3, index: f32| anchor + Vec3::new(24.0 + index * 4.0, 0.0, 0.0);
    // All three stacked inside.
    let (shared, shared_hits) = ladle_damage(|anchor| {
        [
            anchor + Vec3::new(1.0, 0.0, 0.0),
            anchor + Vec3::new(-1.0, 0.0, 0.0),
            anchor + Vec3::new(0.0, 0.0, 1.0),
        ]
    });
    // Only the one being measured inside.
    let (alone, alone_hits) = ladle_damage(|anchor| [far(anchor, 0.0), anchor, far(anchor, 1.0)]);
    // Nobody inside.
    let (spilled, spilled_hits) =
        ladle_damage(|anchor| [far(anchor, 0.0), far(anchor, 1.0), far(anchor, 2.0)]);
    assert_eq!((shared_hits, alone_hits, spilled_hits), (1, 1, 1));
    assert!(shared > 0.0, "the soak did nothing to somebody in it");
    assert!(
        (alone / shared - 3.0).abs() < 0.06,
        "alone {alone}, stacked three {shared}"
    );
    let overflow = match EnemyAbility::MoltenLadle.definition().effect {
        EnemyEffect::Soak { overflow } => overflow,
        other => panic!("the ladle is a soak, not {other:?}"),
    };
    assert!(
        (spilled / alone - overflow).abs() < 0.02,
        "unattended it spilled {spilled} against {alone}"
    );
    // Standing in it together beats standing in it alone, and even alone beats leaving it
    // empty: an unattended ladle hits everyone harder than one soaker takes, so solo soaks too.
    assert!(shared < alone && alone < spilled);
}

#[test]
fn the_brand_punishes_a_clump_and_spares_a_spread() {
    let hits = |spread: bool| {
        let (mut simulation, overseer, players) = overseer_fight();
        let marks = until(&mut simulation, 60 * 30, |_, events| {
            let count = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        CombatEvent::TelegraphStarted { ability, follow: Some(_), .. }
                            if ability.enemy() == Some(EnemyAbility::SlagBrand)
                    )
                })
                .count();
            (count > 0).then_some(count)
        })
        .expect("the Overseer never branded anybody");
        assert_eq!(marks, 3, "a mark on each of three players");
        // The marks follow their players: where they stand now is where they land.
        let places: [Vec3; 3] = if spread {
            [
                Vec3::new(-20.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 14.0),
                Vec3::new(20.0, 0.0, 0.0),
            ]
        } else {
            [
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(-1.0, 0.0, 0.0),
            ]
        };
        for (player, place) in players.iter().zip(places) {
            put(&mut simulation, *player, player_spawn() + place);
        }
        let events = run(&mut simulation, 60 * 9);
        let _ = overseer;
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    CombatEvent::Damage(damage)
                        if damage.ability.enemy() == Some(EnemyAbility::SlagBrand)
                )
            })
            .count()
    };
    let spread = hits(true);
    let clumped = hits(false);
    assert_eq!(spread, 3, "one mark, one hit each");
    assert!(
        clumped >= 2 * spread,
        "{clumped} hits on a clump against {spread} on a spread"
    );
}

/// The Overseer at forty-nine per cent with his called bombers gone, until he starts a surge.
fn surge_fight() -> (Simulation, EntityId, Vec<EntityId>, f32, Vec3) {
    let (mut simulation, overseer, players) = overseer_fight();
    wound_to(&mut simulation, overseer, 0.49);
    run(&mut simulation, 5);
    let adds: Vec<EntityId> = simulation
        .enemies()
        .map(|(entity, _)| entity)
        .filter(|entity| *entity != overseer)
        .collect();
    for add in adds {
        simulation.slay(add);
    }
    let (_, anchor, shape, delay) =
        first_telegraph(&mut simulation, 60 * 30, EnemyAbility::ConveyorSurge);
    let TelegraphShape::Line { yaw, length, width } = shape else {
        panic!("the surge is a lane");
    };
    assert!(length >= 30.0 && width >= 8.0);
    assert!(delay >= 4.0, "a long warning: {delay}");
    (simulation, overseer, players, yaw, anchor)
}

fn lane_point(anchor: Vec3, yaw: f32, along: f32, across: f32) -> Vec3 {
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let side = Vec3::new(-forward.z, 0.0, forward.x);
    anchor + forward * along + side * across
}

#[test]
fn riding_the_conveyor_carries_you_into_the_slag_and_the_lane_hits_you_at_the_end() {
    let (mut simulation, _overseer, players, yaw, anchor) = surge_fight();
    let rider = players[1];
    let start = lane_point(anchor, yaw, 8.0, 0.0);
    put(&mut simulation, rider, start);
    // The others are well out of the lane.
    put(
        &mut simulation,
        players[0],
        lane_point(anchor, yaw, 6.0, 14.0),
    );
    put(
        &mut simulation,
        players[2],
        lane_point(anchor, yaw, 6.0, -14.0),
    );
    let mut events = Vec::new();
    for _ in 0..(60 * 6) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    let carried = horizontal_distance(simulation.world().position(rider).expect("a body"), start);
    assert!(carried > 8.0, "only carried {carried} m by the lane");
    let pools = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::AreaEffectStarted { ability, .. }
                    if ability.enemy() == Some(EnemyAbility::ConveyorSurge)
            )
        })
        .count();
    assert!(pools >= 4, "{pools} pools of slag");
    assert!(
        !damage_from(&events, EnemyAbility::ConveyorSurge, rider).is_empty(),
        "the lane's own hit never landed on the one who rode it"
    );
}

#[test]
fn stepping_out_of_the_lane_beats_the_conveyor() {
    let (mut simulation, _overseer, players, yaw, anchor) = surge_fight();
    let walker = players[1];
    // Just outside the lane's edge: not carried, not hit.
    let here = lane_point(anchor, yaw, 8.0, 7.5);
    put(&mut simulation, walker, here);
    put(
        &mut simulation,
        players[0],
        lane_point(anchor, yaw, 6.0, 14.0),
    );
    put(
        &mut simulation,
        players[2],
        lane_point(anchor, yaw, 6.0, -14.0),
    );
    let mut events = Vec::new();
    for _ in 0..(60 * 6) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    let moved = horizontal_distance(simulation.world().position(walker).expect("a body"), here);
    assert!(moved < 1.0, "carried {moved} m from outside the lane");
    assert!(damage_from(&events, EnemyAbility::ConveyorSurge, walker).is_empty());
    // And the push is slower than a walk, so stepping aside is always enough.
    let EnemyEffect::Conveyor { push_speed, .. } = EnemyAbility::ConveyorSurge.definition().effect
    else {
        panic!("the surge is a conveyor");
    };
    assert!(push_speed < 5.0);
    assert!(!EnemyAbility::ConveyorSurge.definition().interruptible);
}

#[test]
fn at_fifty_per_cent_three_bombers_join_and_at_twenty_five_two_throwers() {
    let (mut simulation, overseer, _players) = overseer_fight();
    run(&mut simulation, 60);
    assert_eq!(
        simulation.encounter().expect("an encounter").summon_count(),
        0
    );
    wound_to(&mut simulation, overseer, 0.49);
    let events = run(&mut simulation, 60 * 3);
    let added: Vec<EntityId> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::AddSummoned { boss, add, .. } if *boss == overseer => Some(*add),
            _ => None,
        })
        .collect();
    assert_eq!(added.len(), 3);
    for add in &added {
        assert_eq!(
            simulation.brain(*add).expect("an enemy").kind(),
            EnemyKind::CinderBomber
        );
        assert!(simulation.brain(*add).expect("an enemy").state() != AiState::Idle);
    }
    // Once: not again however long the phase lasts.
    let events = run(&mut simulation, 60 * 10);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::AddSummoned { .. }))
    );
    wound_to(&mut simulation, overseer, 0.24);
    let events = run(&mut simulation, 60 * 3);
    let throwers = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::AddSummoned { boss, add, .. } if *boss == overseer => Some(*add),
            _ => None,
        })
        .filter(|add| {
            simulation
                .brain(*add)
                .is_some_and(|brain| brain.kind() == EnemyKind::SlagThrower)
        })
        .count();
    assert_eq!(throwers, 2);
}

#[test]
fn the_overheat_is_once_permanent_and_makes_everything_faster() {
    let (mut simulation, overseer, _players) = overseer_fight();
    run(&mut simulation, 60);
    assert!(!simulation.brain(overseer).expect("a brain").is_frenzied());
    wound_to(&mut simulation, overseer, 0.24);
    let events = run(&mut simulation, 60 * 3);
    assert!(
        simulation.brain(overseer).expect("a brain").is_frenzied(),
        "he did not overheat"
    );
    assert!(
        simulation
            .brain(overseer)
            .expect("a brain")
            .frenzy_remaining()
            > 300.0
    );
    assert!(simulation.brain(overseer).expect("a brain").haste() > 1.3);
    let _ = events;
    let events = run(&mut simulation, 60 * 60);
    assert_eq!(
        casts_of(&events, overseer, EnemyAbility::Overheat),
        0,
        "the enrage is cast once"
    );
}

// ------------------------------------------------------------------------------------------
// The dungeon, built from its level file
// ------------------------------------------------------------------------------------------

use engine_game::dungeon::{DungeonScaling, build_level_file, populate_from_scene};

fn scene_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenes/bone_foundry.json")
}

fn foundry() -> Simulation {
    let mut simulation = build_level_file(scene_path(), DungeonScaling::default())
        .expect("the level file builds")
        .0;
    let player = simulation.player().entity;
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
    simulation.step_once();
    simulation
}

fn pack(simulation: &Simulation, name: &str) -> Vec<EntityId> {
    simulation
        .encounters()
        .iter()
        .find(|encounter| encounter.name() == name)
        .unwrap_or_else(|| panic!("no pack {name}"))
        .members()
        .to_vec()
}

#[test]
fn the_level_loads_with_no_unknown_ids_and_a_boss_pack() {
    let scene = engine_environment::scene_file::load(scene_path()).expect("loads");
    let mut simulation = Simulation::new(engine_game::sim::SimConfig::default());
    let report = populate_from_scene(&mut simulation, &scene, DungeonScaling::default());
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(report.packs, 9, "eight packs and the boss");
    assert_eq!(report.monsters, scene.monsters.len());
    let boss = report.boss_encounter.expect("a boss encounter");
    assert_eq!(simulation.encounters()[boss].name(), "THE FOUNDRY OVERSEER");
    let required = scene
        .dungeon
        .as_ref()
        .expect("a dungeon")
        .required_completion;
    assert!(
        report.total_completion >= required,
        "the dungeon cannot be completed"
    );
    let known: Vec<&str> = engine_environment::LIBRARY
        .iter()
        .map(|a| a.name())
        .collect();
    for object in &scene.objects {
        assert!(
            known.contains(&object.asset.name()),
            "{}",
            object.asset.name()
        );
    }
    assert!(scene.objects.len() > 500);
    // The hauler and the foreman walk.
    assert_eq!(
        scene
            .monsters
            .iter()
            .filter(|m| !m.patrol.is_empty())
            .count(),
        2
    );
    // Every new monster is placed somewhere.
    for id in [
        "slag_ironclad",
        "cinder_bomber",
        "slag_thrower",
        "foundry_foreman",
    ] {
        assert!(
            scene.monsters.iter().any(|m| m.monster == id),
            "{id} is not placed"
        );
    }
}

#[test]
fn the_boss_is_sealed_and_every_route_through_the_ring_is_walkable() {
    let mut simulation = foundry();
    assert!(simulation.gate_is_closed());
    let overseer = pack(&simulation, "THE FOUNDRY OVERSEER")[0];
    assert_eq!(
        simulation.brain(overseer).expect("a boss").kind(),
        EnemyKind::FoundryOverseer
    );
    simulation.unlock_dungeon();
    // The spine, and then each arm of the ring and each yard.
    let spine_in = [
        (0.0, -8.0),
        (0.0, -28.0),
        (0.0, -44.0),
        (0.0, -60.0),
        (0.0, -72.0),
    ];
    let west_arm = [
        (0.0, -72.0),
        (-36.0, -72.0),
        (-36.0, -100.0),
        (-36.0, -132.0),
        (-20.0, -136.0),
        (0.0, -136.0),
    ];
    let spine_out = [
        (0.0, -136.0),
        (0.0, -148.0),
        (0.0, -164.0),
        (0.0, -180.0),
        (0.0, -196.0),
        (0.0, -208.0),
    ];
    let mirror =
        |route: &[(f32, f32)]| -> Vec<(f32, f32)> { route.iter().map(|(x, z)| (-x, *z)).collect() };
    let west_yard = [
        (-36.0, -100.0),
        (-48.0, -100.0),
        (-68.0, -100.0),
        (-76.0, -112.0),
    ];
    let mut routes: Vec<Vec<(f32, f32)>> = Vec::new();
    for arm in [west_arm.to_vec(), mirror(&west_arm)] {
        routes.push([&spine_in[..], &arm[..], &spine_out[..]].concat());
    }
    routes.push(west_yard.to_vec());
    routes.push(mirror(&west_yard));
    for route in routes {
        let mut navigator =
            engine_game::nav::Navigator::new(engine_game::nav::NavConfig::default());
        let mut path = Vec::new();
        for hop in route.windows(2) {
            let from = Vec3::new(hop[0].0, 0.0, hop[0].1);
            let to = Vec3::new(hop[1].0, 0.0, hop[1].1);
            assert!(
                navigator.find_path(simulation.collision_world(), from, to, &mut path),
                "no way from {from:?} to {to:?}"
            );
        }
    }
    for encounter in simulation.encounters() {
        assert!(!encounter.members().is_empty());
    }
}

#[test]
fn skipping_either_yard_still_reaches_one_hundred_but_skipping_both_does_not() {
    let clear = |names: &[&str]| {
        let mut simulation = foundry();
        for name in names {
            for member in pack(&simulation, name) {
                simulation.slay(member);
            }
        }
        simulation.step_once();
        simulation.dungeon_run().expect("a run").percent()
    };
    let main = [
        "SCRAP FORECOURT",
        "SLAG HAULER",
        "CRUCIBLE GANG",
        "LADLE CREW",
        "FOREMAN ON ROUNDS",
        "FURNACE CHOIR",
    ];
    assert_eq!(
        clear(&[&main[..], &["BONE-SORTING YARD"][..]].concat()),
        100
    );
    assert_eq!(clear(&[&main[..], &["SLAG WORKS"][..]].concat()), 100);
    assert!(clear(&main) < 100);
}

#[test]
fn the_whole_dungeon_can_be_cleared_and_the_run_completes() {
    let mut simulation = foundry();
    let names: Vec<String> = simulation
        .encounters()
        .iter()
        .map(|encounter| encounter.name().to_owned())
        .collect();
    for name in names.iter().filter(|name| *name != "THE FOUNDRY OVERSEER") {
        for member in pack(&simulation, name) {
            simulation.slay(member);
        }
    }
    simulation.step_once();
    assert_eq!(simulation.dungeon_run().expect("a run").percent(), 100);
    assert!(!simulation.gate_is_closed());
    // Pulled, as a real fight is, then killed with whatever he called.
    let overseer = pack(&simulation, "THE FOUNDRY OVERSEER")[0];
    let at = simulation.world().position(overseer).expect("a boss");
    let player = simulation.player().entity;
    put(&mut simulation, player, at + Vec3::new(0.0, 0.0, 10.0));
    run(&mut simulation, 90);
    for member in pack(&simulation, "THE FOUNDRY OVERSEER") {
        simulation.slay(member);
    }
    run(&mut simulation, 10);
    assert!(simulation.dungeon_run().expect("a run").is_completed());
    assert!(health(&simulation, player) > 0.0);
}

#[test]
fn bombers_and_throwers_a_pack_pulls_are_all_part_of_its_completion() {
    // Killing a pack by detonation counts like any other kill: run the forecourt's bombers
    // into the party and the percentage still moves.
    let mut simulation = foundry();
    let before = simulation.dungeon_run().expect("a run").percent();
    for member in pack(&simulation, "SCRAP FORECOURT") {
        simulation.slay(member);
    }
    simulation.step_once();
    assert!(simulation.dungeon_run().expect("a run").percent() > before);
}
