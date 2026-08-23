//! The three playable heroes, asserted against a real arena.
//!
//! What a unit test on the ability table cannot see: that a swap actually changes what the
//! bar casts, that the Warrior's three abilities and the Druid's three do what their rows
//! say against live dummies, and that swapping does not leave the old hero's cast, cooldowns
//! or fire behind.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::entity::EntityId;
use engine_game::firemage::{ABILITY_SLOTS, AbilityError, FireAbility};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::world::EntityKind;

/// The training arena with the nearest dummy selected.
fn arena() -> Simulation {
    let mut simulation = arena::training_arena(&ArenaConfig::default());
    simulation.step_once();
    simulation.select_nearest_target();
    simulation
}

/// The same arena, already swapped to `hero`.
fn arena_as(hero: Hero) -> Simulation {
    let mut simulation = arena();
    assert!(simulation.set_hero(hero), "the swap did nothing");
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

/// Walks the player to within `metres` of its target, which is what a melee test needs and
/// what the Warrior's own Charge does in the game.
fn close_to_target(simulation: &mut Simulation, metres: f32) {
    let victim = target(simulation);
    let Some(destination) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    let player = simulation.player().entity;
    let Some(from) = simulation.world().position(player) else {
        panic!("the player has a position");
    };
    let mut offset = from - destination;
    offset.y = 0.0;
    let length = offset.length().max(1e-3);
    let placed = destination + offset * (metres / length);
    if let Some(body) = simulation.world_mut().body_mut(player) {
        body.position = placed;
    }
}

/// Runs a cast to completion, however long it takes.
fn cast_through(simulation: &mut Simulation, ability: FireAbility) {
    simulation.cast(ability).expect("the cast was refused");
    for _ in 0..300 {
        if simulation.casting().is_none() {
            break;
        }
        simulation.step_once();
    }
}

#[test]
fn a_swap_changes_which_abilities_the_bar_holds() {
    let mut simulation = arena();
    assert_eq!(simulation.hero(), Hero::FireMage);
    assert_eq!(simulation.hero().ability_at(0), Some(FireAbility::Firebolt));

    assert!(simulation.set_hero(Hero::Warrior));
    assert_eq!(
        simulation.hero().ability_at(0),
        Some(FireAbility::Recuperate)
    );

    // Slot four is still slot four, and the Druid fills it with its own spell.
    assert!(simulation.set_hero(Hero::Druid));
    assert_eq!(
        simulation.hero().ability_at(3),
        Some(FireAbility::Rejuvenation)
    );
    // A slot no hero fills is still refused rather than casting somebody else's spell.
    assert_eq!(simulation.hero().ability_at(ABILITY_SLOTS), None);

    // Swapping to the hero you already are changes nothing and says so.
    assert!(!simulation.set_hero(Hero::Druid));
}

#[test]
fn a_swap_drops_the_cast_in_progress_and_the_fire_on_the_ground() {
    // The one outcome that is certainly wrong is a mage's three-second Pyroblast still
    // landing after its caster became a Warrior.
    let mut simulation = arena();
    simulation.cast(FireAbility::Pyroblast).expect("in range");
    simulation.step_once();
    assert!(simulation.casting().is_some());

    let victim = target(&simulation);
    let before = health(&simulation, victim);

    assert!(simulation.set_hero(Hero::Druid));
    assert!(simulation.casting().is_none(), "the cast survived the swap");

    simulation.step_many(300);
    assert_eq!(
        health(&simulation, victim),
        before,
        "the abandoned cast landed anyway"
    );
    assert!(
        simulation.mage().zones.zones().is_empty(),
        "the mage's fire outlived the mage"
    );
}

#[test]
fn the_warriors_filler_only_reaches_five_metres() {
    let mut simulation = arena_as(Hero::Warrior);
    // The dummies stand well outside a melee reach, which is the whole point of the class
    // having a gap-closer.
    assert_eq!(
        simulation.cast(FireAbility::Recuperate),
        Err(AbilityError::OutOfRange)
    );

    close_to_target(&mut simulation, 3.0);
    let victim = target(&simulation);
    let before = health(&simulation, victim);
    cast_through(&mut simulation, FireAbility::Recuperate);
    assert!(
        health(&simulation, victim) < before,
        "the strike landed nothing"
    );
}

#[test]
fn the_warriors_stomp_catches_more_than_one_enemy() {
    let mut simulation = arena_as(Hero::Warrior);
    // The arena's dummies stand apart, so one is moved to stand beside the other: what is
    // being asserted is that a cone hits everything inside it, not how the arena is laid
    // out.
    let victim = target(&simulation);
    let Some(beside) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    let neighbour = simulation
        .world()
        .entities()
        .find(|id| *id != victim && simulation.world().kind(*id) == Some(EntityKind::Dummy))
        .expect("the arena has more than one dummy");
    if let Some(body) = simulation.world_mut().body_mut(neighbour) {
        body.position = beside + Vec3::new(0.9, 0.0, 0.0);
    }
    close_to_target(&mut simulation, 2.0);
    // Pointed by the *selection*, not by the mouse: War Stomp is a targeted cleave, and the
    // hundred-and-twenty-degree cone is measured from the line to whatever is selected.
    let hits = simulation
        .cast(FireAbility::WarStomp)
        .expect("the stomp was refused");
    assert!(hits >= 2, "a cleave that caught {hits} is a strike");
}

#[test]
fn a_shield_charge_runs_rather_than_teleports() {
    let mut simulation = arena_as(Hero::Warrior);
    let player = simulation.player().entity;
    let victim = target(&simulation);
    let Some(from) = simulation.world().position(player) else {
        panic!("the player has a position");
    };

    simulation.cast(FireAbility::ShieldCharge).expect("instant");
    // Nothing has happened yet: a charge is a *movement*, and on the tick it is pressed the
    // Warrior has not gone anywhere. This is the whole difference from Blink.
    let Some(immediately) = simulation.world().position(player) else {
        panic!("the player has a position");
    };
    assert!(
        (immediately - from).length() < 0.001,
        "the charge teleported"
    );

    let mut travelled = 0.0;
    let mut previous = from;
    for _ in 0..180 {
        simulation.step_once();
        let Some(now) = simulation.world().position(player) else {
            panic!("the player has a position");
        };
        // Every step is a step: no single tick of a charge covers a Blink's distance.
        let step = (now - previous).length();
        assert!(
            step < FireAbility::Blink.definition().range,
            "{step} m in one tick"
        );
        travelled += step;
        previous = now;
        if simulation.warrior().charge.is_none() {
            break;
        }
    }
    assert!(travelled > 5.0, "the charge covered {travelled} m");
    let Some(reached) = simulation.world().position(player) else {
        panic!("the player has a position");
    };
    let Some(destination) = simulation.world().position(victim) else {
        panic!("the target has a position");
    };
    let gap = (reached - destination).length();
    assert!(gap < 4.0, "the charge stopped {gap} m short");
}

#[test]
fn the_druids_moonfire_hurts_and_then_keeps_hurting() {
    let mut simulation = arena_as(Hero::Druid);
    let victim = target(&simulation);
    let before = health(&simulation, victim);
    cast_through(&mut simulation, FireAbility::Moonfire);
    let after_bolt = health(&simulation, victim);
    assert!(after_bolt < before, "moonfire landed nothing");

    // And the burn it left keeps working with nothing else pressed.
    simulation.step_many(300);
    assert!(
        health(&simulation, victim) < after_bolt,
        "the burn ticked for nothing"
    );
}

#[test]
fn regrowth_heals_the_caster_when_nothing_friendly_is_selected() {
    let mut simulation = arena_as(Hero::Druid);
    let player = simulation.player().entity;
    let Some(full) = simulation.world().health(player).map(|health| health.max()) else {
        panic!("the player has health");
    };
    // Hurt first: a heal at full health restores nothing, which is correct and proves
    // nothing.
    simulation.world_mut().damage(player, full * 0.5);
    let wounded = health(&simulation, player);
    assert!(wounded < full);

    cast_through(&mut simulation, FireAbility::Regrowth);
    assert!(
        health(&simulation, player) > wounded,
        "the heal restored nothing"
    );
}

#[test]
fn no_hero_but_the_mage_generates_heat_or_applies_a_burn() {
    // The design rule, against a live fight rather than against the table: the Warrior and
    // the Druid leave the FireMage's resources exactly where they found them.
    for hero in [Hero::Warrior, Hero::Druid] {
        let mut simulation = arena_as(hero);
        close_to_target(&mut simulation, 3.0);
        let victim = target(&simulation);
        for ability in hero.abilities() {
            if simulation.cast(*ability).is_err() {
                let Some(at) = simulation.world().position(victim) else {
                    panic!("the target has a position");
                };
                let _ = simulation.cast_at(*ability, at);
            }
            simulation.step_many(120);
        }
        assert_eq!(simulation.mage().heat.value(), 0.0, "{hero:?} made Heat");
        assert_eq!(
            simulation.mage().ignite.stacks(victim),
            0,
            "{hero:?} applied Ignite"
        );
    }
}
