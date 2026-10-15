//! The Druid, asserted against a real arena: Mana, Astral Power, and thirteen abilities.
//!
//! What a unit test on the tables cannot see: that arming the resource actually empowers the
//! next compatible cast and nothing else, that an empowered Moonfire outlives its duration
//! and still dies with its target, that a heal over time pays thirty per cent more on every
//! tick and exactly once, and that Nature's Exposure amplifies damage the Druid did not deal.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::{AbilityId, Motion, TargetMode};
use engine_game::arena::{self, ArenaConfig};
use engine_game::combat::DamageType;
use engine_game::druid::tuning::*;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::{AbilityError, FireAbility};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::{DispelSchool, Status, StatusKind};
use engine_game::world::EntityKind;

// ------------------------------------------------------------------------ helpers

/// The training arena, played as the Druid, with the nearest dummy selected.
fn druid() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    assert!(simulation.set_hero(Hero::Druid));
    simulation.select_nearest_target();
    simulation
}

fn target(simulation: &Simulation) -> EntityId {
    simulation.player().target.expect("a dummy is selected")
}

/// The friendly dummy the main hall stands the player next to.
fn healing_dummy(simulation: &Simulation) -> EntityId {
    simulation
        .world()
        .entities()
        .find(|id| simulation.world().kind(*id) == Some(EntityKind::HealingDummy))
        .expect("the main hall has a healing dummy")
}

fn health(simulation: &Simulation, id: EntityId) -> f32 {
    simulation
        .world()
        .health(id)
        .map_or(0.0, |health| health.current())
}

fn stacks(simulation: &Simulation, id: EntityId, kind: StatusKind) -> u8 {
    simulation
        .world()
        .statuses(id)
        .map_or(0, |statuses| statuses.stacks(kind))
}

fn remaining(simulation: &Simulation, id: EntityId, kind: StatusKind) -> f32 {
    simulation
        .world()
        .statuses(id)
        .map_or(0.0, |statuses| statuses.remaining(kind))
}

fn has(simulation: &Simulation, id: EntityId, kind: StatusKind) -> bool {
    simulation
        .world()
        .statuses(id)
        .is_some_and(|statuses| statuses.has(kind))
}

/// Runs a cast to completion, however long it takes.
fn cast_through(simulation: &mut Simulation, ability: FireAbility) {
    simulation.step_until_ready();
    simulation.cast(ability).expect("the cast was refused");
    for _ in 0..600 {
        if simulation.casting().is_none() {
            break;
        }
        simulation.step_once();
    }
}

/// Fills the bar and presses the button, so the next compatible cast is empowered.
fn arm(simulation: &mut Simulation) {
    simulation.druid_mut().astral.gain(ASTRAL_MAXIMUM);
    simulation
        .cast(FireAbility::AstralPower)
        .expect("a full bar arms");
    assert!(simulation.druid().astral.is_armed());
    // Off the global cooldown the button started, so the next cast is not refused for it.
    simulation.step_until_ready();
}

/// The healing the player did, from the event stream rather than from the health bar: an
/// overheal lands as nothing and a health bar cannot tell it from a heal that never happened.
fn healing_done(simulation: &Simulation, ability: FireAbility) -> f32 {
    let id = AbilityId::from_fire(ability);
    simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Healing {
                amount, ability, ..
            } if *ability == id => Some(*amount),
            _ => None,
        })
        .sum()
}

/// Hurts the friendly dummy so a heal on it has somewhere to go, and selects it.
fn wounded_ally(simulation: &mut Simulation) -> EntityId {
    let ally = healing_dummy(simulation);
    if let Some(health) = simulation.world_mut().health_mut(ally) {
        // A share of what it *has*, not of its maximum: it already stands at half, and
        // ninety per cent of two million would kill it outright.
        let current = health.current();
        health.damage(current * 0.5);
    }
    simulation.player_mut().target = Some(ally);
    assert!(simulation.world().is_alive(ally), "the ally was killed");
    ally
}

// ------------------------------------------------------------------------ the character

#[test]
fn the_druid_is_an_intellect_character_with_the_universal_stats() {
    use engine_game::character::Stat;
    let simulation = druid();
    assert_eq!(simulation.hero().main_stat(), Stat::Intellect);
    let stats = simulation.character_stats();
    for stat in [
        Stat::Stamina,
        Stat::CriticalStrike,
        Stat::Haste,
        Stat::Versatility,
    ] {
        // Present on the sheet, whatever the value: the Druid uses the universal stats
        // rather than a set of its own.
        let _ = stats.get(stat);
    }
    assert!(stats.get(Stat::Intellect) > 0.0);
    assert_eq!(simulation.sheet().hero(), Hero::Druid);
}

#[test]
fn the_bar_is_ten_spells_two_buttons_and_an_ultimate() {
    let simulation = druid();
    let hero = simulation.hero();
    assert_eq!(hero.abilities().len(), 13);
    assert_eq!(hero.bar().iter().filter(|slot| slot.is_some()).count(), 10);
    assert_eq!(
        hero.extras().collect::<Vec<_>>(),
        [FireAbility::WildRush, FireAbility::AstralPower]
    );
    assert_eq!(hero.ultimate(), Some(FireAbility::Tranquility));
    // Every one of them can be put in an action bar slot except the Ultimate, which has a
    // place of its own — the same rule the Warrior's Last Stand follows.
    for ability in hero.abilities() {
        assert_eq!(
            ability.is_assignable(),
            !ability.is_ultimate(),
            "{ability:?}"
        );
    }
}

// ------------------------------------------------------------------------ mana

