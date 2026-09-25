//! Threat, asserted against a real arena with a real enemy.
//!
//! What a unit test on the threat table cannot see: that each hero's abilities feed the table
//! at their role's rate, that healing is threat on the enemies fighting the one healed, that
//! the enemy's mind really does turn to whoever is highest, that a taunt overrides that and
//! then lets go, and that every one of those changes reaches the event stream the floating
//! combat text reads.
//!
//! A second attacker is the arena's friendly healing dummy, whose "attacks" are
//! [`Simulation::apply_damage`] — the same pipeline every ability lands through, so the
//! threat it generates is threat, not a number written into a table by the test.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::combat::DamageType;
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::{AggroChange, CombatEvent, ControlKind};
use engine_game::firemage::{ABILITIES, FireAbility, ThreatScale};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::world::EntityKind;

/// The arena, `hero` selected, a boss beside the player and the friendly dummy as a second
/// party member.
///
/// The player is given more health than anything in here can take away and regeneration is
/// switched off, so nothing but the abilities under test moves a health bar or a threat
/// table.
fn fight(hero: Hero) -> (Simulation, EntityId, EntityId) {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    simulation.set_hero(hero);
    assert_eq!(simulation.hero(), hero);
    simulation.set_health_regen_per_second(0.0);
    simulation.set_player_health_override(Some(50_000_000.0));
    let origin = simulation
        .world()
        .position(simulation.player().entity)
        .expect("the player has a position");
    // The boss, because it has the health to be hit for a few thousand and still stand.
    let enemy = simulation.spawn_enemy(EnemyKind::Boss, origin + Vec3::new(0.0, 0.0, -3.0), 0.0);
    let ally = simulation
        .world()
        .entities()
        .find(|id| simulation.world().kind(*id) == Some(EntityKind::HealingDummy))
        .expect("the arena has a friendly dummy");
    simulation.player_mut().target = Some(enemy);
    (simulation, enemy, ally)
}

/// Steps `ticks` times, keeping every event: the stream only holds the last step's.
fn run(simulation: &mut Simulation, ticks: u32, log: &mut Vec<CombatEvent>) {
    for _ in 0..ticks {
        simulation.step_once();
        log.extend_from_slice(simulation.events());
    }
}

/// Hits `enemy` for `amount` on `source`'s behalf, through the ordinary damage pipeline.
fn hit(simulation: &mut Simulation, source: EntityId, enemy: EntityId, amount: f32) {
    simulation.apply_damage(source, enemy, amount, AbilityId::NONE, DamageType::Physical);
}

fn threat(simulation: &Simulation, enemy: EntityId, entity: EntityId) -> f32 {
    simulation
        .brain(enemy)
        .map_or(0.0, |brain| brain.threat().threat(entity))
}

fn attacking(simulation: &Simulation, enemy: EntityId) -> Option<EntityId> {
    simulation.brain(enemy).and_then(|brain| brain.target())
}

fn aggro_count(log: &[CombatEvent], enemy: EntityId, who: EntityId, what: AggroChange) -> usize {
    log.iter()
        .filter(|event| {
            matches!(
                **event,
                CombatEvent::Aggro { enemy: e, entity, change, .. }
                    if e == enemy && entity == who && change == what
            )
        })
        .count()
}

fn damage_dealt(log: &[CombatEvent], source: EntityId, target: EntityId) -> f32 {
    log.iter()
        .filter_map(CombatEvent::as_damage)
        .filter(|damage| damage.source == source && damage.target == target)
        .map(|damage| damage.amount)
        .sum()
}

fn healing_done(log: &[CombatEvent], source: EntityId) -> f32 {
    log.iter()
        .filter_map(|event| match *event {
            CombatEvent::Healing {
                source: s, amount, ..
            } if s == source => Some(amount),
            _ => None,
        })
        .sum()
}

/// Lets the enemy notice, turn and close in, so it is fighting rather than still waking up.
fn engage(simulation: &mut Simulation, enemy: EntityId, log: &mut Vec<CombatEvent>) {
    run(simulation, 150, log);
    assert!(
        simulation
            .brain(enemy)
            .is_some_and(|brain| brain.state().is_combat()),
        "the enemy never engaged: {:?}",
        simulation.brain(enemy).map(|brain| brain.state())
    );
}

// -------------------------------------------------------------------- the ability table

