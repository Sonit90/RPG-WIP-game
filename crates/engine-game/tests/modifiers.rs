//! Dungeon modifiers, driven headless: rotating beams, plague explosion, and what they must
//! never do.
//!
//! Every test runs the real simulation — the real brains, the real damage path, the real
//! status registry — with a modifier set the way an instance would set it. What is asserted
//! is the mechanic as the design states it: a warm-up that hurts nobody, a cadence, a cap, a
//! five-second fuse, a cloud that infects, and a client that only watches.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::modifier::{
    ActiveModifier, MAX_PLAGUE_FUSES, ModifierKind, ModifierParams, PlagueParams,
};
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;

const PLAYER_HEALTH: f32 = 1_000.0;

/// A simulation with one enemy of `kind` `distance` metres in front of the player, `modifiers`
/// in force, and a player too healthy to die of the test.
fn duel(kind: EnemyKind, distance: f32, modifiers: Vec<ActiveModifier>) -> (Simulation, EntityId) {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("DUEL", vec![EnemySpawn::new(kind, spawn)]);
    let mut simulation = arena::encounter_arena(&config, encounter);
    simulation.set_player_health_override(Some(PLAYER_HEALTH));
    simulation.set_modifiers(modifiers);
    let enemy = simulation
        .encounter_at(0)
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    (simulation, enemy)
}

fn beams(magnitude: f32) -> ActiveModifier {
    ActiveModifier::new(ModifierKind::RotatingBeams, magnitude)
}

fn plague(magnitude: f32) -> ActiveModifier {
    ActiveModifier::new(ModifierKind::PlagueExplosion, magnitude)
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

/// Runs until `found` returns something, up to `limit` ticks.
fn run_until<T>(
    simulation: &mut Simulation,
    limit: u32,
    mut found: impl FnMut(&CombatEvent) -> Option<T>,
) -> Option<T> {
    for _ in 0..limit {
        simulation.step_once();
        for event in simulation.events() {
            if let Some(hit) = found(event) {
                return Some(hit);
            }
        }
    }
    None
}

/// Whether `event` is damage a modifier dealt: from nobody, with no ability.
fn modifier_damage(event: &CombatEvent) -> Option<f32> {
    match event {
        CombatEvent::Damage(damage)
            if damage.source == EntityId::INVALID && damage.ability == AbilityId::NONE =>
        {
            Some(damage.amount)
        }
        _ => None,
    }
}

fn started(event: &CombatEvent) -> Option<(f64, f64, Vec3)> {
    match *event {
        CombatEvent::ModifierBeamsStarted {
            start_time,
            warmup_end_time,
            center,
            ..
        } => Some((start_time, warmup_end_time, center)),
        _ => None,
    }
}

/// Puts the player on top of the beams' centre so every beam covers them.
fn stand_at(simulation: &mut Simulation, point: Vec3) {
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = point;
    }
}

fn player_health(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .health(simulation.player().entity)
        .map_or(0.0, |health| health.current())
}

// ---------------------------------------------------------------------------------------
// Rotating Beams
// ---------------------------------------------------------------------------------------

#[test]
fn beams_light_when_the_pack_engages_and_go_out_when_it_dies() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![beams(1.0)]);
    assert!(
        simulation.modifiers().beams().is_empty(),
        "nothing is lit before a pull"
    );

    let (start, warmup_end, _) =
        run_until(&mut simulation, 600, started).expect("the pack engaged and lit the beams");
    assert!(
        (warmup_end - start - 2.0).abs() < 1e-6,
        "a two-second warm-up"
    );
    let emitter = simulation.modifiers().beams()[0];
    assert_eq!(
        (emitter.beams, emitter.radius, emitter.width),
        (3, 9.0, 0.9)
    );

    simulation.slay(enemy);
    let ended = run(&mut simulation, 2).iter().any(
        |event| matches!(event, CombatEvent::ModifierBeamsEnded { id, .. } if *id == emitter.id),
    );
    assert!(ended, "the beams end with the encounter");
    assert!(simulation.modifiers().beams().is_empty());
}