#[test]
fn mana_is_paid_by_the_heals_and_never_by_wrath() {
    let mut simulation = druid();
    let full = simulation.druid().mana.current();
    assert_eq!(full, MANA_MAXIMUM);

    cast_through(&mut simulation, FireAbility::Wrath);
    assert!(simulation.druid().mana.current() >= full, "wrath cost mana");
    assert_eq!(FireAbility::Wrath.definition().druid().mana_cost, 0.0);

    simulation.player_mut().target = Some(healing_dummy(&simulation));
    simulation.step_many(60);
    let before = simulation.druid().mana.current();
    cast_through(&mut simulation, FireAbility::Rejuvenation);
    assert!(
        (before - simulation.druid().mana.current() - REJUVENATION_MANA).abs() < 1.0,
        "rejuvenation cost {}",
        before - simulation.druid().mana.current()
    );
}

#[test]
fn a_spell_with_no_mana_for_it_is_refused_before_it_costs_anything() {
    let mut simulation = druid();
    simulation.druid_mut().mana.pay(MANA_MAXIMUM);
    simulation.player_mut().target = Some(healing_dummy(&simulation));

    assert_eq!(
        simulation.cast(FireAbility::Rejuvenation),
        Err(AbilityError::NotEnoughMana)
    );
    // And the global cooldown was not started by the refusal.
    assert!(simulation.mage().off_global_cooldown());
}

#[test]
fn wrath_restores_a_share_of_maximum_mana_on_a_roll() {
    let mut simulation = druid();
    simulation.druid_mut().mana.pay(MANA_MAXIMUM * 0.5);
    let mut restored = 0.0_f32;
    for _ in 0..40 {
        cast_through(&mut simulation, FireAbility::Wrath);
        restored += simulation
            .events()
            .iter()
            .filter_map(|event| match event {
                CombatEvent::ManaRestored { amount, .. } => Some(*amount),
                _ => None,
            })
            .sum::<f32>();
        simulation.step_many(30);
    }
    assert!(restored > 0.0, "forty casts restored nothing");
    // Every restoration is exactly one per cent of the pool.
    let one = MANA_MAXIMUM * WRATH_MANA_RESTORE;
    assert!(
        (restored / one - (restored / one).round()).abs() < 1e-3,
        "{restored} is not a whole number of {one}"
    );
}

// ------------------------------------------------------------------------ astral power

#[test]
fn astral_power_is_generated_by_casting_and_never_spends_itself() {
    let mut simulation = druid();
    assert_eq!(simulation.druid().astral.value(), 0.0);

    cast_through(&mut simulation, FireAbility::Wrath);
    let generated = FireAbility::Wrath.definition().druid().astral_generation;
    assert!((simulation.druid().astral.value() - generated).abs() < 1e-3);

    // Filling it does nothing on its own: no proc, no free empowerment.
    simulation.druid_mut().astral.gain(ASTRAL_MAXIMUM);
    assert_eq!(simulation.druid().astral.value(), ASTRAL_MAXIMUM);
    assert!(!simulation.druid().astral.is_armed());
    simulation.step_many(60);
    cast_through(&mut simulation, FireAbility::Moonfire);
    assert_eq!(
        simulation.druid().astral.value(),
        ASTRAL_MAXIMUM,
        "a full bar was spent without being pressed"
    );
}

#[test]
fn the_button_arms_the_next_compatible_spell_and_that_spell_pays_for_it() {
    let mut simulation = druid();
    assert_eq!(
        simulation.cast(FireAbility::AstralPower),
        Err(AbilityError::NotEnoughAstralPower),
        "an empty bar armed"
    );

    arm(&mut simulation);
    assert_eq!(
        simulation.druid().astral.value(),
        ASTRAL_MAXIMUM,
        "arming spent the bar"
    );

    cast_through(&mut simulation, FireAbility::Moonfire);
    assert!(!simulation.druid().astral.is_armed());
    // The spend, and then what Moonfire itself generated.
    let expected = ASTRAL_MAXIMUM - ASTRAL_SPEND
        + FireAbility::Moonfire.definition().druid().astral_generation;
    assert!((simulation.druid().astral.value() - expected).abs() < 1e-3);
}

#[test]
fn the_incompatible_abilities_never_consume_the_arming() {
    for ability in [
        FireAbility::NaturesCleanse,
        FireAbility::WildRush,
        FireAbility::AstralPower,
    ] {
        assert!(
            !ability.definition().druid().astral_empowerable,
            "{ability:?} is empowerable"
        );
    }

    let mut simulation = druid();
    arm(&mut simulation);
    simulation.player_mut().target = Some(healing_dummy(&simulation));
    cast_through(&mut simulation, FireAbility::WildRush);
    assert!(simulation.druid().astral.is_armed(), "wild rush ate it");
    simulation.step_many(60);
    cast_through(&mut simulation, FireAbility::NaturesCleanse);
    assert!(
        simulation.druid().astral.is_armed(),
        "nature's cleanse ate it"
    );
}

// ------------------------------------------------------------------------ moonfire

#[test]
fn moonfire_hits_and_then_burns_for_eighteen_seconds() {
    let mut simulation = druid();
    let victim = target(&simulation);
    let before = health(&simulation, victim);
    cast_through(&mut simulation, FireAbility::Moonfire);

    let after = health(&simulation, victim);
    let direct = before - after;
    assert!(direct > 0.0, "moonfire hit nothing");
    assert!((remaining(&simulation, victim, StatusKind::Moonfire) - 18.0).abs() < 0.2);

    // Eighteen seconds later the burn is gone and it dealt what its row says.
    simulation.step_many(19 * 60);
    assert!(!has(&simulation, victim, StatusKind::Moonfire), "it stayed");
    let burnt = after - health(&simulation, victim);
    let per_tick =
        FireAbility::Moonfire.definition().tick_coefficient * simulation.mage().stats().spell_power;
    // Eighteen seconds at one tick every two.
    assert!(
        (burnt - per_tick * 9.0).abs() < per_tick * 0.5,
        "burnt {burnt}, expected about {}",
        per_tick * 9.0
    );
}