#[test]
fn every_ability_threatens_at_its_roles_rate() {
    for definition in &ABILITIES {
        let scale = definition.threat;
        match definition.hero {
            Hero::FireMage => assert_eq!(scale, ThreatScale::DAMAGE_DEALER, "{:?}", definition.id),
            Hero::Warrior => assert!(
                scale.damage >= 2.0,
                "{:?} is below tank threat",
                definition.id
            ),
            Hero::Druid => assert_eq!(scale, ThreatScale::HEALER, "{:?}", definition.id),
        }
        assert_eq!(scale.healing, 0.5, "{:?}", definition.id);
    }
    assert_eq!(ThreatScale::DAMAGE_DEALER.damage, 1.0);
    assert_eq!(ThreatScale::TANK.damage, 2.0);
    assert_eq!(ThreatScale::HEALER.healing, 0.5);
}

// ------------------------------------------------------------------ threat from damage

#[test]
fn a_mage_generates_one_threat_per_point_of_damage() {
    let (mut simulation, enemy, _) = fight(Hero::FireMage);
    let player = simulation.player().entity;
    let mut log = Vec::new();
    simulation.cast(FireAbility::Firebolt).expect("in range");
    run(&mut simulation, 90, &mut log);

    let dealt = damage_dealt(&log, player, enemy);
    assert!(dealt > 0.0, "the Firebolt never landed");
    let generated = threat(&simulation, enemy, player);
    assert!(
        (generated - dealt).abs() < 0.5,
        "{generated} threat for {dealt} damage"
    );
}

#[test]
fn a_tank_generates_two_threat_per_point_of_damage() {
    let (mut simulation, enemy, _) = fight(Hero::Warrior);
    let player = simulation.player().entity;
    simulation.warrior_mut().deflection.set(0.0);
    // Recuperate is the Warrior's filler, not a threat tool: it is the plain tank rate.
    assert_eq!(
        FireAbility::Recuperate.definition().threat,
        ThreatScale::TANK
    );

    simulation.cast(FireAbility::Recuperate).expect("in range");
    let dealt = damage_dealt(simulation.events(), player, enemy);
    assert!(dealt > 0.0, "the swing never landed");
    let generated = threat(&simulation, enemy, player);
    assert!(
        (generated - 2.0 * dealt).abs() < 0.5,
        "{generated} threat for {dealt} damage"
    );
}

// ----------------------------------------------------------------- threat from healing

#[test]
fn a_healer_generates_half_a_threat_per_point_of_healing() {
    let (mut simulation, enemy, ally) = fight(Hero::Druid);
    let player = simulation.player().entity;
    // The ally is in the fight: the enemy has it in its threat table.
    hit(&mut simulation, ally, enemy, 100.0);
    simulation.player_mut().target = Some(ally);
    assert_eq!(threat(&simulation, enemy, player), 0.0);

    let mut log = Vec::new();
    simulation
        .cast(FireAbility::Regrowth)
        .expect("the heal was refused");
    run(&mut simulation, 120, &mut log);

    let healed = healing_done(&log, player);
    assert!(healed > 0.0, "the heal never landed");
    let generated = threat(&simulation, enemy, player);
    assert!(
        (generated - 0.5 * healed).abs() < 0.5,
        "{generated} threat for {healed} healing"
    );
    assert_eq!(
        threat(&simulation, enemy, ally),
        100.0,
        "healing credits the healer, not the healed"
    );
}

#[test]
fn healing_somebody_no_enemy_is_fighting_generates_no_threat() {
    let (mut simulation, enemy, ally) = fight(Hero::Druid);
    let player = simulation.player().entity;
    simulation.player_mut().target = Some(ally);

    let mut log = Vec::new();
    simulation
        .cast(FireAbility::Regrowth)
        .expect("the heal was refused");
    run(&mut simulation, 120, &mut log);

    assert!(healing_done(&log, player) > 0.0, "the heal never landed");
    assert!(
        simulation
            .brain(enemy)
            .is_some_and(|brain| !brain.threat().contains(player)),
        "a heal on somebody nobody is fighting pulled the enemy"
    );
}

#[test]
fn overhealing_generates_no_threat() {
    let (mut simulation, enemy, ally) = fight(Hero::Druid);
    let player = simulation.player().entity;
    hit(&mut simulation, ally, enemy, 100.0);
    simulation.world_mut().restore(ally);
    simulation.player_mut().target = Some(ally);

    let mut log = Vec::new();
    simulation.cast(FireAbility::Rejuvenation).expect("instant");
    run(&mut simulation, 60, &mut log);

    assert_eq!(healing_done(&log, player), 0.0, "a full ally was healed");
    assert_eq!(threat(&simulation, enemy, player), 0.0);
}

// -------------------------------------------------------------------- who is attacked