#[test]
fn the_warm_up_hurts_nobody_and_then_the_beams_tick_every_half_second() {
    let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![beams(1.0)]);
    let (start, warmup_end, center) =
        run_until(&mut simulation, 600, started).expect("the beams lit");
    // Standing on the centre, every beam covers the player whichever way it points.
    stand_at(&mut simulation, center);

    let mut hits: Vec<(f64, f32)> = Vec::new();
    while simulation.time() < warmup_end + 3.0 {
        simulation.step_once();
        stand_at(&mut simulation, center);
        let now = simulation.time();
        hits.extend(
            simulation
                .events()
                .iter()
                .filter_map(modifier_damage)
                .map(|amount| (now, amount)),
        );
    }

    assert!(
        hits.iter().all(|(time, _)| *time >= warmup_end - 1e-3),
        "the warm-up dealt damage (start {start}, warm-up end {warmup_end}): {hits:?}"
    );
    assert!(
        (5..=7).contains(&hits.len()),
        "three seconds at a half-second cadence: {hits:?}"
    );
    for pair in hits.windows(2) {
        let gap = pair[1].0 - pair[0].0;
        assert!(
            (gap - 0.5).abs() < 0.03,
            "ticks are half a second apart: {gap}"
        );
    }
    // Five per cent of the player's maximum health, exactly.
    assert!((hits[0].1 - PLAYER_HEALTH * 0.05).abs() < 0.01, "{hits:?}");
}

#[test]
fn magnitude_scales_the_tick() {
    let amount = |magnitude: f32| {
        let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![beams(magnitude)]);
        let (_, warmup_end, center) = run_until(&mut simulation, 600, started).expect("lit");
        while simulation.time() < warmup_end + 1.0 {
            simulation.step_once();
            stand_at(&mut simulation, center);
            if let Some(hit) = simulation.events().iter().find_map(modifier_damage) {
                return hit;
            }
        }
        panic!("no tick");
    };
    let base = amount(1.0);
    let stronger = amount(1.5);
    assert!((stronger / base - 1.5).abs() < 1e-3, "{base} -> {stronger}");
}

#[test]
fn no_magnitude_can_turn_a_tick_into_a_one_shot() {
    let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![beams(1_000.0)]);
    let (_, warmup_end, center) = run_until(&mut simulation, 600, started).expect("lit");
    let mut worst = 0.0_f32;
    let mut ticks = 0;
    while simulation.time() < warmup_end + 0.6 {
        simulation.step_once();
        stand_at(&mut simulation, center);
        for amount in simulation.events().iter().filter_map(modifier_damage) {
            worst = worst.max(amount);
            ticks += 1;
        }
    }
    assert!(ticks >= 1, "the beams did tick");
    assert!(
        worst < PLAYER_HEALTH * 0.10,
        "a tick took {worst} of {PLAYER_HEALTH}"
    );
    assert!(
        simulation.player_alive(),
        "a full-health player survives a tick"
    );
}

#[test]
fn someone_outside_the_reach_of_the_beams_is_never_hurt() {
    let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![beams(1.0)]);
    let (_, warmup_end, center) = run_until(&mut simulation, 600, started).expect("lit");
    while simulation.time() < warmup_end + 2.0 {
        simulation.step_once();
        stand_at(&mut simulation, center + Vec3::new(30.0, 0.0, 0.0));
        assert!(
            simulation
                .events()
                .iter()
                .all(|e| modifier_damage(e).is_none()),
            "a player 30 m away was hit"
        );
    }
}

#[test]
fn the_same_seed_lights_the_same_beams() {
    let record = || {
        let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![beams(1.0), plague(1.0)]);
        let mut events = run(&mut simulation, 900);
        simulation.slay(
            simulation
                .encounter_at(0)
                .and_then(|encounter| encounter.members().first().copied())
                .expect("a member"),
        );
        events.extend(run(&mut simulation, 600));
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    CombatEvent::ModifierBeamsStarted { .. }
                        | CombatEvent::ModifierBeamsEnded { .. }
                        | CombatEvent::PlagueArmed { .. }
                        | CombatEvent::PlagueExploded { .. }
                        | CombatEvent::PlagueCloudEnded { .. }
                ) || modifier_damage(event).is_some()
            })
            .map(|event| format!("{event:?}"))
            .collect::<Vec<_>>()
    };
    let (first, second) = (record(), record());
    assert!(first.len() >= 4, "the run produced modifier events");
    assert_eq!(first, second);
}