#[test]
fn an_astral_moonfire_hits_half_again_as_hard_and_never_expires() {
    let mut plain = druid();
    let victim = target(&plain);
    let before = health(&plain, victim);
    cast_through(&mut plain, FireAbility::Moonfire);
    let plain_direct = before - health(&plain, victim);

    let mut empowered = druid();
    let victim = target(&empowered);
    arm(&mut empowered);
    let before = health(&empowered, victim);
    cast_through(&mut empowered, FireAbility::Moonfire);
    let astral_direct = before - health(&empowered, victim);

    assert!(
        (astral_direct / plain_direct - (1.0 + ASTRAL_MOONFIRE_DAMAGE)).abs() < 0.05,
        "{astral_direct} against {plain_direct}"
    );

    // And the burn it left is worth half again as much per tick, and does not expire.
    let plain_tick = plain
        .world()
        .statuses(victim)
        .and_then(|statuses| statuses.get(StatusKind::Moonfire))
        .map(|status| status.damage_per_tick);
    let astral_tick = empowered
        .world()
        .statuses(victim)
        .and_then(|statuses| statuses.get(StatusKind::Moonfire))
        .map(|status| status.damage_per_tick);
    let (plain_tick, astral_tick) = (plain_tick.expect("a burn"), astral_tick.expect("a burn"));
    assert!((astral_tick / plain_tick - 1.5).abs() < 1e-3);

    // Five minutes later it is still there and still ticking.
    let before = health(&empowered, victim);
    empowered.step_many(300 * 60);
    assert!(
        has(&empowered, victim, StatusKind::Moonfire),
        "the astral burn expired"
    );
    assert!(health(&empowered, victim) < before, "it stopped ticking");
}

#[test]
fn an_astral_moonfire_dies_with_its_target_and_with_the_encounter() {
    let mut simulation = druid();
    let victim = target(&simulation);
    arm(&mut simulation);
    cast_through(&mut simulation, FireAbility::Moonfire);
    assert!(has(&simulation, victim, StatusKind::Moonfire));

    simulation.slay(victim);
    assert!(
        !has(&simulation, victim, StatusKind::Moonfire),
        "it outlived its target"
    );

    // And again, cleared by the reset rather than by a death.
    simulation.reset_dummies();
    simulation.step_once();
    simulation.select_nearest_target();
    let victim = target(&simulation);
    arm(&mut simulation);
    cast_through(&mut simulation, FireAbility::Moonfire);
    assert!(has(&simulation, victim, StatusKind::Moonfire));

    simulation.reset_dummies();
    simulation.step_once();
    assert!(
        !has(&simulation, victim, StatusKind::Moonfire),
        "it survived an encounter reset"
    );
    assert_eq!(simulation.druid().astral.value(), 0.0, "the bar survived");
}

// ------------------------------------------------------------------------ sunfire

#[test]
fn sunfire_burns_the_pack_around_the_target_for_eighteen_seconds() {
    let config = ArenaConfig::default();
    let mut simulation = arena::training_arena(&config);
    simulation.set_hero(Hero::Druid);
    simulation.step_once();
    // The group, which stands inside one radius of itself.
    let group: Vec<EntityId> = simulation
        .world()
        .entities()
        .filter(|id| simulation.world().kind(*id) == Some(EntityKind::Dummy))
        .collect();
    let primary = group
        .iter()
        .copied()
        .max_by(|a, b| {
            let position = |id: EntityId| simulation.world().position(id).unwrap_or(Vec3::ZERO).x;
            position(*a).total_cmp(&position(*b))
        })
        .expect("the arena has dummies");
    simulation.player_mut().target = Some(primary);

    cast_through(&mut simulation, FireAbility::Sunfire);
    let burning = group
        .iter()
        .filter(|id| has(&simulation, **id, StatusKind::Sunfire))
        .count();
    assert!(burning > 1, "sunfire hit only {burning}");
    assert!((remaining(&simulation, primary, StatusKind::Sunfire) - 18.0).abs() < 0.2);
    // The same burn as Moonfire's, per the design.
    assert_eq!(
        FireAbility::Sunfire.definition().tick_coefficient,
        FireAbility::Moonfire.definition().tick_coefficient
    );
}

#[test]
fn an_astral_sunfire_hits_harder_and_turns_the_packs_damage_into_healing() {
    let mut plain = druid();
    let victim = target(&plain);
    let before = health(&plain, victim);
    cast_through(&mut plain, FireAbility::Sunfire);
    let plain_direct = before - health(&plain, victim);

    let mut simulation = druid();
    let victim = target(&simulation);
    arm(&mut simulation);
    let before = health(&simulation, victim);
    cast_through(&mut simulation, FireAbility::Sunfire);
    let astral_direct = before - health(&simulation, victim);
    assert!(
        (astral_direct / plain_direct - (1.0 + ASTRAL_SUNFIRE_DAMAGE)).abs() < 0.05,
        "{astral_direct} against {plain_direct}"
    );
    assert!(has(&simulation, victim, StatusKind::AstralSunfire));
    assert!(
        (remaining(&simulation, victim, StatusKind::AstralSunfire) - ASTRAL_SUNFIRE_DURATION).abs()
            < 0.2
    );

    // Damage dealt to it now heals the party for a share of what landed. The player is hurt
    // well past what the conversion can restore, so what is measured is the conversion rather
    // than the health bar's ceiling.
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        let current = health.current();
        health.damage(current * 0.9);
    }
    let wounded = health(&simulation, player);
    simulation.step_many(1);
    let dealt = 100.0;
    simulation.apply_damage(player, victim, dealt, AbilityId::NONE, DamageType::Nature);
    let healed = health(&simulation, player) - wounded;
    assert!(
        (healed - dealt * ASTRAL_SUNFIRE_CONVERSION).abs() < 1.0,
        "healed {healed} from {dealt}"
    );
}

