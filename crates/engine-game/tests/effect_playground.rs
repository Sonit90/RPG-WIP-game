//! The global effect registry, exercised the way the Monster Combat Playground exercises it:
//! applied by name to the player and to monsters, counted down by the simulation's clock,
//! stacked, refreshed, removed, dispelled by the Druid's real Nature's Cleanse, and cleaned
//! up by the playground's resets.

#![allow(
    clippy::expect_used,
    reason = "integration tests may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::character::Stat;
use engine_game::character::stat::health_from_stamina;
use engine_game::combat::DamageType;
use engine_game::enemy::EnemyKind;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::playground::{HealthMode, MonsterPlayground, TargetRole};
use engine_game::sim::Simulation;
use engine_game::status::{EffectClass, StatusKind};
use engine_game::world::EntityKind;

fn playground() -> (Simulation, MonsterPlayground) {
    let mut simulation = arena::training_arena(&ArenaConfig::with_group(0));
    let mut playground = MonsterPlayground::default();
    playground.install(&mut simulation);
    // Normal health, so Stamina is visible in the maximum.
    playground.set_health_mode(&mut simulation, HealthMode::Normal);
    (simulation, playground)
}

/// A passive monster in front of the player, made the effect target.
fn monster(
    simulation: &mut Simulation,
    playground: &mut MonsterPlayground,
    kind: EnemyKind,
) -> EntityId {
    playground.set_passive(true);
    let selection = playground.selection_mut();
    selection.clear();
    selection.select(kind);
    let origin = simulation.player_position();
    playground.spawn_selected(simulation, origin, 0.0);
    let entity = playground.encounter().members()[0].entity;
    playground.set_effect_target(entity);
    entity
}

fn remaining(simulation: &Simulation, entity: EntityId, kind: StatusKind) -> Option<f32> {
    simulation
        .world()
        .statuses(entity)
        .and_then(|statuses| statuses.get(kind))
        .map(|status| status.remaining)
}

fn stacks(simulation: &Simulation, entity: EntityId, kind: StatusKind) -> u8 {
    simulation
        .world()
        .statuses(entity)
        .map_or(0, |statuses| statuses.stacks(kind))
}

fn max_health(simulation: &Simulation) -> f32 {
    simulation
        .world()
        .health(simulation.player().entity)
        .expect("a player")
        .max()
}

// ---------------------------------------------------------------------------------------
// The registry

#[test]
fn every_registered_effect_is_offered_and_split_into_buffs_and_debuffs() {
    let buffs = StatusKind::ALL
        .into_iter()
        .filter(|kind| kind.definition().class == EffectClass::Buff)
        .count();
    let debuffs = StatusKind::ALL.len() - buffs;
    assert!(
        buffs >= 10 && debuffs >= 10,
        "{buffs} buffs, {debuffs} debuffs"
    );
    for kind in StatusKind::ALL {
        assert_eq!(StatusKind::from_id(kind.definition().id), Some(kind));
    }
}

// ---------------------------------------------------------------------------------------
// Applying, counting down, expiring

#[test]
fn an_effect_applied_to_the_player_counts_down_on_the_game_clock_and_expires() {
    let (mut simulation, playground) = playground();
    let player = simulation.player().entity;
    assert!(playground.apply_effect(&mut simulation, StatusKind::Regeneration, 2.0, 1));
    let applied = simulation.events().iter().any(|event| {
        matches!(event, CombatEvent::StatusApplied { target, status: StatusKind::Regeneration, duration, .. }
            if *target == player && (*duration - 2.0).abs() < 1e-6)
    });
    assert!(applied, "the ordinary StatusApplied event");
    assert_eq!(
        remaining(&simulation, player, StatusKind::Regeneration),
        Some(2.0)
    );

    simulation.step_many(60);
    let left = remaining(&simulation, player, StatusKind::Regeneration).expect("still on");
    assert!((left - 1.0).abs() < 0.05, "one second of game time: {left}");

    let mut expired = false;
    for _ in 0..90 {
        simulation.step_once();
        expired |= simulation.events().iter().any(|event| {
            matches!(event, CombatEvent::StatusRemoved { target, status: StatusKind::Regeneration, reason, .. }
                if *target == player && *reason == engine_game::status::StatusEnd::Expired)
        });
    }
    assert!(expired, "it ended through the ordinary expiry");
    assert_eq!(
        remaining(&simulation, player, StatusKind::Regeneration),
        None
    );
}