#[test]
fn a_client_under_a_remote_authority_runs_no_modifier_and_only_mirrors_the_events() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![beams(1.0), plague(1.0)]);
    simulation.set_remote_authority(true);
    let events = run(&mut simulation, 1_200);
    assert!(
        events.iter().all(|event| modifier_damage(event).is_none()
            && !matches!(
                event,
                CombatEvent::ModifierBeamsStarted { .. } | CombatEvent::PlagueArmed { .. }
            )),
        "a remote client produced modifier events of its own"
    );
    simulation.slay(enemy);
    run(&mut simulation, 2);
    assert!(simulation.modifiers().fuses().is_empty());

    // What the server announced is filed away for drawing, rebased to the local clock.
    let now = simulation.time();
    simulation.inject_event(CombatEvent::ModifierBeamsStarted {
        id: 7,
        center: Vec3::new(1.0, 0.0, 2.0),
        start_angle: 0.5,
        angular_speed: 0.6,
        radius: 9.0,
        width: 0.9,
        beams: 3,
        warmup_end_time: 502.0,
        start_time: 500.0,
        tick: 1,
    });
    let emitter = simulation.modifiers().beams()[0];
    assert_eq!(emitter.id, 7);
    assert!((emitter.start_time - now).abs() < 1e-9);
    assert!(
        (emitter.warmup_end_time - now - 2.0).abs() < 1e-9,
        "durations survive the rebase"
    );

    simulation.inject_event(CombatEvent::PlagueArmed {
        id: 8,
        position: Vec3::ZERO,
        radius: 4.0,
        armed_time: 500.0,
        explode_time: 505.0,
        tick: 1,
    });
    assert_eq!(simulation.modifiers().fuses().len(), 1);
    simulation.inject_event(CombatEvent::PlagueExploded {
        id: 8,
        position: Vec3::ZERO,
        radius: 4.0,
        tick: 1,
    });
    assert!(simulation.modifiers().fuses().is_empty());
    assert_eq!(simulation.modifiers().clouds().len(), 1);
    simulation.inject_event(CombatEvent::PlagueCloudEnded { id: 8, tick: 1 });
    simulation.inject_event(CombatEvent::ModifierBeamsEnded { id: 7, tick: 1 });
    assert!(simulation.modifiers().clouds().is_empty());
    assert!(simulation.modifiers().beams().is_empty());
}

// ---------------------------------------------------------------------------------------
// Plague Explosion
// ---------------------------------------------------------------------------------------

/// The duel's corpse: kills the enemy, puts the player on the spot, returns where it lies.
fn kill_and_stand_on_it(simulation: &mut Simulation, enemy: EntityId) -> Vec3 {
    let at = simulation.world().position(enemy).expect("it has a place");
    simulation.slay(enemy);
    stand_at(simulation, at);
    at
}

#[test]
fn a_dead_monster_bursts_exactly_five_seconds_later() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![plague(1.0)]);
    let at = kill_and_stand_on_it(&mut simulation, enemy);

    let mut events: Vec<CombatEvent> = simulation.events().to_vec();
    let armed_at = simulation.tick_count();
    events.retain(|event| matches!(event, CombatEvent::PlagueArmed { .. }));
    let CombatEvent::PlagueArmed {
        id,
        position,
        radius,
        armed_time,
        explode_time,
        ..
    } = events[0]
    else {
        panic!("the death armed a plague");
    };
    assert_eq!(position, at);
    assert_eq!(radius, 4.0);
    assert!((explode_time - armed_time - 5.0).abs() < 1e-9);

    // 299 ticks later nothing has burst; the 300th bursts it.
    let mut burst_tick = None;
    for _ in 0..320 {
        stand_at(&mut simulation, at);
        simulation.step_once();
        if simulation.events().iter().any(
            |event| matches!(event, CombatEvent::PlagueExploded { id: burst, .. } if *burst == id),
        ) {
            burst_tick = Some(simulation.tick_count() - armed_at);
            break;
        }
    }
    assert_eq!(burst_tick, Some(300), "five seconds at 60 Hz");
}

#[test]
fn the_burst_hurts_inside_its_radius_and_spares_outside_it() {
    let hurt_at = |offset: f32| {
        let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![plague(1.0)]);
        let at = kill_and_stand_on_it(&mut simulation, enemy);
        let spot = at + Vec3::new(offset, 0.0, 0.0);
        let mut damage = Vec::new();
        for _ in 0..302 {
            stand_at(&mut simulation, spot);
            simulation.step_once();
            let burst = simulation
                .events()
                .iter()
                .any(|event| matches!(event, CombatEvent::PlagueExploded { .. }));
            damage.extend(simulation.events().iter().filter_map(modifier_damage));
            if burst {
                break;
            }
        }
        damage
    };
    let inside = hurt_at(3.0);
    assert_eq!(inside.len(), 1, "one burst, one hit");
    assert!(
        (inside[0] - PLAYER_HEALTH * 0.12).abs() < 0.01,
        "twelve per cent"
    );
    assert!(
        hurt_at(9.0).is_empty(),
        "nine metres away is outside a four-metre burst"
    );
}