#[test]
fn the_conversion_never_feeds_itself() {
    let mut simulation = druid();
    let victim = target(&simulation);
    arm(&mut simulation);
    cast_through(&mut simulation, FireAbility::Sunfire);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        let current = health.current();
        health.damage(current * 0.9);
    }

    simulation.step_many(1);
    let before = health(&simulation, player);
    simulation.apply_damage(player, victim, 100.0, AbilityId::NONE, DamageType::Nature);
    let healed = health(&simulation, player) - before;
    assert!(
        (healed - 100.0 * ASTRAL_SUNFIRE_CONVERSION).abs() < 1.0,
        "{healed}"
    );
    // **One conversion per party member per instance of damage, and no more.** A loop would
    // pay out again and again for the one hit; healing is not damage, so nothing the
    // conversion produced can come back round and start another.
    let conversions = simulation
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::Healing { ability, .. }
                    if *ability == AbilityId::from_fire(FireAbility::Sunfire)
            )
        })
        .count();
    assert_eq!(
        conversions,
        simulation.party().len(),
        "the conversion ran {conversions} times for a party of {}",
        simulation.party().len()
    );
}

// ------------------------------------------------------------------------ wrath

#[test]
fn wrath_stacks_natures_exposure_to_ten_and_no_further() {
    let mut simulation = druid();
    let victim = target(&simulation);
    for _ in 0..20 {
        cast_through(&mut simulation, FireAbility::Wrath);
        simulation.step_many(10);
    }
    assert_eq!(
        stacks(&simulation, victim, StatusKind::NaturesExposure),
        NATURES_EXPOSURE_MAX_STACKS
    );
    // Ten stacks at two per cent each is the design's twenty.
    assert_eq!(NATURES_EXPOSURE_MAX_STACKS, 10);
    assert!(
        (engine_game::druid::Druid::exposure_multiplier(10) - 1.20).abs() < 1e-6,
        "the ceiling moved"
    );
}

#[test]
fn natures_exposure_amplifies_nature_and_arcane_from_any_source() {
    let mut simulation = druid();
    let victim = target(&simulation);
    let attacker = simulation.player().entity;

    let plain = {
        simulation.step_many(1);
        let before = health(&simulation, victim);
        simulation.apply_damage(
            attacker,
            victim,
            1_000.0,
            AbilityId::NONE,
            DamageType::Nature,
        );
        before - health(&simulation, victim)
    };

    // Five stacks, applied directly: the debuff is the status, not the ability.
    let status = Status::new(
        StatusKind::NaturesExposure,
        attacker,
        AbilityId::from_fire(FireAbility::Wrath),
        NATURES_EXPOSURE_DURATION,
        0.0,
    );
    for _ in 0..5 {
        if let Some(statuses) = simulation.world_mut().statuses_mut(victim) {
            statuses.apply(status);
        }
    }
    assert_eq!(stacks(&simulation, victim, StatusKind::NaturesExposure), 5);

    for (school, expected) in [
        (DamageType::Nature, 1.0 + 5.0 * NATURES_EXPOSURE_PER_STACK),
        (DamageType::Arcane, 1.0 + 5.0 * NATURES_EXPOSURE_PER_STACK),
        (DamageType::Fire, 1.0),
        (DamageType::Physical, 1.0),
    ] {
        simulation.step_many(1);
        let before = health(&simulation, victim);
        simulation.apply_damage(attacker, victim, 1_000.0, AbilityId::NONE, school);
        let landed = before - health(&simulation, victim);
        assert!(
            (landed / plain - expected).abs() < 1e-3,
            "{school:?}: {landed} against {plain}"
        );
    }
}

#[test]
fn the_exposure_refreshes_rather_than_falling_off_mid_rotation() {
    let mut simulation = druid();
    let victim = target(&simulation);
    cast_through(&mut simulation, FireAbility::Wrath);
    simulation.step_many(10 * 60);
    let before = remaining(&simulation, victim, StatusKind::NaturesExposure);
    assert!(before < NATURES_EXPOSURE_DURATION);

    cast_through(&mut simulation, FireAbility::Wrath);
    let after = remaining(&simulation, victim, StatusKind::NaturesExposure);
    assert!(
        (after - NATURES_EXPOSURE_DURATION).abs() < 0.2,
        "{after} after a refresh"
    );
    assert_eq!(stacks(&simulation, victim, StatusKind::NaturesExposure), 2);
}