#[test]
fn a_monster_attacks_whoever_has_the_most_threat() {
    let (mut simulation, enemy, ally) = fight(Hero::FireMage);
    let player = simulation.player().entity;
    let mut log = Vec::new();
    hit(&mut simulation, ally, enemy, 1_000.0);
    hit(&mut simulation, player, enemy, 400.0);
    engage(&mut simulation, enemy, &mut log);
    assert_eq!(attacking(&simulation, enemy), Some(ally));

    // Out-threaten the ally: the enemy turns, and says so to both of them.
    log.clear();
    hit(&mut simulation, player, enemy, 1_000.0);
    run(&mut simulation, 1, &mut log);
    assert_eq!(attacking(&simulation, enemy), Some(player));
    assert_eq!(aggro_count(&log, enemy, player, AggroChange::Gained), 1);
    assert_eq!(aggro_count(&log, enemy, ally, AggroChange::Lost), 1);
}

#[test]
fn closing_in_on_the_holders_threat_warns_both_sides_once() {
    let (mut simulation, enemy, ally) = fight(Hero::FireMage);
    let player = simulation.player().entity;
    let mut log = Vec::new();
    hit(&mut simulation, ally, enemy, 1_000.0);
    hit(&mut simulation, player, enemy, 500.0);
    engage(&mut simulation, enemy, &mut log);
    assert_eq!(aggro_count(&log, enemy, player, AggroChange::Gaining), 0);

    // Ninety-two per cent of the ally's threat: about to pull, not pulled yet.
    log.clear();
    hit(&mut simulation, player, enemy, 420.0);
    run(&mut simulation, 60, &mut log);
    assert_eq!(
        attacking(&simulation, enemy),
        Some(ally),
        "it switched early"
    );
    assert_eq!(aggro_count(&log, enemy, player, AggroChange::Gaining), 1);
    assert_eq!(aggro_count(&log, enemy, ally, AggroChange::Losing), 1);
    assert_eq!(
        aggro_count(&log, enemy, player, AggroChange::Gained),
        0,
        "a warning is not a switch"
    );
}

// ---------------------------------------------------------------------------- taunt

#[test]
fn taunt_forces_the_enemy_onto_the_tank_until_it_runs_out() {
    let (mut simulation, enemy, ally) = fight(Hero::Warrior);
    let player = simulation.player().entity;
    let mut log = Vec::new();
    hit(&mut simulation, ally, enemy, 1_000.0);
    engage(&mut simulation, enemy, &mut log);
    assert_eq!(attacking(&simulation, enemy), Some(ally));

    log.clear();
    simulation.player_mut().target = Some(enemy);
    simulation.cast(FireAbility::Taunt).expect("in range");
    log.extend_from_slice(simulation.events());
    assert!(
        log.iter().any(|event| matches!(
            *event,
            CombatEvent::ControlApplied { target, control: ControlKind::Taunt, .. } if target == enemy
        )),
        "the taunt was not announced"
    );
    assert!(
        (threat(&simulation, enemy, player) - 1_000.0).abs() < 0.5,
        "the taunt lifts the tank to the top threat"
    );

    run(&mut simulation, 1, &mut log);
    assert_eq!(attacking(&simulation, enemy), Some(player));
    assert_eq!(aggro_count(&log, enemy, player, AggroChange::Gained), 1);
    assert_eq!(aggro_count(&log, enemy, ally, AggroChange::Lost), 1);

    // No amount of threat takes it back while the taunt runs...
    hit(&mut simulation, ally, enemy, 1_500.0);
    run(&mut simulation, 60, &mut log);
    assert_eq!(
        attacking(&simulation, enemy),
        Some(player),
        "the taunt broke early"
    );

    // ...and once it has run out, threat decides again.
    let duration = FireAbility::Taunt.definition().duration;
    run(&mut simulation, (duration * 60.0) as u32, &mut log);
    assert!(
        simulation
            .brain(enemy)
            .is_some_and(|brain| brain.threat().taunted().is_none())
    );
    assert_eq!(attacking(&simulation, enemy), Some(ally));
    assert_eq!(aggro_count(&log, enemy, player, AggroChange::Lost), 1);
}

#[test]
fn taunt_is_the_warriors_and_sits_beside_the_bar() {
    assert_eq!(FireAbility::Taunt.hero(), Hero::Warrior);
    assert!(
        Hero::Warrior
            .extras()
            .any(|ability| ability == FireAbility::Taunt)
    );
    assert!(FireAbility::Taunt.definition().duration > 0.0);
    assert!(FireAbility::Taunt.definition().target.needs_target());
}