#[test]
fn each_effect_keeps_its_own_duration() {
    let (mut simulation, playground) = playground();
    let player = simulation.player().entity;
    playground.apply_effect(&mut simulation, StatusKind::Fortitude, 5.0, 1);
    playground.apply_effect(&mut simulation, StatusKind::Bloodlust, 30.0, 1);
    playground.apply_effect(&mut simulation, StatusKind::Poisoned, 120.0, 1);
    simulation.step_many(6 * 60);
    assert_eq!(remaining(&simulation, player, StatusKind::Fortitude), None);
    assert!(remaining(&simulation, player, StatusKind::Bloodlust).is_some_and(|left| left > 20.0));
    assert!(remaining(&simulation, player, StatusKind::Poisoned).is_some_and(|left| left > 110.0));
}

#[test]
fn respawn_protection_cannot_be_applied_from_outside() {
    let (mut simulation, playground) = playground();
    assert!(!playground.apply_effect(&mut simulation, StatusKind::RespawnProtection, 30.0, 1));
}

// ---------------------------------------------------------------------------------------
// Stats

#[test]
fn a_stat_buff_changes_the_sheet_and_health_and_expiry_puts_them_back() {
    let (mut simulation, playground) = playground();
    let stamina = simulation.character_stats().get(Stat::Stamina);
    let health = max_health(&simulation);

    playground.apply_effect(&mut simulation, StatusKind::Fortitude, 2.0, 1);
    assert_eq!(
        simulation.character_stats().get(Stat::Stamina),
        stamina + 20.0
    );
    assert!(
        (max_health(&simulation) - health_from_stamina(stamina + 20.0)).abs() < 1e-3,
        "maximum health follows Stamina at once"
    );
    let shown = simulation
        .effect_stats(simulation.player().entity)
        .and_then(|stats| stats.sheet)
        .expect("the player has a sheet");
    assert_eq!(
        shown.get(Stat::Stamina),
        stamina + 20.0,
        "the readout is the sheet"
    );

    simulation.step_many(3 * 60);
    assert_eq!(simulation.character_stats().get(Stat::Stamina), stamina);
    assert!((max_health(&simulation) - health).abs() < 1e-3);
}

#[test]
fn stacking_adds_and_caps_and_reapplying_refreshes() {
    let (mut simulation, playground) = playground();
    let player = simulation.player().entity;
    let stamina = simulation.character_stats().get(Stat::Stamina);
    playground.apply_effect(&mut simulation, StatusKind::Fortitude, 10.0, 1);
    simulation.step_many(5 * 60);
    playground.apply_effect(&mut simulation, StatusKind::Fortitude, 10.0, 1);
    assert_eq!(stacks(&simulation, player, StatusKind::Fortitude), 2);
    assert_eq!(
        remaining(&simulation, player, StatusKind::Fortitude),
        Some(10.0),
        "refreshed to the new duration"
    );
    playground.apply_effect(&mut simulation, StatusKind::Fortitude, 10.0, 5);
    assert_eq!(
        stacks(&simulation, player, StatusKind::Fortitude),
        3,
        "capped at three"
    );
    assert_eq!(
        simulation.character_stats().get(Stat::Stamina),
        stamina + 60.0
    );
}

#[test]
fn removing_an_effect_by_hand_takes_its_stats_with_it() {
    let (mut simulation, playground) = playground();
    let player = simulation.player().entity;
    let stamina = simulation.character_stats().get(Stat::Stamina);
    playground.apply_effect(&mut simulation, StatusKind::WitheringPlague, 60.0, 2);
    assert_eq!(
        simulation.character_stats().get(Stat::Stamina),
        stamina - 30.0
    );
    assert!(playground.remove_effect(&mut simulation, StatusKind::WitheringPlague));
    assert_eq!(
        remaining(&simulation, player, StatusKind::WitheringPlague),
        None
    );
    assert_eq!(simulation.character_stats().get(Stat::Stamina), stamina);
    assert!(simulation.events().iter().any(|event| matches!(
        event,
        CombatEvent::StatusRemoved {
            status: StatusKind::WitheringPlague,
            ..
        }
    )));
}