#[test]
fn an_astral_wrath_spreads_the_debuff_ten_metres_and_never_twice_to_the_primary() {
    let config = ArenaConfig::default();
    let mut simulation = arena::training_arena(&config);
    simulation.set_hero(Hero::Druid);
    simulation.step_once();
    let group: Vec<EntityId> = simulation
        .world()
        .entities()
        .filter(|id| simulation.world().kind(*id) == Some(EntityKind::Dummy))
        .collect();
    // The group's own members stand three metres apart, well inside ten.
    let primary = *group.last().expect("the arena has dummies");
    simulation.player_mut().target = Some(primary);

    arm(&mut simulation);
    cast_through(&mut simulation, FireAbility::Wrath);
    assert!(has(
        &simulation,
        simulation.player().entity,
        StatusKind::AstralWrath
    ));

    let spread = group
        .iter()
        .filter(|id| **id != primary && stacks(&simulation, **id, StatusKind::NaturesExposure) > 0)
        .count();
    assert!(spread > 0, "the debuff never left the primary");
    assert_eq!(
        stacks(&simulation, primary, StatusKind::NaturesExposure),
        1,
        "the primary was stacked twice by one cast"
    );

    // Everything it reached carries the same one stack and the same duration.
    for id in &group {
        let count = stacks(&simulation, *id, StatusKind::NaturesExposure);
        if count == 0 {
            continue;
        }
        assert_eq!(count, 1, "{id:?}");
        assert!(
            (remaining(&simulation, *id, StatusKind::NaturesExposure) - NATURES_EXPOSURE_DURATION)
                .abs()
                < 0.3
        );
    }

    // Everything further than ten metres from the primary was left alone.
    let centre = simulation.world().position(primary).expect("a position");
    for id in &group {
        let Some(position) = simulation.world().position(*id) else {
            continue;
        };
        let gap = ((position.x - centre.x).powi(2) + (position.z - centre.z).powi(2)).sqrt();
        if gap > ASTRAL_WRATH_RADIUS + simulation.world().radius(*id) + 0.1 {
            assert_eq!(
                stacks(&simulation, *id, StatusKind::NaturesExposure),
                0,
                "something {gap} m away was stacked"
            );
        }
    }
}

// ------------------------------------------------------------------------ healing

/// Every healing spell, cast on a wounded ally, plain and empowered.
///
/// One test for the rule the whole healing kit shares: **thirty per cent, on every healing
/// event the spell produces, exactly once.**
#[test]
fn every_astral_healing_spell_pays_thirty_per_cent_more_and_no_more() {
    for ability in [
        FireAbility::Rejuvenation,
        FireAbility::Regrowth,
        FireAbility::Lifebloom,
        FireAbility::WildGrowth,
        FireAbility::Nourish,
    ] {
        let plain = healing_run(ability, false);
        let empowered = healing_run(ability, true);
        assert!(plain > 0.0, "{ability:?} healed nothing");
        assert!(
            (empowered / plain - (1.0 + ASTRAL_HEAL_BONUS)).abs() < 0.02,
            "{ability:?}: {empowered} against {plain}"
        );
    }
}

/// Casts one healing spell on a wounded ally and returns everything it healed for,
/// ticks and blooms included.
fn healing_run(ability: FireAbility, empowered: bool) -> f32 {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    if empowered {
        arm(&mut simulation);
    }
    // A big pool of Mana, so a run cannot be refused for the cost.
    simulation.druid_mut().mana.restore();
    cast_through(&mut simulation, ability);
    // Long enough for the whole effect: the longest heal over time in the kit is eighteen
    // seconds, and Lifebloom's bloom lands on the tick after that.
    let mut total = healing_done(&simulation, ability);
    for _ in 0..20 {
        simulation.step_many(60);
        total += healing_done(&simulation, ability);
    }
    let _ = ally;
    total
}

#[test]
fn a_heal_over_time_ticks_for_its_whole_duration() {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    cast_through(&mut simulation, FireAbility::Rejuvenation);
    assert!(has(&simulation, ally, StatusKind::Rejuvenation));
    assert!((remaining(&simulation, ally, StatusKind::Rejuvenation) - 18.0).abs() < 0.2);

    let mut ticks = 0;
    for _ in 0..20 {
        simulation.step_many(60);
        ticks += simulation
            .events()
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    CombatEvent::Healing { ability, .. }
                        if *ability == AbilityId::from_fire(FireAbility::Rejuvenation)
                )
            })
            .count();
    }
    // Eighteen seconds, one tick every three.
    assert_eq!(ticks, 6, "{ticks} ticks");
    assert!(!has(&simulation, ally, StatusKind::Rejuvenation));
}

#[test]
fn lifebloom_blooms_when_it_runs_out_and_not_when_it_is_eaten() {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    cast_through(&mut simulation, FireAbility::Lifebloom);

    let mut bloomed = 0.0;
    let mut last_tick = 0.0;
    for _ in 0..20 {
        simulation.step_many(60);
        let healed = healing_done(&simulation, FireAbility::Lifebloom);
        if !has(&simulation, ally, StatusKind::Lifebloom) && healed > last_tick {
            bloomed = healed;
            break;
        }
        if healed > 0.0 {
            last_tick = healed;
        }
    }
    assert!(bloomed > last_tick, "nothing bloomed: {bloomed}");

    // Eaten by Swiftmend instead: no bloom.
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    cast_through(&mut simulation, FireAbility::Lifebloom);
    simulation.step_many(60);
    cast_through(&mut simulation, FireAbility::Swiftmend);
    assert!(!has(&simulation, ally, StatusKind::Lifebloom));
    simulation.step_many(60);
    assert_eq!(
        healing_done(&simulation, FireAbility::Lifebloom),
        0.0,
        "a consumed lifebloom bloomed"
    );
}

#[test]
fn swiftmend_needs_a_heal_over_time_and_eats_exactly_one() {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    assert_eq!(
        simulation.cast(FireAbility::Swiftmend),
        Err(AbilityError::NoHealOverTime)
    );

    cast_through(&mut simulation, FireAbility::Rejuvenation);
    simulation.step_many(60);
    cast_through(&mut simulation, FireAbility::Lifebloom);
    simulation.step_many(60);
    assert_eq!(simulation.druid_hot_count(ally), 2);

    let before = health(&simulation, ally);
    cast_through(&mut simulation, FireAbility::Swiftmend);
    assert!(health(&simulation, ally) > before, "it healed nothing");
    assert_eq!(simulation.druid_hot_count(ally), 1, "it ate more than one");
}