#[test]
fn the_cloud_lasts_six_seconds_ticks_and_leaves_a_plagued_debuff_that_lingers() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![plague(1.0)]);
    let at = kill_and_stand_on_it(&mut simulation, enemy);

    let mut burst = None;
    let mut ended = None;
    let mut cloud_ticks = Vec::new();
    let mut plagued_seen = false;
    let mut standing = true;
    let mut left_at = None;
    for _ in 0..(5 + 8) * 60 {
        // Stand in the cloud for four seconds after it forms, then walk out of it.
        if standing {
            stand_at(&mut simulation, at);
        } else {
            stand_at(&mut simulation, at + Vec3::new(30.0, 0.0, 0.0));
        }
        simulation.step_once();
        let now = simulation.time();
        for event in simulation.events() {
            match event {
                CombatEvent::PlagueExploded { .. } => burst = Some(now),
                CombatEvent::PlagueCloudEnded { .. } => ended = Some(now),
                CombatEvent::Damage(_) if burst.is_some() => {
                    if let Some(amount) = modifier_damage(event) {
                        cloud_ticks.push(amount);
                    }
                }
                _ => {}
            }
        }
        plagued_seen |= simulation
            .world()
            .statuses(simulation.player().entity)
            .is_some_and(|statuses| statuses.has(StatusKind::Plagued));
        if let Some(burst) = burst
            && standing
            && now > burst + 4.0
        {
            standing = false;
            left_at = Some(now);
        }
    }

    let (burst, ended) = (burst.expect("it burst"), ended.expect("the cloud ended"));
    assert!(
        (ended - burst - 6.0).abs() < 0.03,
        "a six-second cloud: {}",
        ended - burst
    );
    assert!(plagued_seen, "the cloud infects");
    // The burst's own hit is first; the rest are cloud ticks of three per cent a second.
    assert!(cloud_ticks.len() >= 6, "{cloud_ticks:?}");
    let tick = PLAYER_HEALTH * 0.03 * 0.5;
    assert!(
        cloud_ticks
            .iter()
            .skip(1)
            .all(|amount| (amount - tick).abs() < 0.01),
        "{cloud_ticks:?}"
    );
    // After leaving, the debuff lingers about three seconds and is then gone.
    let left_at = left_at.expect("the player left");
    let plagued_now = simulation
        .world()
        .statuses(simulation.player().entity)
        .is_some_and(|statuses| statuses.has(StatusKind::Plagued));
    assert!(
        !plagued_now,
        "it wore off (left at {left_at}, now {})",
        simulation.time()
    );
}

#[test]
fn plagued_reduces_healing_received_in_the_one_heal_path() {
    let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![plague(1.0)]);
    let player = simulation.player().entity;
    simulation.world_mut().damage(player, 500.0);
    assert!(simulation.apply_effect(player, StatusKind::Plagued, 10.0, 1, EntityId::INVALID));

    let restored = simulation.world_mut().heal(player, 100.0).applied;
    assert!(
        (restored - 80.0).abs() < 1e-3,
        "twenty per cent less: {restored}"
    );
    assert!(simulation.remove_effect(player, StatusKind::Plagued));
    let restored = simulation.world_mut().heal(player, 100.0).applied;
    assert!((restored - 100.0).abs() < 1e-3);
}

#[test]
fn the_boss_does_not_fester_and_players_never_do() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 12.0, vec![plague(1.0)]);
    simulation.slay(boss);
    assert!(
        simulation.modifiers().fuses().is_empty(),
        "the boss is exempt"
    );

    let (mut simulation, _) = duel(EnemyKind::Melee, 8.0, vec![plague(1.0)]);
    let player = simulation.player().entity;
    simulation.slay(player);
    assert!(
        simulation.modifiers().fuses().is_empty(),
        "a player is not a monster"
    );
}

#[test]
fn a_mass_pull_costs_a_bounded_amount_of_ground() {
    let config = ArenaConfig::with_group(0);
    let spawns = (0..MAX_PLAGUE_FUSES + 12)
        .map(|index| {
            let offset = Vec3::new(
                (index % 10) as f32 * 2.0,
                0.0,
                -20.0 - (index / 10) as f32 * 2.0,
            );
            EnemySpawn::new(EnemyKind::Melee, config.player_spawn + offset)
        })
        .collect();
    let mut simulation = arena::encounter_arena(&config, Encounter::with_spawns("MOB", spawns));
    simulation.set_modifiers(vec![plague(1.0)]);
    let members = simulation.encounter().expect("a mob").members().to_vec();
    for member in members {
        simulation.slay(member);
    }
    // Every slay in a row lands in the same event batch.
    let dropped = simulation
        .events()
        .iter()
        .filter(|event| matches!(event, CombatEvent::PlagueCloudEnded { .. }))
        .count();
    assert_eq!(simulation.modifiers().fuses().len(), MAX_PLAGUE_FUSES);
    assert_eq!(dropped, 12, "every fuse that was dropped was announced");
}