#[test]
fn the_same_effect_does_the_same_thing_to_the_player_and_to_a_monster() {
    let (mut simulation, mut playground) = playground();
    let player = simulation.player().entity;
    let skeleton = monster(&mut simulation, &mut playground, EnemyKind::SkeletonWarrior);
    for target in [player, skeleton] {
        playground.set_effect_target(target);
        playground.apply_effect(&mut simulation, StatusKind::Crippled, 30.0, 1);
        playground.apply_effect(&mut simulation, StatusKind::MagicVulnerability, 30.0, 2);
        playground.apply_effect(&mut simulation, StatusKind::CurseOfWeakness, 30.0, 1);
    }
    let theirs = simulation.effect_stats(skeleton).expect("a monster");
    let mine = simulation.effect_stats(player).expect("a player");
    for stats in [theirs, mine] {
        assert!((stats.move_speed_multiplier - 0.6).abs() < 1e-5);
        assert!((stats.damage_dealt - 0.75).abs() < 1e-5);
        let fire = stats.damage_taken[1];
        assert_eq!(fire.0, DamageType::Fire);
        assert!((fire.1 - 1.30).abs() < 1e-5, "{fire:?}");
    }
    assert!(theirs.sheet.is_none(), "a monster has no character sheet");
}

#[test]
fn a_vulnerable_monster_really_takes_more_and_a_cursed_one_really_deals_less() {
    let (mut simulation, mut playground) = playground();
    let skeleton = monster(&mut simulation, &mut playground, EnemyKind::GraveBrute);
    let health = |simulation: &Simulation| {
        simulation
            .world()
            .health(skeleton)
            .expect("a monster")
            .current()
    };
    let before = health(&simulation);
    simulation.apply_damage(
        EntityId::INVALID,
        skeleton,
        100.0,
        AbilityId::NONE,
        DamageType::Fire,
    );
    let plain = before - health(&simulation);
    playground.apply_effect(&mut simulation, StatusKind::MagicVulnerability, 30.0, 1);
    let before = health(&simulation);
    simulation.apply_damage(
        EntityId::INVALID,
        skeleton,
        100.0,
        AbilityId::NONE,
        DamageType::Fire,
    );
    let vulnerable = before - health(&simulation);
    assert!(
        (vulnerable / plain - 1.15).abs() < 1e-3,
        "{vulnerable} against {plain}"
    );

    // Its own blows, through the player's ordinary incoming pipeline.
    let player = simulation.player().entity;
    let player_health = |simulation: &Simulation| {
        simulation
            .world()
            .health(player)
            .expect("a player")
            .current()
    };
    let dealt = |simulation: &mut Simulation| {
        let before = player_health(simulation);
        simulation.deliver_attack(skeleton, 40.0, AbilityId::NONE, DamageType::Nature);
        before - player_health(simulation)
    };
    let normal = dealt(&mut simulation);
    playground.apply_effect(&mut simulation, StatusKind::CurseOfWeakness, 30.0, 1);
    let cursed = dealt(&mut simulation);
    assert!(
        (cursed / normal - 0.75).abs() < 1e-3,
        "{cursed} against {normal}"
    );
}

#[test]
fn a_slowed_monster_really_walks_slower() {
    let walked = |slow: bool| {
        let (mut simulation, mut playground) = playground();
        let ghoul = monster(&mut simulation, &mut playground, EnemyKind::CryptGhoul);
        if slow {
            playground.apply_effect(&mut simulation, StatusKind::Crippled, 30.0, 1);
        }
        // Walk away so it gives chase, and measure how far it gets in a second.
        let away = simulation.player_position() + Vec3::new(0.0, 0.0, 40.0);
        simulation.push_command(engine_game::command::PlayerCommand::Teleport { position: away });
        simulation.apply_damage(
            simulation.player().entity,
            ghoul,
            1.0,
            AbilityId::NONE,
            DamageType::Physical,
        );
        simulation.step_many(90);
        let start = simulation.world().position(ghoul).expect("a ghoul");
        simulation.step_many(60);
        let end = simulation.world().position(ghoul).expect("a ghoul");
        (end - start).length()
    };
    let (fast, slow) = (walked(false), walked(true));
    assert!(fast > 1.0, "the ghoul gave chase: {fast}");
    assert!((slow / fast - 0.6).abs() < 0.08, "{slow} against {fast}");
}