#[test]
fn nourish_grows_with_every_heal_over_time_and_has_no_ceiling() {
    // Zero, one, and as many as the kit can put on one target: the multiplier is ten per
    // cent a piece all the way up.
    let plain = nourish_run(0, false);
    let one = nourish_run(1, false);
    let four = nourish_run(4, false);
    assert!(plain > 0.0);
    assert!((one / plain - 1.10).abs() < 0.02, "{one} against {plain}");
    assert!((four / plain - 1.40).abs() < 0.02, "{four} against {plain}");

    // And the empowerment is applied *after* that scaling.
    let four_astral = nourish_run(4, true);
    assert!(
        (four_astral / plain - 1.40 * (1.0 + ASTRAL_HEAL_BONUS)).abs() < 0.03,
        "{four_astral} against {plain}"
    );
}

/// Casts Nourish on an ally already carrying `hots` of the Druid's heals over time.
fn nourish_run(hots: usize, empowered: bool) -> f32 {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    let source = simulation.player().entity;
    for kind in [
        StatusKind::Rejuvenation,
        StatusKind::Regrowth,
        StatusKind::Lifebloom,
        StatusKind::WildGrowth,
    ]
    .into_iter()
    .take(hots)
    {
        // Applied directly, with no healing of their own, so what is measured is Nourish.
        let status = Status::new(
            kind,
            source,
            AbilityId::from_fire(FireAbility::Nourish),
            60.0,
            0.0,
        );
        if let Some(statuses) = simulation.world_mut().statuses_mut(ally) {
            statuses.apply(status);
        }
    }
    assert_eq!(simulation.druid_hot_count(ally), hots as u32);
    if empowered {
        arm(&mut simulation);
    }
    cast_through(&mut simulation, FireAbility::Nourish);
    healing_done(&simulation, FireAbility::Nourish)
}

#[test]
fn wild_growth_reaches_the_target_and_the_hurt_allies_around_them() {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        let max = health.max();
        health.damage(max * 0.5);
    }
    cast_through(&mut simulation, FireAbility::WildGrowth);

    assert!(has(&simulation, ally, StatusKind::WildGrowth));
    assert!(
        has(&simulation, player, StatusKind::WildGrowth),
        "the wounded caster standing beside them was missed"
    );
}

#[test]
fn tranquility_pulses_once_a_second_while_it_is_channelled() {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        let max = health.max();
        health.damage(max * 0.9);
    }

    simulation
        .cast(FireAbility::Tranquility)
        .expect("the channel was refused");
    assert!(simulation.casting().is_some(), "no channel started");

    let mut pulses = 0;
    for _ in 0..8 {
        simulation.step_many(60);
        // Counted on the ally rather than on the player: the friendly dummy has two million
        // points of health and can never be topped up, so every pulse that happened produced
        // an event. A pulse that healed a full-health player produces none, and a test that
        // counted those would be measuring the player's health bar rather than the channel.
        pulses += tranquility_events(&simulation, ally);
    }
    // Six seconds, one pulse a second.
    assert_eq!(pulses, 6, "{pulses} pulses");
    assert!(simulation.casting().is_none(), "the channel never ended");
    assert!(
        health(&simulation, player) > 0.0,
        "the caster was healed by their own channel"
    );
}

/// How many Tranquility heals landed on `target` in the current event batch.
fn tranquility_events(simulation: &Simulation, target: EntityId) -> usize {
    simulation
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event,
                CombatEvent::Healing { ability, target: healed, .. }
                    if *ability == AbilityId::from_fire(FireAbility::Tranquility)
                        && *healed == target
            )
        })
        .count()
}

#[test]
fn an_astral_tranquility_empowers_every_pulse_and_lengthens_nothing() {
    let plain = tranquility_run(false);
    let empowered = tranquility_run(true);
    assert!(plain.0 > 0.0);
    assert_eq!(plain.1, empowered.1, "the empowerment added pulses");
    assert!(
        (empowered.0 / plain.0 - (1.0 + ASTRAL_HEAL_BONUS)).abs() < 0.02,
        "{} against {}",
        empowered.0,
        plain.0
    );
}

/// Channels one Tranquility and returns what it healed the ally for, and in how many pulses.
///
/// Measured on the ally alone, for the reason the pulse count above is: a party member who
/// reaches full health stops producing healing events, and the channel is what is being
/// measured rather than the size of a health bar.
fn tranquility_run(empowered: bool) -> (f32, usize) {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    if empowered {
        arm(&mut simulation);
    }
    simulation
        .cast(FireAbility::Tranquility)
        .expect("the channel was refused");

    let mut healed = 0.0;
    let mut events = 0;
    for _ in 0..10 {
        simulation.step_many(60);
        healed += simulation
            .events()
            .iter()
            .filter_map(|event| match event {
                CombatEvent::Healing {
                    ability,
                    target,
                    amount,
                    ..
                } if *ability == AbilityId::from_fire(FireAbility::Tranquility)
                    && *target == ally =>
                {
                    Some(*amount)
                }
                _ => None,
            })
            .sum::<f32>();
        events += tranquility_events(&simulation, ally);
    }
    (healed, events)
}

// ------------------------------------------------------------------------ dispel