#[test]
fn a_reset_puts_the_ground_out() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![plague(1.0), beams(1.0)]);
    simulation.slay(enemy);
    assert_eq!(simulation.modifiers().fuses().len(), 1);
    simulation.reset_encounter();
    assert!(simulation.modifiers().fuses().is_empty());
    let announced = simulation
        .events()
        .iter()
        .any(|event| matches!(event, CombatEvent::PlagueCloudEnded { .. }));
    assert!(announced);
}

#[test]
fn a_simulation_without_modifiers_is_untouched() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![]);
    let events = run(&mut simulation, 300);
    simulation.slay(enemy);
    let more = run(&mut simulation, 400);
    assert!(events.iter().chain(&more).all(|event| !matches!(
        event,
        CombatEvent::ModifierBeamsStarted { .. } | CombatEvent::PlagueArmed { .. }
    ) && modifier_damage(event).is_none()));
    assert!(player_health(&simulation) > 0.0);
}

#[test]
fn custom_parameters_replace_the_fallbacks() {
    let params = PlagueParams {
        fuse: 2.0,
        ..PlagueParams::default()
    };
    let modifier = ActiveModifier::new(ModifierKind::PlagueExplosion, 1.0)
        .with_params(ModifierParams::PlagueExplosion(params));
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![modifier]);
    simulation.slay(enemy);
    let fuse = simulation.modifiers().fuses()[0];
    assert!((fuse.explode_time - fuse.armed_time - 2.0).abs() < 1e-9);
}

// ---------------------------------------------------------------------------------------
// The monster-side modifiers: Bolstering, Necrotic Pools, Raging, Spiteful Casters
// ---------------------------------------------------------------------------------------

fn bolstering(magnitude: f32) -> ActiveModifier {
    ActiveModifier::new(ModifierKind::Bolstering, magnitude)
}

fn necrotic(magnitude: f32) -> ActiveModifier {
    ActiveModifier::new(ModifierKind::NecroticPools, magnitude)
}

fn raging(magnitude: f32) -> ActiveModifier {
    ActiveModifier::new(ModifierKind::Raging, magnitude)
}

fn spiteful(magnitude: f32) -> ActiveModifier {
    ActiveModifier::new(ModifierKind::SpitefulCasters, magnitude)
}

/// A pack of `kinds` standing `distance` metres in front of the player, spread along X.
fn pack(
    kinds: &[EnemyKind],
    distance: f32,
    modifiers: Vec<ActiveModifier>,
) -> (Simulation, Vec<EntityId>) {
    let config = ArenaConfig::with_group(0);
    let spawns = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let at = config.player_spawn + Vec3::new(index as f32 * 2.0, 0.0, -distance);
            EnemySpawn::new(*kind, at)
        })
        .collect();
    let mut simulation = arena::encounter_arena(&config, Encounter::with_spawns("PACK", spawns));
    simulation.set_player_health_override(Some(PLAYER_HEALTH));
    simulation.set_modifiers(modifiers);
    let members = simulation
        .encounter_at(0)
        .map(|encounter| encounter.members().to_vec())
        .expect("the encounter spawned its members");
    (simulation, members)
}

fn max_health(simulation: &Simulation, entity: EntityId) -> f32 {
    simulation
        .world()
        .health(entity)
        .map(|health| health.max())
        .expect("it has health")
}

fn current_health(simulation: &Simulation, entity: EntityId) -> f32 {
    simulation
        .world()
        .health(entity)
        .map(|health| health.current())
        .expect("it has health")
}

fn set_health_fraction(simulation: &mut Simulation, entity: EntityId, fraction: f32) {
    let max = max_health(simulation, entity);
    if let Some(health) = simulation.world_mut().health_mut(entity) {
        health.set_current(max * fraction);
    }
}

/// The first hit `source` lands on anybody, within `ticks`.
fn first_hit_from(simulation: &mut Simulation, source: EntityId, ticks: u32) -> Option<f32> {
    run_until(simulation, ticks, |event| match event {
        CombatEvent::Damage(damage) if damage.source == source => Some(damage.amount),
        _ => None,
    })
}

// --- Bolstering ------------------------------------------------------------------------