#[test]
fn war_stomp_and_exposure_keep_their_numbers_through_the_registry() {
    let (mut simulation, mut playground) = playground();
    let skeleton = monster(&mut simulation, &mut playground, EnemyKind::SkeletonWarrior);
    playground.apply_effect(&mut simulation, StatusKind::Sundered, 30.0, 8);
    playground.apply_effect(&mut simulation, StatusKind::NaturesExposure, 30.0, 15);
    simulation.step_once();
    let stats = simulation.effect_stats(skeleton).expect("a monster");
    assert!(
        (stats.attack_speed - 0.84).abs() < 1e-5,
        "{}",
        stats.attack_speed
    );
    assert!((stats.damage_taken[3].1 - 1.45).abs() < 1e-5);
    assert!((stats.damage_taken[2].1 - 1.45).abs() < 1e-5);
    assert!(
        (stats.damage_taken[0].1 - 1.0).abs() < 1e-5,
        "physical untouched"
    );
}

// ---------------------------------------------------------------------------------------
// Dispelling with the real healer

#[test]
fn the_druids_real_cleanse_takes_off_what_its_schools_name_and_leaves_the_rest() {
    let (mut simulation, playground) = playground();
    assert!(simulation.set_hero(Hero::Druid));
    simulation.step_many(2);
    let player = simulation.player().entity;
    let stamina = simulation.character_stats().get(Stat::Stamina);
    for kind in [
        StatusKind::Poisoned,
        StatusKind::CurseOfWeakness,
        StatusKind::WitheringPlague,
        StatusKind::MagicVulnerability,
        StatusKind::Rended,
        StatusKind::Fortitude,
    ] {
        playground.apply_effect(&mut simulation, kind, 60.0, 1);
    }
    assert_eq!(
        simulation.character_stats().get(Stat::Stamina),
        stamina + 20.0 - 15.0
    );
    let mana = simulation.druid().mana.current();
    simulation.player_mut().target = Some(player);
    simulation
        .cast(FireAbility::NaturesCleanse)
        .expect("the cleanse is castable on yourself");
    let dispelled: Vec<StatusKind> = simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Dispelled { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    // Measured before the next tick's regeneration.
    let spent = mana - simulation.druid().mana.current();
    simulation.step_once();

    for gone in [
        StatusKind::Poisoned,
        StatusKind::CurseOfWeakness,
        StatusKind::WitheringPlague,
        StatusKind::MagicVulnerability,
    ] {
        assert!(dispelled.contains(&gone), "{gone:?} was not dispelled");
        assert_eq!(remaining(&simulation, player, gone), None, "{gone:?}");
    }
    assert!(
        remaining(&simulation, player, StatusKind::Rended).is_some(),
        "a wound stays"
    );
    assert!(
        remaining(&simulation, player, StatusKind::Fortitude).is_some(),
        "a buff stays"
    );
    assert_eq!(
        simulation.character_stats().get(Stat::Stamina),
        stamina + 20.0,
        "the plague's Stamina came back with it"
    );
    let cost = engine_game::druid::Druid::mana_cost(FireAbility::NaturesCleanse);
    assert!(
        (spent - cost).abs() < 1.0,
        "spent {spent}, the ability costs {cost}"
    );
}

#[test]
fn a_friendly_party_member_is_a_target_and_can_be_cleansed() {
    let (mut simulation, mut playground) = playground();
    assert!(simulation.set_hero(Hero::Druid));
    simulation.step_many(2);
    let mut targets = Vec::new();
    playground.effect_targets(&simulation, &mut targets);
    let ally = targets
        .iter()
        .find(|target| target.role == TargetRole::Friendly)
        .expect("the healing dummy is in the party")
        .entity;
    assert_eq!(
        simulation.world().kind(ally),
        Some(EntityKind::HealingDummy)
    );
    playground.set_effect_target(ally);
    playground.apply_effect(&mut simulation, StatusKind::Poisoned, 60.0, 1);
    simulation.player_mut().target = Some(ally);
    simulation
        .cast(FireAbility::NaturesCleanse)
        .expect("the cleanse reaches the ally");
    assert_eq!(remaining(&simulation, ally, StatusKind::Poisoned), None);
}

// ---------------------------------------------------------------------------------------
// Many at once, and the resets

#[test]
fn every_applicable_effect_fits_on_one_body_at_once() {
    let (mut simulation, mut playground) = playground();
    let skeleton = monster(&mut simulation, &mut playground, EnemyKind::CryptArcher);
    let applicable = StatusKind::ALL
        .into_iter()
        .filter(|kind| kind.definition().applicable)
        .count();
    assert_eq!(playground.add_test_effects(&mut simulation), applicable);
    let carried = simulation
        .world()
        .statuses(skeleton)
        .expect("a monster")
        .len();
    assert_eq!(carried, applicable);
    // The stackable ones are at their full stack.
    assert_eq!(
        stacks(&simulation, skeleton, StatusKind::NaturesExposure),
        15
    );
    simulation.step_many(60);
    assert!(simulation.world().is_alive(skeleton) || simulation.world().contains(skeleton));
}

#[test]
fn the_target_list_follows_the_encounter() {
    let (mut simulation, mut playground) = playground();
    let skeleton = monster(&mut simulation, &mut playground, EnemyKind::SkeletonWarrior);
    let mut targets = Vec::new();
    playground.effect_targets(&simulation, &mut targets);
    assert_eq!(targets[0].role, TargetRole::Player);
    assert!(targets.iter().any(|target| target.entity == skeleton));
    simulation.slay(skeleton);
    playground.observe(&simulation);
    playground.effect_targets(&simulation, &mut targets);
    assert!(
        targets.iter().all(|target| target.entity != skeleton),
        "the dead are not offered"
    );
    assert_eq!(
        playground.effect_target(&simulation),
        simulation.player().entity,
        "the target falls back to the player"
    );
}

#[test]
fn reset_player_clears_effects_and_their_stats() {
    let (mut simulation, mut playground) = playground();
    let player = simulation.player().entity;
    let stamina = simulation.character_stats().get(Stat::Stamina);
    playground.add_test_effects(&mut simulation);
    assert!(simulation.world().statuses(player).expect("a player").len() > 10);
    playground.reset_player(&mut simulation);
    assert!(
        simulation
            .world()
            .statuses(player)
            .expect("a player")
            .is_empty()
    );
    assert_eq!(simulation.character_stats().get(Stat::Stamina), stamina);
    let stats = simulation.effect_stats(player).expect("a player");
    assert!((stats.move_speed_multiplier - 1.0).abs() < 1e-6);
    assert!((stats.damage_dealt - 1.0).abs() < 1e-6);
}

#[test]
fn reset_encounter_leaves_no_monster_effect_anywhere() {
    let (mut simulation, mut playground) = playground();
    let baseline = simulation.world().len();
    let skeleton = monster(&mut simulation, &mut playground, EnemyKind::SkeletonWarrior);
    playground.add_test_effects(&mut simulation);
    playground.reset_encounter(&mut simulation);
    assert!(!simulation.world().contains(skeleton));
    assert_eq!(simulation.world().len(), baseline);
    assert_eq!(
        playground.effect_target(&simulation),
        simulation.player().entity
    );
    for entity in simulation.world().entities() {
        let statuses = simulation.world().statuses(entity).expect("statuses");
        assert!(statuses.iter().all(|status| status.source != skeleton));
    }
}

#[test]
fn the_whole_effect_workflow_repeats() {
    let (mut simulation, mut playground) = playground();
    let player = simulation.player().entity;
    let stamina = simulation.character_stats().get(Stat::Stamina);
    for _ in 0..4 {
        let skeleton = monster(&mut simulation, &mut playground, EnemyKind::GraveAcolyte);
        playground.add_test_effects(&mut simulation);
        playground.set_effect_target(player);
        playground.apply_effect(&mut simulation, StatusKind::Fortitude, 1.0, 1);
        playground.apply_effect(&mut simulation, StatusKind::Poisoned, 30.0, 1);
        simulation.step_many(2 * 60);
        assert_eq!(remaining(&simulation, player, StatusKind::Fortitude), None);
        assert_eq!(playground.clear_effects(&mut simulation, false), 1);
        playground.reset_encounter(&mut simulation);
        playground.reset_player(&mut simulation);
        assert!(!simulation.world().contains(skeleton));
        assert!(
            simulation
                .world()
                .statuses(player)
                .expect("a player")
                .is_empty()
        );
        assert_eq!(simulation.character_stats().get(Stat::Stamina), stamina);
    }
}