#[test]
fn natures_cleanse_removes_magic_and_poison_and_leaves_the_rest() {
    let mut simulation = druid();
    let ally = wounded_ally(&mut simulation);
    let source = simulation.player().entity;
    let apply = |simulation: &mut Simulation, kind: StatusKind| {
        let status = Status::new(kind, source, AbilityId::NONE, 30.0, 0.0);
        if let Some(statuses) = simulation.world_mut().statuses_mut(ally) {
            statuses.apply(status);
        }
    };
    apply(&mut simulation, StatusKind::Poisoned);
    apply(&mut simulation, StatusKind::Burning);
    apply(&mut simulation, StatusKind::NaturesExposure);
    apply(&mut simulation, StatusKind::Bleeding);
    apply(&mut simulation, StatusKind::Rejuvenation);

    cast_through(&mut simulation, FireAbility::NaturesCleanse);

    assert!(!has(&simulation, ally, StatusKind::Poisoned), "poison");
    assert!(!has(&simulation, ally, StatusKind::Burning), "magic");
    assert!(
        !has(&simulation, ally, StatusKind::NaturesExposure),
        "the other magic"
    );
    // A wound is not a spell, and a heal is not something to be rid of.
    assert!(has(&simulation, ally, StatusKind::Bleeding), "the bleed");
    assert!(has(&simulation, ally, StatusKind::Rejuvenation), "the heal");

    // And it said which school answered for each.
    let schools: Vec<DispelSchool> = simulation
        .events()
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Dispelled { school, .. } => Some(*school),
            _ => None,
        })
        .collect();
    assert!(schools.contains(&DispelSchool::Poison));
    assert!(schools.contains(&DispelSchool::Magic));
}

// ------------------------------------------------------------------------ wild rush

#[test]
fn wild_rush_is_six_seconds_of_speed_and_mitigation_and_then_nothing() {
    let mut simulation = druid();
    let player = simulation.player().entity;
    let base_speed = simulation.move_speed_multiplier();
    assert!((base_speed - 1.0).abs() < 1e-6);
    assert_eq!(simulation.defense_profile().damage_reduction, 0.0);

    cast_through(&mut simulation, FireAbility::WildRush);
    assert!(has(&simulation, player, StatusKind::WildRush));
    assert!((simulation.move_speed_multiplier() - 1.5).abs() < 1e-6);
    assert!((simulation.defense_profile().damage_reduction - WILD_RUSH_REDUCTION).abs() < 1e-6);
    assert!((remaining(&simulation, player, StatusKind::WildRush) - 6.0).abs() < 0.2);

    // Six seconds later everything is exactly as it was.
    simulation.step_many(7 * 60);
    assert!(!has(&simulation, player, StatusKind::WildRush));
    assert!((simulation.move_speed_multiplier() - base_speed).abs() < 1e-6);
    assert_eq!(simulation.defense_profile().damage_reduction, 0.0);
}

#[test]
fn wild_rush_actually_takes_thirty_per_cent_off_an_incoming_hit() {
    let mut simulation = druid();
    let attacker = target(&simulation);
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        let max = health.max();
        health.damage(max * 0.5);
    }

    simulation.step_many(1);
    let before = health(&simulation, player);
    simulation.deliver_attack(attacker, 100.0, AbilityId::NONE, DamageType::Nature);
    let plain = before - health(&simulation, player);

    cast_through(&mut simulation, FireAbility::WildRush);
    simulation.step_many(1);
    let before = health(&simulation, player);
    simulation.deliver_attack(attacker, 100.0, AbilityId::NONE, DamageType::Nature);
    let mitigated = before - health(&simulation, player);
    assert!(
        (mitigated / plain - (1.0 - WILD_RUSH_REDUCTION)).abs() < 1e-3,
        "{mitigated} against {plain}"
    );
}

// ------------------------------------------------------------------------ haste

#[test]
fn haste_makes_a_burn_and_a_heal_over_time_tick_more_often() {
    use engine_game::character::{Stat, StatBlock};
    let slow = tick_count(0.0);
    // Enough rating to be worth a clearly measurable amount of haste.
    let mut bonus = StatBlock::zero();
    bonus.set(Stat::Haste, 5_000.0);
    let fast = tick_count_with(bonus);
    assert!(
        fast > slow,
        "haste changed nothing: {fast} ticks against {slow}"
    );
}

fn tick_count(_haste: f32) -> usize {
    tick_count_with(engine_game::character::StatBlock::zero())
}

/// How many times a Moonfire ticks in twelve seconds at the given stat bonus.
fn tick_count_with(bonus: engine_game::character::StatBlock) -> usize {
    let mut simulation = druid();
    simulation.set_stat_bonus(bonus);
    let victim = target(&simulation);
    cast_through(&mut simulation, FireAbility::Moonfire);
    let mut ticks = 0;
    for _ in 0..12 {
        simulation.step_many(60);
        ticks += simulation
            .events()
            .iter()
            .filter(|event| {
                event.as_damage().is_some_and(|damage| {
                    damage.target == victim
                        && damage.ability == AbilityId::from_fire(FireAbility::Moonfire)
                })
            })
            .count();
    }
    ticks
}

// ------------------------------------------------------------------------ reset

#[test]
fn an_encounter_reset_clears_everything_the_druid_left_behind() {
    let mut simulation = druid();
    let victim = target(&simulation);
    let player = simulation.player().entity;

    cast_through(&mut simulation, FireAbility::Moonfire);
    simulation.step_many(30);
    cast_through(&mut simulation, FireAbility::Sunfire);
    simulation.step_many(30);
    cast_through(&mut simulation, FireAbility::Wrath);
    simulation.step_many(30);
    cast_through(&mut simulation, FireAbility::WildRush);
    simulation.step_many(30);
    simulation.player_mut().target = Some(healing_dummy(&simulation));
    cast_through(&mut simulation, FireAbility::Rejuvenation);
    simulation.druid_mut().astral.gain(ASTRAL_MAXIMUM);

    let ally = healing_dummy(&simulation);
    assert!(has(&simulation, victim, StatusKind::Moonfire));
    assert!(has(&simulation, player, StatusKind::WildRush));
    assert!(has(&simulation, ally, StatusKind::Rejuvenation));

    simulation.reset_dummies();
    simulation.step_once();

    for (id, kind) in [
        (victim, StatusKind::Moonfire),
        (victim, StatusKind::Sunfire),
        (victim, StatusKind::NaturesExposure),
        (player, StatusKind::WildRush),
        (player, StatusKind::AstralWrath),
        (ally, StatusKind::Rejuvenation),
    ] {
        assert!(!has(&simulation, id, kind), "{kind:?} survived the reset");
    }
    assert_eq!(simulation.druid().astral.value(), 0.0);
    assert!(!simulation.druid().astral.is_armed());
    assert_eq!(simulation.druid().mana.current(), MANA_MAXIMUM);
    // And the friendly dummy is back at half health rather than full, which is what makes it
    // a healing target again rather than a full bar.
    let health = simulation.world().health(ally).expect("it has health");
    assert!((health.fraction() - 0.5).abs() < 1e-3);
}