#[test]
fn a_death_bolsters_the_living_neighbours_and_nobody_else() {
    let (mut simulation, members) = pack(
        &[EnemyKind::Melee, EnemyKind::Melee, EnemyKind::Caster],
        40.0,
        vec![bolstering(1.0)],
    );
    let base = max_health(&simulation, members[1]);
    simulation.slay(members[0]);
    run(&mut simulation, 2);

    // One stack of ten per cent: more maximum health, and as much more current health.
    let bolstered = simulation.modifiers().bolstered();
    assert_eq!(bolstered.len(), 2, "both survivors, not the dead one");
    assert!(bolstered.iter().all(|entry| entry.stacks == 1));
    assert!((max_health(&simulation, members[1]) - base * 1.10).abs() < 1e-2);
    assert!((current_health(&simulation, members[1]) - base * 1.10).abs() < 1e-2);

    // Out of the radius: nobody. A bystander 100 m away gains nothing.
    let (mut far, members) = pack(
        &[EnemyKind::Melee, EnemyKind::Melee],
        40.0,
        vec![bolstering(1.0)],
    );
    if let Some(body) = far.world_mut().body_mut(members[1]) {
        body.position += Vec3::new(0.0, 0.0, -100.0);
    }
    far.slay(members[0]);
    run(&mut far, 2);
    assert!(far.modifiers().bolstered().is_empty());
}

#[test]
fn bolstering_stacks_to_its_cap_and_no_further() {
    let kinds = [EnemyKind::Melee; 9];
    let (mut simulation, members) = pack(&kinds, 40.0, vec![bolstering(1.0)]);
    let base = max_health(&simulation, members[8]);
    for dead in &members[..8] {
        simulation.slay(*dead);
        run(&mut simulation, 1);
    }
    let entry = simulation
        .modifiers()
        .bolstered()
        .iter()
        .find(|entry| entry.entity == members[8])
        .copied()
        .expect("the survivor is bolstered");
    // Eight deaths, five stacks: the cap holds, and the maximum is half again the base rather
    // than a compounding product.
    assert_eq!(entry.stacks, 5);
    assert!((max_health(&simulation, members[8]) - base * 1.5).abs() < 1e-1);
}

#[test]
fn bolstered_monsters_hit_harder_and_a_reset_undoes_it() {
    let hit = |modifiers: Vec<ActiveModifier>| {
        let (mut simulation, members) = pack(&[EnemyKind::Melee, EnemyKind::Melee], 6.0, modifiers);
        simulation.slay(members[0]);
        first_hit_from(&mut simulation, members[1], 600).expect("the survivor hit the player")
    };
    let plain = hit(Vec::new());
    let bolstered = hit(vec![bolstering(1.0)]);
    assert!(
        (bolstered / plain - 1.10).abs() < 1e-3,
        "{bolstered} against {plain}"
    );

    let (mut simulation, members) = pack(
        &[EnemyKind::Melee, EnemyKind::Melee],
        40.0,
        vec![bolstering(1.0)],
    );
    let base = max_health(&simulation, members[1]);
    simulation.slay(members[0]);
    run(&mut simulation, 2);
    assert!(max_health(&simulation, members[1]) > base);
    simulation.reset_encounter();
    assert!(simulation.modifiers().bolstered().is_empty());
    assert!((max_health(&simulation, members[1]) - base).abs() < 1e-3);
    assert!(
        (current_health(&simulation, members[1]) - base).abs() < 1e-3,
        "restored to the base, and full"
    );
    assert_eq!(simulation.enemy_modifier_damage(members[1]), 1.0);
}

#[test]
fn bolstering_leaves_the_boss_alone() {
    let (mut simulation, members) = pack(
        &[EnemyKind::Melee, EnemyKind::Boss],
        40.0,
        vec![bolstering(1.0)],
    );
    let base = max_health(&simulation, members[1]);
    simulation.slay(members[0]);
    run(&mut simulation, 2);
    assert!(simulation.modifiers().bolstered().is_empty());
    assert_eq!(max_health(&simulation, members[1]), base);
}

// --- Necrotic Pools --------------------------------------------------------------------

#[test]
fn a_necrotic_pool_warns_then_forms_then_ends() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![necrotic(1.0)]);
    let at = kill_and_stand_on_it(&mut simulation, enemy);
    let mut events: Vec<CombatEvent> = simulation.events().to_vec();
    events.extend(run(&mut simulation, 60 * 12));

    let (id, warned, formed) = events
        .iter()
        .find_map(|event| match *event {
            CombatEvent::PlagueArmed {
                id,
                position,
                radius,
                armed_time,
                explode_time,
                ..
            } => {
                assert_eq!(position, at);
                assert_eq!(radius, 3.0);
                Some((id, armed_time, explode_time))
            }
            _ => None,
        })
        .expect("the death marked the ground");
    assert!(
        (formed - warned - 1.5).abs() < 1e-6,
        "a second and a half of warning"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            CombatEvent::PlagueExploded { id: formed_id, .. } if *formed_id == id
        )),
        "the pool formed"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            CombatEvent::PlagueCloudEnded { id: ended, .. } if *ended == id
        )),
        "and ended after its duration"
    );
    assert!(simulation.modifiers().pools().is_empty());
    assert!(simulation.modifiers().pool_warnings().is_empty());
}