#[test]
fn swapping_away_from_the_druid_takes_its_state_with_it() {
    let mut simulation = druid();
    let player = simulation.player().entity;
    cast_through(&mut simulation, FireAbility::WildRush);
    simulation.druid_mut().astral.gain(ASTRAL_MAXIMUM);
    assert!(has(&simulation, player, StatusKind::WildRush));

    assert!(simulation.set_hero(Hero::FireMage));
    assert!(!has(&simulation, player, StatusKind::WildRush));
    assert_eq!(simulation.druid().astral.value(), 0.0);
    assert!((simulation.move_speed_multiplier() - 1.0).abs() < 1e-6);
}

#[test]
fn an_area_spell_never_lands_on_the_friendly_body_standing_beside_the_player() {
    // The arena has an ally in it now. Every area query in the game answers "living
    // hostiles", and this is the test that says so out loud.
    let mut simulation = druid();
    let ally = healing_dummy(&simulation);
    if let Some(health) = simulation.world_mut().health_mut(ally) {
        let current = health.current();
        health.damage(current * 0.5);
    }
    let before = health(&simulation, ally);

    // Sunfire is the Druid's area spell, and it is centred on an enemy the ally is nowhere
    // near — but the query is what is being tested, so the ally is moved on top of it first.
    let victim = target(&simulation);
    let Some(position) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    if let Some(body) = simulation.world_mut().body_mut(ally) {
        body.position = position + Vec3::new(1.0, 0.0, 0.0);
    }
    cast_through(&mut simulation, FireAbility::Sunfire);
    simulation.step_many(120);

    assert!(
        health(&simulation, ally) >= before,
        "the area spell hit the ally"
    );
    assert!(!has(&simulation, ally, StatusKind::Sunfire));
}

// ------------------------------------------------------------------------ the main hall

#[test]
fn the_main_hall_has_a_two_million_point_dummy_at_half_health_to_heal() {
    let simulation = druid();
    let ally = healing_dummy(&simulation);
    let health = simulation.world().health(ally).expect("it has health");
    assert!((health.max() - 2_000_000.0).abs() < 1.0, "{}", health.max());
    assert!((health.fraction() - 0.5).abs() < 1e-3);
    assert!(simulation.party().contains(ally));
    // Friendly: it cannot be attacked, and it is what a heal resolves to when it is
    // selected.
    assert!(!simulation.is_attackable(ally));
}

#[test]
fn a_heal_falls_back_to_the_caster_when_the_selection_is_an_enemy() {
    let mut simulation = druid();
    let player = simulation.player().entity;
    if let Some(health) = simulation.world_mut().health_mut(player) {
        let max = health.max();
        health.damage(max * 0.5);
    }
    // An enemy is selected, which is the ordinary state of a healer mid-fight.
    assert!(simulation.is_attackable(target(&simulation)));
    assert_eq!(simulation.friendly_target(), Some(player));

    let before = health(&simulation, player);
    cast_through(&mut simulation, FireAbility::Regrowth);
    assert!(
        health(&simulation, player) > before,
        "the heal landed on nobody"
    );
}

#[test]
fn a_moonfire_tick_is_announced_as_periodic_and_its_hit_is_not() {
    // What lets the combat text draw a tick smaller than a hit: the event says which it is,
    // because the ability id alone cannot — the burn ticks under Moonfire's own id.
    let mut simulation = druid();
    let victim = target(&simulation);
    simulation
        .cast(FireAbility::Moonfire)
        .expect("the cast was refused");
    let mut hits = 0;
    let mut ticks = 0;
    // The first pass reads what the cast itself emitted: an instant lands inside `cast`.
    for step in 0..=(6 * 60) {
        if step > 0 {
            simulation.step_once();
        }
        for event in simulation.events() {
            if let CombatEvent::Damage(damage) = event
                && damage.target == victim
            {
                assert_eq!(damage.ability, AbilityId::from_fire(FireAbility::Moonfire));
                if damage.periodic {
                    ticks += 1;
                    assert!(!damage.critical, "a tick never rolls a critical strike");
                } else {
                    hits += 1;
                }
            }
        }
    }
    assert_eq!(hits, 1, "one direct hit");
    assert!(ticks >= 2, "the burn ticked {ticks} times in six seconds");
}

#[test]
fn every_druid_heal_is_a_heal_motion_and_every_other_druid_row_is_a_plain_cast() {
    // The motion is data on the row, so the visual layer never matches on an ability's
    // identity: a heal on an ally plays the heal cast clips, a damage spell or a self buff
    // keeps the ordinary cast.
    for ability in FireAbility::ALL {
        if ability.hero() != Hero::Druid {
            continue;
        }
        let definition = ability.definition();
        let expected = if definition.target == TargetMode::Friendly {
            Motion::Heal
        } else {
            Motion::Cast
        };
        assert_eq!(definition.motion, expected, "{ability:?}");
    }
}