#[test]
fn a_necrotic_pool_hurts_only_once_it_has_formed_and_only_inside() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![necrotic(1.0)]);
    kill_and_stand_on_it(&mut simulation, enemy);

    // The warning: standing on it costs nothing.
    let during_warning = run(&mut simulation, 80);
    assert!(
        during_warning
            .iter()
            .all(|event| modifier_damage(event).is_none()),
        "the warning hurt somebody"
    );
    // Formed: 4 % a second, a tick every half second, two per cent a tick.
    let formed = run(&mut simulation, 60 * 3);
    let hits: Vec<f32> = formed.iter().filter_map(modifier_damage).collect();
    assert!(hits.len() >= 4, "{} ticks in three seconds", hits.len());
    let expected = PLAYER_HEALTH * 0.02;
    assert!(
        hits.iter().all(|hit| (hit - expected).abs() < 1e-2),
        "{hits:?} against {expected}"
    );
    // A pool infects nobody: this is ground, not a plague.
    let statuses = simulation
        .world()
        .statuses(simulation.player().entity)
        .expect("statuses");
    assert!(!statuses.has(StatusKind::Plagued));

    // Step out: nothing more.
    stand_at(&mut simulation, Vec3::new(40.0, 0.0, 40.0));
    let outside = run(&mut simulation, 60 * 2);
    assert!(outside.iter().all(|event| modifier_damage(event).is_none()));
}

#[test]
fn the_boss_leaves_no_pool_and_a_reset_clears_them() {
    let (mut simulation, boss) = duel(EnemyKind::Boss, 12.0, vec![necrotic(1.0)]);
    simulation.slay(boss);
    run(&mut simulation, 2);
    assert!(simulation.modifiers().pool_warnings().is_empty());

    let (mut simulation, enemy) = duel(EnemyKind::Melee, 8.0, vec![necrotic(1.0)]);
    simulation.slay(enemy);
    run(&mut simulation, 120);
    assert_eq!(simulation.modifiers().pools().len(), 1);
    simulation.reset_encounter();
    assert!(simulation.modifiers().pools().is_empty());
    assert!(simulation.modifiers().pool_warnings().is_empty());
}

// --- Raging ----------------------------------------------------------------------------

#[test]
fn a_wounded_monster_hits_harder_and_a_healthy_one_does_not() {
    let hit = |modifiers: Vec<ActiveModifier>, fraction: f32| {
        let (mut simulation, enemy) = duel(EnemyKind::Melee, 3.0, modifiers);
        set_health_fraction(&mut simulation, enemy, fraction);
        first_hit_from(&mut simulation, enemy, 600).expect("it hit the player")
    };
    let plain = hit(Vec::new(), 0.25);
    let enraged = hit(vec![raging(1.0)], 0.25);
    assert!(
        (enraged / plain - 1.5).abs() < 1e-3,
        "{enraged} against {plain}"
    );
    let healthy = hit(vec![raging(1.0)], 0.9);
    assert!(
        (healthy / plain - 1.0).abs() < 1e-3,
        "{healthy} against {plain}"
    );
    // Magnitude scales the bonus.
    let stronger = hit(vec![raging(2.0)], 0.25);
    assert!((stronger / plain - 2.0).abs() < 1e-3);
}

#[test]
fn rage_ends_when_the_monster_is_healed_over_the_threshold_and_spares_the_boss() {
    let (mut simulation, enemy) = duel(EnemyKind::Melee, 40.0, vec![raging(1.0)]);
    set_health_fraction(&mut simulation, enemy, 0.2);
    run(&mut simulation, 2);
    assert!((simulation.enemy_modifier_damage(enemy) - 1.5).abs() < 1e-6);
    set_health_fraction(&mut simulation, enemy, 0.6);
    run(&mut simulation, 2);
    assert_eq!(
        simulation.enemy_modifier_damage(enemy),
        1.0,
        "healed out of it"
    );

    let (mut simulation, boss) = duel(EnemyKind::Boss, 40.0, vec![raging(1.0)]);
    set_health_fraction(&mut simulation, boss, 0.1);
    run(&mut simulation, 2);
    assert_eq!(
        simulation.enemy_modifier_damage(boss),
        1.0,
        "bosses do not rage"
    );
}

// --- Spiteful Casters ------------------------------------------------------------------

fn first_cast_duration(simulation: &mut Simulation, caster: EntityId) -> f32 {
    run_until(simulation, 600, |event| match event {
        CombatEvent::CastStarted {
            source, duration, ..
        } if *source == caster => Some(*duration),
        _ => None,
    })
    .expect("the caster started a cast")
}

fn current_cast(
    simulation: &Simulation,
    caster: EntityId,
) -> Option<engine_game::enemy::brain::EnemyCast> {
    simulation
        .brain(caster)
        .and_then(engine_game::EnemyBrain::cast)
}

#[test]
fn an_interruptible_cast_is_faster() {
    let (mut plain, caster) = duel(EnemyKind::Ritualist, 9.0, Vec::new());
    let normal = first_cast_duration(&mut plain, caster);
    let (mut hurried, caster) = duel(EnemyKind::Ritualist, 9.0, vec![spiteful(1.0)]);
    let quick = first_cast_duration(&mut hurried, caster);
    assert!(
        current_cast(&hurried, caster).is_some_and(|cast| cast.interruptible),
        "the ritualist's nova is the interruptible kind"
    );
    assert!(
        (quick - normal / 1.3).abs() < 1e-4,
        "{quick} against {normal}"
    );
    // Magnitude scales it.
    let (mut hasty, caster) = duel(EnemyKind::Ritualist, 9.0, vec![spiteful(2.0)]);
    let quicker = first_cast_duration(&mut hasty, caster);
    assert!((quicker - normal / 1.6).abs() < 1e-4);
}

#[test]
fn a_completed_cast_curses_and_an_interrupted_one_does_not() {
    let (mut simulation, caster) = duel(EnemyKind::Ritualist, 9.0, vec![spiteful(1.0)]);
    let mut cast = None;
    for _ in 0..600 {
        simulation.step_once();
        cast = current_cast(&simulation, caster);
        if cast.is_some() {
            break;
        }
    }
    assert!(cast.expect("it started a cast").interruptible);
    assert!(simulation.interrupt(caster), "the cast is stopped");
    let after = run(&mut simulation, 120);
    assert!(
        !after.iter().any(|event| matches!(
            event,
            CombatEvent::StatusApplied {
                status: StatusKind::Spiteful,
                ..
            }
        )),
        "an interrupted cast leaves nothing"
    );

    let (mut simulation, _) = duel(EnemyKind::Ritualist, 9.0, vec![spiteful(1.0)]);
    let player_id = simulation.player().entity;
    let cursed = run_until(&mut simulation, 900, |event| match event {
        CombatEvent::StatusApplied { status, target, .. }
            if *status == StatusKind::Spiteful && *target == player_id =>
        {
            Some(())
        }
        _ => None,
    });
    assert!(cursed.is_some(), "a cast that landed cursed its victim");
    assert!(
        simulation
            .world()
            .statuses(player_id)
            .expect("statuses")
            .has(StatusKind::Spiteful)
    );
}

#[test]
fn the_curse_is_a_dispellable_magic_debuff_that_cuts_healing() {
    assert_eq!(
        StatusKind::Spiteful.dispel_school(),
        Some(engine_game::status::DispelSchool::Magic),
        "a Magic dispel takes it off"
    );
    let (mut simulation, _) = duel(EnemyKind::Melee, 40.0, vec![spiteful(1.0)]);
    let player = simulation.player().entity;
    simulation.world_mut().damage(player, 500.0);
    assert!(simulation.apply_effect(player, StatusKind::Spiteful, 10.0, 1, EntityId::INVALID));
    let restored = simulation.world_mut().heal(player, 100.0).applied;
    assert!(
        (restored - 70.0).abs() < 1e-3,
        "thirty per cent less: {restored}"
    );
    assert!(simulation.remove_effect(player, StatusKind::Spiteful));
    let restored = simulation.world_mut().heal(player, 100.0).applied;
    assert!((restored - 100.0).abs() < 1e-3);
}

#[test]
fn the_monster_side_modifiers_are_deterministic() {
    let record = || {
        let (mut simulation, members) = pack(
            &[EnemyKind::Melee, EnemyKind::Ritualist, EnemyKind::Melee],
            12.0,
            vec![bolstering(1.0), necrotic(1.0), raging(1.0), spiteful(1.0)],
        );
        let mut events = run(&mut simulation, 300);
        simulation.slay(members[0]);
        events.extend(run(&mut simulation, 900));
        events
            .iter()
            .map(|event| format!("{event:?}"))
            .collect::<Vec<_>>()
    };
    assert_eq!(record(), record());
}
