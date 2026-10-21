//! The Sanctum of Ash: its four new monsters, its pylons, its mid-dungeon guardian and its
//! boss, each asserted on its signature behaviour and on its counter (alone as well as in a
//! party), and the dungeon built from its level file.
//!
//! Deterministic and headless, like the other dungeons' suites: a fixed timestep, a fixed seed,
//! the real brains and the real damage path, no device and no window.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::combat::DamageType;
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::{
    EnemyAbility, EnemyEffect, EnemyKind, EnemyRider, EnemyTarget, MonsterRole, MonsterTier,
    TelegraphShape,
};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::hero::Hero;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::status::{DispelSchool, EffectModifier, STATUS_KIND_COUNT, StatusKind};
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

fn stacks_of(simulation: &Simulation, entity: EntityId, kind: StatusKind) -> u8 {
    simulation
        .world()
        .statuses(entity)
        .map_or(0, |statuses| statuses.stacks(kind))
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

/// Runs until `caster` is `elapsed` seconds into a cast of `ability`, then kicks it.
fn kick(simulation: &mut Simulation, caster: EntityId, ability: EnemyAbility, elapsed: f32) {
    let mut kicked = false;
    for _ in 0..(60 * 12) {
        simulation.step_once();
        if simulation
            .brain(caster)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == ability && cast.elapsed > elapsed)
        {
            assert!(simulation.is_interruptible(caster), "{ability:?}");
            assert!(simulation.interrupt(caster), "{ability:?}");
            kicked = true;
            break;
        }
    }
    assert!(kicked, "no {ability:?} to kick");
}

/// One hit by the first player, and the events that hit alone produced.
fn hit(simulation: &mut Simulation, target: EntityId, amount: f32) -> Vec<CombatEvent> {
    let player = simulation.player().entity;
    let before = simulation.events().len();
    simulation.apply_damage(
        player,
        target,
        amount,
        AbilityId::NONE,
        DamageType::Physical,
    );
    simulation.events()[before..].to_vec()
}

// ------------------------------------------------------------------------------------------
// The table
// ------------------------------------------------------------------------------------------

#[test]
fn the_new_roster_is_in_the_table_with_the_roles_the_design_asks_for() {
    let archetype = |id: &str| {
        EnemyKind::from_id(id)
            .unwrap_or_else(|| panic!("{id} is not a monster"))
            .archetype()
    };
    assert_eq!(archetype("mirror_penitent").role, MonsterRole::Melee);
    assert_eq!(archetype("ash_cantor").role, MonsterRole::Support);
    assert_eq!(archetype("ash_stalker").role, MonsterRole::Melee);
    assert_eq!(archetype("pyre_immolator").role, MonsterRole::Melee);
    assert_eq!(archetype("ember_pylon").role, MonsterRole::Ranged);
    assert_eq!(archetype("censer_warden").role, MonsterRole::Elite);
    assert_eq!(archetype("ashen_pontiff").role, MonsterRole::Boss);
    assert!(EnemyKind::AshenPontiff.is_boss());
    assert_eq!(EnemyKind::AshenPontiff.archetype().tier, MonsterTier::Boss);
    // The guardian is an elite with a table of its own, so the level has one boss pack.
    assert!(!EnemyKind::CenserWarden.is_boss());
    for kind in [
        EnemyKind::MirrorPenitent,
        EnemyKind::AshCantor,
        EnemyKind::AshStalker,
        EnemyKind::PyreImmolator,
        EnemyKind::EmberPylon,
    ] {
        assert!(!kind.is_boss(), "{kind:?}");
    }
    // Every new body wears a model somebody else already has.
    for kind in [
        EnemyKind::MirrorPenitent,
        EnemyKind::AshCantor,
        EnemyKind::AshStalker,
        EnemyKind::PyreImmolator,
        EnemyKind::EmberPylon,
        EnemyKind::CenserWarden,
        EnemyKind::AshenPontiff,
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
    // Only the immolator leaves anything behind, and only the pylon holds a boss.
    for kind in EnemyKind::ALL {
        let a = kind.archetype();
        assert_eq!(
            a.on_death.is_some(),
            kind == EnemyKind::PyreImmolator,
            "{kind:?}"
        );
        assert_eq!(a.shields_boss, kind == EnemyKind::EmberPylon, "{kind:?}");
    }
    // The hardest of the five: a boss above the Overseer, and trash heavier than the Foundry's.
    assert!(
        EnemyKind::AshenPontiff.archetype().max_health
            > EnemyKind::FoundryOverseer.archetype().max_health
    );
    assert!(
        EnemyKind::MirrorPenitent.archetype().max_health
            > EnemyKind::SlagIronclad.archetype().max_health
    );
    // The mini-boss is a boss of its own size, not the dungeon's.
    assert!(
        EnemyKind::CenserWarden.archetype().max_health
            < EnemyKind::AshenPontiff.archetype().max_health / 1.5
    );
}

#[test]
fn every_new_ability_tells_the_player_what_it_is_and_what_to_do() {
    for ability in [
        EnemyAbility::MirrorWard,
        EnemyAbility::PenitentBlade,
        EnemyAbility::LitanyOfAsh,
        EnemyAbility::AshenMend,
        EnemyAbility::AmbushLeap,
        EnemyAbility::StalkerSlash,
        EnemyAbility::PyreSlam,
        EnemyAbility::EmberCollapse,
        EnemyAbility::AshTithe,
        EnemyAbility::CenserSwing,
        EnemyAbility::SmokeVeil,
        EnemyAbility::CenserBlaze,
        EnemyAbility::BrandingStrike,
        EnemyAbility::PyreSweep,
        EnemyAbility::RiteOfCinders,
        EnemyAbility::LastRite,
    ] {
        let definition = ability.definition();
        assert!(!definition.description.is_empty(), "{}", definition.name);
        assert!(!definition.counter.is_empty(), "{}", definition.name);
    }
}

#[test]
fn the_new_statuses_are_appended_last_and_the_curses_can_be_dispelled() {
    // Statuses travel the wire by index: the first thirty-six never move.
    assert_eq!(STATUS_KIND_COUNT, 40);
    assert_eq!(StatusKind::Shielded.index(), 39);
    assert!(!StatusKind::Shielded.is_purgeable() && !StatusKind::Shielded.is_dispellable());
    assert_eq!(StatusKind::Stoked.index(), 35);
    assert_eq!(StatusKind::Mirrored.index(), 36);
    assert_eq!(StatusKind::Ashen.index(), 37);
    assert_eq!(StatusKind::Branded.index(), 38);
    // The ward is a buff on a monster; the other two are what the party carries.
    assert!(!StatusKind::Mirrored.is_harmful());
    assert!(!StatusKind::Mirrored.is_dispellable());
    assert!(
        StatusKind::Mirrored
            .definition()
            .modifiers
            .iter()
            .any(|m| matches!(m, EffectModifier::Reflect { fraction } if *fraction > 0.0))
    );
    for (kind, school) in [
        (StatusKind::Ashen, DispelSchool::Curse),
        (StatusKind::Branded, DispelSchool::Magic),
    ] {
        assert!(kind.is_harmful() && kind.is_dispellable(), "{kind:?}");
        assert_eq!(kind.dispel_school(), Some(school));
        assert!(kind.max_stacks() >= 5, "{kind:?} stacks");
        assert!(!kind.definition().description.is_empty());
    }
}

// ------------------------------------------------------------------------------------------
// Mirror Penitent: the ward
// ------------------------------------------------------------------------------------------

/// A penitent in front of the player, a warrior beside it.
fn penitent_pack() -> (Simulation, EntityId, EntityId) {
    let spawn = player_spawn();
    let (simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::MirrorPenitent, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(4.0, 0.0, -10.0),
        ),
    ]);
    (simulation, members[0], members[1])
}

/// Until the penitent has finished a ward.
fn ward_finished(simulation: &mut Simulation, penitent: EntityId) {
    let mut events = Vec::new();
    for _ in 0..(60 * 8) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if completed(&events, penitent, EnemyAbility::MirrorWard) >= 1 {
            return;
        }
    }
    panic!("the penitent never finished a ward");
}

/// What the player lost to the mirror in `events`.
fn reflected(events: &[CombatEvent], player: EntityId) -> f32 {
    damage_from(events, EnemyAbility::MirrorWard, player)
        .iter()
        .map(|(_, amount)| amount)
        .sum()
}

#[test]
fn a_finished_ward_mirrors_the_pack_and_sends_half_of_what_lands_back_at_the_attacker() {
    let (mut simulation, penitent, warrior) = penitent_pack();
    let player = simulation.player().entity;
    // Unwarded, a hit costs the attacker nothing.
    let events = hit(&mut simulation, warrior, 500.0);
    assert!(reflected(&events, player) <= 0.0);
    ward_finished(&mut simulation, penitent);
    for who in [warrior, penitent] {
        assert!(
            has_status(&simulation, who, StatusKind::Mirrored),
            "{who:?}"
        );
    }
    let before = simulation
        .world()
        .health(warrior)
        .map_or(0.0, |h| h.current());
    // Several hits, so one dodged by the player's own defences cannot hide the mirror.
    let mut back = 0.0;
    let mut dealt = 0.0;
    for _ in 0..6 {
        let events = hit(&mut simulation, warrior, 100.0);
        back += reflected(&events, player);
        dealt += events
            .iter()
            .filter_map(|event| match event {
                CombatEvent::Damage(damage)
                    if damage.target == warrior && damage.source == player =>
                {
                    Some(damage.amount)
                }
                _ => None,
            })
            .sum::<f32>();
    }
    assert!(dealt > 400.0, "the hits did not land: {dealt}");
    assert!(back > 0.0, "nothing came back");
    assert!(
        back <= dealt * 0.5 + 1e-3,
        "{back} came back from {dealt} dealt"
    );
    // Everything the penitent's pack takes still lands: the mirror takes nothing off it.
    assert!(before - health(&simulation, warrior) >= dealt - 1.0);
    // It reaches fire as well as steel: a school-blind ward.
    assert!(
        EnemyAbility::MirrorWard.definition().damage_type != DamageType::Physical,
        "the reflection is a spell"
    );
}

#[test]
fn a_reflection_is_capped_and_never_reflected_in_turn() {
    let (mut simulation, penitent, warrior) = penitent_pack();
    let player = simulation.player().entity;
    ward_finished(&mut simulation, penitent);
    // A tiny player takes a huge hit's worth of mirror: the cap stops it short of death.
    simulation.world_mut().set_max_health(player, 1_000.0);
    simulation.world_mut().restore(player);
    let mut worst = 0.0_f32;
    for _ in 0..8 {
        simulation.world_mut().restore(player);
        let events = hit(&mut simulation, warrior, 1.0e6);
        worst = worst.max(reflected(&events, player));
        assert!(
            simulation.world().is_alive(player),
            "a mirrored hit killed the attacker"
        );
        simulation.world_mut().restore(warrior);
    }
    assert!(worst > 0.0, "the mirror never fired");
    assert!(worst <= 1_000.0 * 0.12 + 1e-3, "{worst} through the cap");

    // Two mirrors facing each other would bounce one hit for ever: the reflection itself is not
    // reflected.
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
    assert!(simulation.apply_effect(player, StatusKind::Mirrored, 30.0, 1, EntityId::INVALID));
    simulation.world_mut().restore(warrior);
    let events = hit(&mut simulation, warrior, 100.0);
    let on_warrior = events
        .iter()
        .filter(|event| matches!(event, CombatEvent::Damage(d) if d.target == warrior))
        .count();
    assert_eq!(on_warrior, 1, "the reflection bounced");
}

#[test]
fn the_ward_wears_off_so_waiting_it_out_stops_the_mirror() {
    let (mut simulation, penitent, warrior) = penitent_pack();
    let player = simulation.player().entity;
    ward_finished(&mut simulation, penitent);
    assert!(has_status(&simulation, warrior, StatusKind::Mirrored));
    // Seven seconds later the ward is gone. The penitent may have begun another; kill it first
    // so this measures the lapse and not the next ward.
    simulation.slay(penitent);
    run(&mut simulation, 60 * 7);
    assert!(!has_status(&simulation, warrior, StatusKind::Mirrored));
    for _ in 0..4 {
        let events = hit(&mut simulation, warrior, 100.0);
        assert!(reflected(&events, player) <= 0.0);
    }
}

#[test]
fn a_kicked_ward_mirrors_nobody_and_it_reaches_only_the_allies_around_it() {
    let (mut simulation, penitent, warrior) = penitent_pack();
    kick(&mut simulation, penitent, EnemyAbility::MirrorWard, 0.6);
    run(&mut simulation, 30);
    assert!(!has_status(&simulation, warrior, StatusKind::Mirrored));
    assert!(!has_status(&simulation, penitent, StatusKind::Mirrored));

    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::MirrorPenitent, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(4.0, 0.0, -10.0),
        ),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(40.0, 0.0, -10.0),
        ),
    ]);
    ward_finished(&mut simulation, members[0]);
    assert!(has_status(&simulation, members[1], StatusKind::Mirrored));
    assert!(!has_status(&simulation, members[2], StatusKind::Mirrored));
}

#[test]
fn a_party_that_hits_the_unwarded_member_is_never_reflected_on() {
    // The solo answer, and the party's: switch targets. The warrior is outside the ward's reach.
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::MirrorPenitent, spawn + Vec3::new(0.0, 0.0, -8.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(40.0, 0.0, -10.0),
        ),
    ]);
    let player = simulation.player().entity;
    ward_finished(&mut simulation, members[0]);
    for _ in 0..4 {
        let events = hit(&mut simulation, members[1], 200.0);
        assert!(reflected(&events, player) <= 0.0);
    }
}

#[test]
fn the_blade_is_a_cone_drawn_for_the_whole_cast() {
    let (mut simulation, _penitent) = duel(EnemyKind::MirrorPenitent, 3.0);
    let (_, _, shape, delay) =
        first_telegraph(&mut simulation, 60 * 14, EnemyAbility::PenitentBlade);
    assert!(matches!(shape, TelegraphShape::Cone { range, .. } if range >= 5.5));
    assert!((delay - EnemyAbility::PenitentBlade.definition().cast_time).abs() < 0.05);
}

// ------------------------------------------------------------------------------------------
// Ash Cantor: the curse
// ------------------------------------------------------------------------------------------

fn cantor_fight() -> (Simulation, EntityId, Vec<EntityId>) {
    let (mut simulation, cantor) = duel(EnemyKind::AshCantor, 10.0);
    let mut players = vec![simulation.player().entity];
    players.extend(add_players(
        &mut simulation,
        &[Vec3::new(4.0, 0.0, 0.0), Vec3::new(-4.0, 0.0, 0.0)],
    ));
    (simulation, cantor, players)
}

#[test]
fn a_litany_curses_two_players_and_the_curse_stacks_slows_and_exposes() {
    let (mut simulation, cantor, players) = cantor_fight();
    let mut events = Vec::new();
    let mut cursed: Vec<EntityId> = Vec::new();
    for _ in 0..(60 * 30) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        cursed = players
            .iter()
            .copied()
            .filter(|player| has_status(&simulation, *player, StatusKind::Ashen))
            .collect();
        if completed(&events, cantor, EnemyAbility::LitanyOfAsh) >= 1 {
            break;
        }
    }
    assert!(
        completed(&events, cantor, EnemyAbility::LitanyOfAsh) >= 1,
        "no litany finished"
    );
    cursed = players
        .iter()
        .copied()
        .filter(|player| has_status(&simulation, *player, StatusKind::Ashen))
        .chain(cursed)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(cursed.len(), 2, "two players, one stack each");
    // Another few litanies climb the stacks towards five and no further.
    let mut peak = 0;
    for _ in 0..(60 * 120) {
        simulation.step_once();
        for player in &players {
            peak = peak.max(stacks_of(&simulation, *player, StatusKind::Ashen));
        }
    }
    assert!(peak >= 2, "the curse never stacked");
    assert!(peak <= 5, "the curse stacked to {peak}");
    // Six per cent more damage taken and three per cent slower, a stack at a time.
    let victim = cursed[0];
    let stacks = f32::from(stacks_of(&simulation, victim, StatusKind::Ashen).max(1));
    let statuses = simulation.world().statuses(victim).expect("statuses");
    let taken = statuses.damage_taken_multiplier(DamageType::Physical);
    assert!((taken - (1.0 + 0.06 * stacks)).abs() < 0.005, "{taken}");
    assert!(statuses.move_speed_multiplier() < 1.0);
}

#[test]
fn a_cleanse_takes_the_ashen_curse_off_every_stack_at_once() {
    let (mut simulation, _cantor, players) = cantor_fight();
    let player = players[0];
    assert!(simulation.set_hero(Hero::Druid));
    assert!(simulation.apply_effect(player, StatusKind::Ashen, 30.0, 4, EntityId::INVALID));
    assert_eq!(stacks_of(&simulation, player, StatusKind::Ashen), 4);
    simulation.player_mut().target = Some(player);
    simulation.step_once();
    let _ = simulation.cast(engine_game::firemage::FireAbility::NaturesCleanse);
    assert!(
        !has_status(&simulation, player, StatusKind::Ashen),
        "the curse stayed"
    );
}

#[test]
fn a_kicked_litany_curses_nobody_and_a_kicked_mend_heals_nobody() {
    let (mut simulation, cantor, players) = cantor_fight();
    kick(&mut simulation, cantor, EnemyAbility::LitanyOfAsh, 0.8);
    run(&mut simulation, 30);
    for player in &players {
        assert!(!has_status(&simulation, *player, StatusKind::Ashen));
    }

    // The mend: a wounded packmate, and the cantor's heal on it.
    let spawn = player_spawn();
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::AshCantor, spawn + Vec3::new(0.0, 0.0, -9.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(5.0, 0.0, -11.0),
        ),
    ]);
    let (cantor, mate) = (members[0], members[1]);
    wound_to(&mut simulation, mate, 0.4);
    let low = health(&simulation, mate);
    // Kicked while it is the mend that is being cast.
    kick(&mut simulation, cantor, EnemyAbility::AshenMend, 0.8);
    run(&mut simulation, 5);
    assert!(
        health(&simulation, mate) <= low + 1.0,
        "a kicked mend healed"
    );
    // Allowed to finish, it restores the amount on its row.
    let (mut simulation, members) = arena_with(vec![
        EnemySpawn::new(EnemyKind::AshCantor, spawn + Vec3::new(0.0, 0.0, -9.0)),
        EnemySpawn::new(
            EnemyKind::SkeletonWarrior,
            spawn + Vec3::new(5.0, 0.0, -11.0),
        ),
    ]);
    wound_to(&mut simulation, members[1], 0.4);
    let low = health(&simulation, members[1]);
    let events = run(&mut simulation, 60 * 8);
    assert!(completed(&events, members[0], EnemyAbility::AshenMend) >= 1);
    assert!(health(&simulation, members[1]) > low + 100.0);
}

// ------------------------------------------------------------------------------------------
// Ash Stalker: the hunt
// ------------------------------------------------------------------------------------------

/// A stalker 14 m ahead of three players: the tank, one 12 m to the left and one 12 m to the right.
fn stalker_fight() -> (Simulation, EntityId, Vec<EntityId>) {
    let (mut simulation, stalker) = duel(EnemyKind::AshStalker, 14.0);
    let mut players = vec![simulation.player().entity];
    players.extend(add_players(
        &mut simulation,
        &[Vec3::new(-12.0, 0.0, 0.0), Vec3::new(12.0, 0.0, 0.0)],
    ));
    // The tank holds its attention.
    let tank = players[0];
    simulation.apply_damage(tank, stalker, 5.0, AbilityId::NONE, DamageType::Physical);
    (simulation, stalker, players)
}

fn leaps(events: &[CombatEvent], stalker: EntityId, ability: EnemyAbility) -> Vec<EntityId> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage)
                if damage.source == stalker && damage.ability.enemy() == Some(ability) =>
            {
                Some(damage.target)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_ambush_leap_goes_for_the_player_with_the_least_health_left() {
    let (mut simulation, stalker, players) = stalker_fight();
    wound_to(&mut simulation, players[1], 0.30);
    let events = run(&mut simulation, 60 * 5);
    let first = leaps(&events, stalker, EnemyAbility::AmbushLeap);
    assert!(!first.is_empty(), "the stalker never leapt");
    assert_eq!(
        first[0], players[1],
        "it leapt at somebody other than the weakest"
    );
    // The weakest changes: the next leap follows the health bars, not the first victim.
    simulation.world_mut().restore(players[1]);
    wound_to(&mut simulation, players[2], 0.10);
    let events = run(&mut simulation, 60 * 16);
    let next = leaps(&events, stalker, EnemyAbility::AmbushLeap);
    assert!(!next.is_empty(), "no second leap");
    assert_eq!(next[0], players[2]);
}

#[test]
fn a_lone_player_is_the_weakest_and_the_leap_still_lands_with_its_slow() {
    let (mut simulation, stalker) = duel(EnemyKind::AshStalker, 10.0);
    let player = simulation.player().entity;
    let events = run(&mut simulation, 60 * 4);
    let hits = leaps(&events, stalker, EnemyAbility::AmbushLeap);
    assert_eq!(hits.first(), Some(&player));
    // Crippled by the landing: the counter is health and the circle, not a hiding place.
    assert!(
        events.iter().any(|event| matches!(
            event,
            CombatEvent::StatusApplied { target, status, .. }
                if *target == player && *status == StatusKind::Crippled
        )),
        "the landing did not slow"
    );
    // The table says so too.
    let leap = EnemyAbility::AmbushLeap.definition();
    assert_eq!(leap.select, EnemyTarget::Weakest);
    assert!(leap.effect.is_charge() && !leap.is_cast());
}

#[test]
fn the_stalkers_slash_leaves_a_stacking_bleed() {
    assert!(matches!(
        EnemyAbility::StalkerSlash.definition().rider,
        EnemyRider::Bleed { max_stacks, .. } if max_stacks >= 4
    ));
}

// ------------------------------------------------------------------------------------------
// Pyre Immolator: the fire it leaves
// ------------------------------------------------------------------------------------------

fn immolator_fight(distance: f32) -> (Simulation, EntityId) {
    duel(EnemyKind::PyreImmolator, distance)
}

/// The radii of every fire `EmberCollapse` lit, in the order they were drawn.
fn fire_radii(events: &[CombatEvent]) -> Vec<f32> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::AreaEffectStarted {
                ability,
                shape: engine_game::firemage::zone::ZoneShape::Circle { radius },
                ..
            } if ability.enemy() == Some(EnemyAbility::EmberCollapse) => Some(*radius),
            _ => None,
        })
        .collect()
}

#[test]
fn killing_an_immolator_leaves_a_blast_and_a_fire_that_spreads() {
    let (mut simulation, immolator) = immolator_fight(14.0);
    let player = simulation.player().entity;
    run(&mut simulation, 30);
    // The party kills it where it stands, a long way off.
    let mut events = hit(&mut simulation, immolator, 1.0e9);
    assert!(!simulation.world().is_alive(immolator));
    events.extend(run(&mut simulation, 60 * 16));
    let radii = fire_radii(&events);
    assert!(radii.len() >= 3, "the fire never spread: {radii:?}");
    assert!(radii.windows(2).all(|pair| pair[1] >= pair[0]));
    let (small, big) = (radii[0], *radii.last().expect("a radius"));
    assert!(small <= 3.6 && big >= 7.0, "{small} to {big}");
    // It goes out.
    assert!(
        events
            .iter()
            .any(|event| matches!(event, CombatEvent::AreaEffectEnded { .. }))
    );
    // And the player, who stayed back, was not touched by any of it.
    assert!(damage_from(&events, EnemyAbility::EmberCollapse, player).is_empty());
}

#[test]
fn the_fire_burns_whoever_stands_in_it_and_the_blast_hits_whoever_killed_it_close() {
    let (mut simulation, immolator) = immolator_fight(14.0);
    let player = simulation.player().entity;
    let at = simulation.world().position(immolator).expect("a body");
    // The party kills it from point-blank range: the blast catches them.
    put(&mut simulation, player, at + Vec3::new(1.5, 0.0, 0.0));
    let events = hit(&mut simulation, immolator, 1.0e9);
    let blast = damage_from(&events, EnemyAbility::EmberCollapse, player);
    assert!(!blast.is_empty(), "the blast missed somebody next to it");
    // Staying in the fire burns on, tick after tick.
    let events = run(&mut simulation, 60 * 4);
    assert!(
        damage_from(&events, EnemyAbility::EmberCollapse, player).len() >= 3,
        "standing in the fire did not burn"
    );
    // Walking out of it ends the burning: eight metres is beyond its widest reach.
    put(&mut simulation, player, at + Vec3::new(14.0, 0.0, 0.0));
    let events = run(&mut simulation, 60 * 8);
    assert!(damage_from(&events, EnemyAbility::EmberCollapse, player).is_empty());
}

#[test]
fn a_development_kill_lights_no_fire() {
    let (mut simulation, immolator) = immolator_fight(14.0);
    run(&mut simulation, 30);
    simulation.slay(immolator);
    let mut events = simulation.events().to_vec();
    events.extend(run(&mut simulation, 60));
    assert!(fire_radii(&events).is_empty());
}

#[test]
fn the_death_ability_scales_with_the_key_like_every_other() {
    // Doubling the monster's damage scale doubles the blast it leaves.
    let blast = |scale: f32| {
        let spawn = player_spawn() + Vec3::new(0.0, 0.0, -14.0);
        let (mut simulation, members) = arena_with(vec![
            EnemySpawn::new(EnemyKind::PyreImmolator, spawn).detailed(
                engine_game::dungeon::SpawnDetail {
                    level: 1,
                    health_scale: 1.0,
                    damage_scale: scale,
                    aggro_radius: None,
                    completion: None,
                    size: 1.0,
                },
            ),
        ]);
        let player = simulation.player().entity;
        let at = simulation.world().position(members[0]).expect("a body");
        put(&mut simulation, player, at + Vec3::new(1.0, 0.0, 0.0));
        let events = hit(&mut simulation, members[0], 1.0e9);
        damage_from(&events, EnemyAbility::EmberCollapse, player)
            .iter()
            .map(|(_, amount)| amount)
            .sum::<f32>()
    };
    let (one, two) = (blast(1.0), blast(2.0));
    assert!(one > 0.0);
    assert!((two / one - 2.0).abs() < 0.02, "{one} then {two}");
}

// ------------------------------------------------------------------------------------------
// The Ashen Pontiff
// ------------------------------------------------------------------------------------------

/// The Pontiff alone, 12 m in front of the player, with two more players beside the first.
fn pontiff_fight() -> (Simulation, EntityId, Vec<EntityId>) {
    let (mut simulation, pontiff) = duel(EnemyKind::AshenPontiff, 12.0);
    let mut players = vec![simulation.player().entity];
    players.extend(add_players(
        &mut simulation,
        &[Vec3::new(3.0, 0.0, 0.0), Vec3::new(-3.0, 0.0, 0.0)],
    ));
    (simulation, pontiff, players)
}

#[test]
fn the_pontiff_has_four_phases_with_each_mechanic_where_the_design_puts_it() {
    let archetype = EnemyKind::AshenPontiff.archetype();
    assert_eq!(archetype.phase_count(), 4);
    let thresholds: Vec<f32> = archetype.phases.iter().map(|p| p.at_or_below).collect();
    assert_eq!(thresholds, vec![0.75, 0.50, 0.25]);
    let has = |phase: usize, ability: EnemyAbility| {
        archetype.phase_abilities(phase).any(|a| a == ability)
    };
    // The brand from the start, the rite from seventy-five, the last rite at the end.
    assert!(has(0, EnemyAbility::BrandingStrike) && !has(0, EnemyAbility::RiteOfCinders));
    for phase in 1..4 {
        assert!(has(phase, EnemyAbility::RiteOfCinders), "phase {phase}");
        assert!(has(phase, EnemyAbility::BrandingStrike), "phase {phase}");
    }
    for phase in 0..3 {
        assert!(!has(phase, EnemyAbility::LastRite), "phase {phase}");
    }
    assert!(has(3, EnemyAbility::LastRite));
    assert!(EnemyAbility::LastRite.definition().once_per_phase);
    // The adds: three pylons at fifty, two immolators at twenty-five, nobody else.
    let summons: Vec<_> = (0..4).map(|p| archetype.phase_summon(p)).collect();
    assert!(summons[0].is_none() && summons[1].is_none());
    let pylons = summons[2].expect("pylons at fifty");
    assert_eq!((pylons.kind, pylons.count), (EnemyKind::EmberPylon, 3));
    let immolators = summons[3].expect("immolators at twenty-five");
    assert_eq!(
        (immolators.kind, immolators.count),
        (EnemyKind::PyreImmolator, 2)
    );
    // He never outruns a player, however desperate.
    for phase in 0..4 {
        assert!(archetype.phase_speed(phase) < 7.0, "phase {phase}");
    }
    assert!(archetype.max_health > 19_000.0 && archetype.max_health <= 25_000.0);
}

// ----------------------------------------------------------------------- the brand

/// A cone's apex and yaw, from the first Branding Strike the Pontiff draws.
fn first_brand(simulation: &mut Simulation) -> (Vec3, f32, f32) {
    let (_, anchor, shape, delay) =
        first_telegraph(simulation, 60 * 20, EnemyAbility::BrandingStrike);
    let TelegraphShape::Cone { yaw, range, .. } = shape else {
        panic!("the brand is a cone");
    };
    assert!(range >= 5.0);
    assert!(
        (delay - EnemyAbility::BrandingStrike.definition().cast_time).abs() < 0.05,
        "drawn for the whole cast: {delay}"
    );
    (anchor, yaw, delay)
}

#[test]
fn a_branding_strike_brands_whoever_stands_in_its_cone_and_the_brand_stacks() {
    let (mut simulation, pontiff, players) = pontiff_fight();
    // The tank stands where the cone points; everyone else is well away.
    for (index, player) in players.iter().enumerate().skip(1) {
        put(
            &mut simulation,
            *player,
            player_spawn() + Vec3::new(index as f32 * 18.0, 0.0, 20.0),
        );
    }
    let tank = players[0];
    simulation.apply_damage(tank, pontiff, 5.0, AbilityId::NONE, DamageType::Physical);
    let mut peak = 0;
    for _ in 0..(60 * 60) {
        simulation.step_once();
        peak = peak.max(stacks_of(&simulation, tank, StatusKind::Branded));
        if peak >= 3 {
            break;
        }
    }
    assert!(peak >= 3, "the brand only reached {peak}");
    // Twelve per cent more damage taken a stack.
    let stacks = f32::from(stacks_of(&simulation, tank, StatusKind::Branded));
    let taken = simulation
        .world()
        .statuses(tank)
        .expect("statuses")
        .damage_taken_multiplier(DamageType::Physical);
    assert!((taken - (1.0 + 0.12 * stacks)).abs() < 0.01, "{taken}");
    // A second tank starts from nothing: the swap.
    assert_eq!(stacks_of(&simulation, players[1], StatusKind::Branded), 0);
}

#[test]
fn stepping_out_of_the_cone_beats_the_brand_alone() {
    let (mut simulation, pontiff, players) = pontiff_fight();
    let player = players[0];
    simulation.apply_damage(player, pontiff, 5.0, AbilityId::NONE, DamageType::Physical);
    for other in &players[1..] {
        put(
            &mut simulation,
            *other,
            player_spawn() + Vec3::new(30.0, 0.0, 30.0),
        );
    }
    let (anchor, yaw, delay) = first_brand(&mut simulation);
    // One step sideways, well out of a cone of that angle.
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let side = Vec3::new(-forward.z, 0.0, forward.x);
    put(&mut simulation, player, anchor + forward * 3.0 + side * 9.0);
    let events = run(&mut simulation, (delay * 60.0) as u32 + 20);
    assert!(damage_from(&events, EnemyAbility::BrandingStrike, player).is_empty());
    assert_eq!(stacks_of(&simulation, player, StatusKind::Branded), 0);
}

#[test]
fn a_kicked_strike_brands_nobody_and_a_cleanse_takes_the_brand_off() {
    let (mut simulation, pontiff, players) = pontiff_fight();
    let tank = players[0];
    simulation.apply_damage(tank, pontiff, 5.0, AbilityId::NONE, DamageType::Physical);
    kick(&mut simulation, pontiff, EnemyAbility::BrandingStrike, 0.6);
    run(&mut simulation, 20);
    assert_eq!(stacks_of(&simulation, tank, StatusKind::Branded), 0);

    // And a stack that did land comes off with a dispel of its school.
    assert!(simulation.set_hero(Hero::Druid));
    assert!(simulation.apply_effect(tank, StatusKind::Branded, 30.0, 5, EntityId::INVALID));
    simulation.player_mut().target = Some(tank);
    simulation.step_once();
    let _ = simulation.cast(engine_game::firemage::FireAbility::NaturesCleanse);
    assert!(!has_status(&simulation, tank, StatusKind::Branded));
}

// ----------------------------------------------------------------------- the rite

/// The Pontiff at seventy-four, until he begins the rite: the telegraph and who is where.
fn rite_fight() -> (Simulation, EntityId, Vec<EntityId>, u32, f32) {
    let (mut simulation, pontiff, players) = pontiff_fight();
    wound_to(&mut simulation, pontiff, 0.74);
    let (id, _, shape, delay) =
        first_telegraph(&mut simulation, 60 * 30, EnemyAbility::RiteOfCinders);
    let TelegraphShape::Circle { radius } = shape else {
        panic!("the rite is a circle");
    };
    // Bigger than any room: there is nowhere to run to.
    assert!(radius >= 60.0, "{radius}");
    assert!(delay >= 4.0, "a long warning: {delay}");
    (simulation, pontiff, players, id, delay)
}

#[test]
fn the_rite_fills_an_open_floor_and_hits_everyone_in_it() {
    let (mut simulation, _pontiff, players, _, delay) = rite_fight();
    let events = run(&mut simulation, (delay * 60.0) as u32 + 30);
    for player in &players {
        assert!(
            !damage_from(&events, EnemyAbility::RiteOfCinders, *player).is_empty(),
            "the rite missed somebody standing in the open"
        );
    }
    assert!(!EnemyAbility::RiteOfCinders.definition().interruptible);
}

#[test]
fn a_wall_of_stone_between_you_and_the_pontiff_stops_the_rite_for_one_player_or_five() {
    let (mut simulation, pontiff, players, _, delay) = rite_fight();
    // A great pillar, three metres across, between the Pontiff and the first two players.
    let from = simulation.world().position(pontiff).expect("a boss");
    let toward = Vec3::new(1.0, 0.0, 0.0);
    let column = from + toward * 12.0;
    let collision = simulation.collision_world_mut();
    collision.add_box(
        column + Vec3::new(-1.5, 0.0, -1.5),
        column + Vec3::new(1.5, 6.0, 1.5),
        "TEST PILLAR",
    );
    collision.rebuild_broad_phase();
    let hidden = column + toward * 3.5;
    put(&mut simulation, players[0], hidden);
    put(
        &mut simulation,
        players[1],
        hidden + Vec3::new(0.0, 0.0, 0.5),
    );
    // The third stands where he can see her.
    put(
        &mut simulation,
        players[2],
        from + Vec3::new(0.0, 0.0, 10.0),
    );
    let events = run(&mut simulation, (delay * 60.0) as u32 + 30);
    for player in &players[..2] {
        assert!(
            damage_from(&events, EnemyAbility::RiteOfCinders, *player).is_empty(),
            "stone did not stop the rite"
        );
    }
    assert!(
        !damage_from(&events, EnemyAbility::RiteOfCinders, players[2]).is_empty(),
        "the one in the open should have been hit"
    );
}

// ----------------------------------------------------------------------- the pylons

/// The Pontiff driven to forty-nine per cent: three pylons come, and the boss is held.
fn pylon_fight() -> (Simulation, EntityId, Vec<EntityId>, Vec<EntityId>) {
    let (mut simulation, pontiff, players) = pontiff_fight();
    wound_to(&mut simulation, pontiff, 0.49);
    let events = run(&mut simulation, 60 * 2);
    let pylons: Vec<EntityId> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::AddSummoned { boss, add, .. } if *boss == pontiff => Some(*add),
            _ => None,
        })
        .collect();
    (simulation, pontiff, players, pylons)
}

#[test]
fn at_fifty_per_cent_three_pylons_rise_and_at_twenty_five_two_immolators() {
    let (mut simulation, pontiff, _players, pylons) = pylon_fight();
    assert_eq!(pylons.len(), 3);
    for pylon in &pylons {
        assert_eq!(
            simulation.brain(*pylon).expect("an enemy").kind(),
            EnemyKind::EmberPylon
        );
    }
    // Once: not again however long the phase lasts.
    let events = run(&mut simulation, 60 * 10);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::AddSummoned { .. }))
    );
    // Held, the boss cannot be taken to the next threshold: drop the pylons and then the rest.
    for pylon in &pylons {
        simulation.slay(*pylon);
    }
    wound_to(&mut simulation, pontiff, 0.24);
    let events = run(&mut simulation, 60 * 3);
    let immolators = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::AddSummoned { boss, add, .. } if *boss == pontiff => Some(*add),
            _ => None,
        })
        .filter(|add| {
            simulation
                .brain(*add)
                .is_some_and(|brain| brain.kind() == EnemyKind::PyreImmolator)
        })
        .count();
    assert_eq!(immolators, 2);
}

#[test]
fn while_a_pylon_stands_the_boss_cannot_be_hurt_and_the_pylons_can() {
    let (mut simulation, pontiff, _players, pylons) = pylon_fight();
    let before = health(&simulation, pontiff);
    hit(&mut simulation, pontiff, 5_000.0);
    assert!(
        (health(&simulation, pontiff) - before).abs() < 1e-3,
        "the boss took damage through the pylons"
    );
    // Kill all but one: still held.
    for pylon in &pylons[..2] {
        assert!(health(&simulation, *pylon) > 0.0);
        hit(&mut simulation, *pylon, 1.0e7);
        assert!(!simulation.world().is_alive(*pylon));
    }
    hit(&mut simulation, pontiff, 5_000.0);
    assert!((health(&simulation, pontiff) - before).abs() < 1e-3);
    // The last one falls and the fight is on again.
    hit(&mut simulation, pylons[2], 1.0e7);
    hit(&mut simulation, pontiff, 5_000.0);
    assert!(
        health(&simulation, pontiff) < before - 1_000.0,
        "the boss stayed immune after the pylons fell"
    );
}

#[test]
fn a_pylon_pulses_the_sanctum_until_it_is_killed_or_kicked_and_stone_stops_the_pulse() {
    let (mut simulation, _pontiff, players, pylons) = pylon_fight();
    // Two of the three down; one left to watch.
    simulation.slay(pylons[1]);
    simulation.slay(pylons[2]);
    let pylon = pylons[0];
    // Drawn for the whole cast.
    let (_, _, shape, delay) = first_telegraph(&mut simulation, 60 * 12, EnemyAbility::AshTithe);
    assert!(matches!(shape, TelegraphShape::Circle { radius } if radius >= 60.0));
    assert!(
        (delay - EnemyAbility::AshTithe.definition().cast_time).abs() < 0.05,
        "{delay}"
    );
    let events = run(&mut simulation, (delay * 60.0) as u32 + 30);
    let hit_players = players
        .iter()
        .filter(|player| !damage_from(&events, EnemyAbility::AshTithe, **player).is_empty())
        .count();
    assert_eq!(hit_players, 3, "the pulse missed somebody in the open");
    // A kick on the next one lands nothing.
    kick(&mut simulation, pylon, EnemyAbility::AshTithe, 0.8);
    let events = run(&mut simulation, 60);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::Damage(d) if d.ability.enemy() == Some(EnemyAbility::AshTithe))),
        "a kicked pulse landed"
    );
    // A pillar between you and it: no damage for the one behind.
    let from = simulation.world().position(pylon).expect("a body");
    let column = from + Vec3::new(0.0, 0.0, 10.0);
    let collision = simulation.collision_world_mut();
    collision.add_box(
        column + Vec3::new(-1.5, 0.0, -1.5),
        column + Vec3::new(1.5, 6.0, 1.5),
        "TEST PILLAR",
    );
    collision.rebuild_broad_phase();
    put(
        &mut simulation,
        players[0],
        column + Vec3::new(0.0, 0.0, 3.5),
    );
    put(
        &mut simulation,
        players[1],
        from + Vec3::new(14.0, 0.0, 6.0),
    );
    let (_, _, _, delay) = first_telegraph(&mut simulation, 60 * 12, EnemyAbility::AshTithe);
    let events = run(&mut simulation, (delay * 60.0) as u32 + 30);
    assert!(damage_from(&events, EnemyAbility::AshTithe, players[0]).is_empty());
    assert!(!damage_from(&events, EnemyAbility::AshTithe, players[1]).is_empty());
}

// ----------------------------------------------------------------------- the last rite

#[test]
fn the_last_rite_is_once_permanent_and_makes_everything_faster() {
    let (mut simulation, pontiff, _players) = pontiff_fight();
    run(&mut simulation, 60);
    assert!(!simulation.brain(pontiff).expect("a brain").is_frenzied());
    wound_to(&mut simulation, pontiff, 0.24);
    run(&mut simulation, 60 * 3);
    let brain = simulation.brain(pontiff).expect("a brain");
    assert!(brain.is_frenzied(), "he did not begin his last rite");
    assert!(brain.frenzy_remaining() > 300.0);
    assert!(brain.haste() > 1.3);
    let events = run(&mut simulation, 60 * 60);
    assert_eq!(
        casts_of(&events, pontiff, EnemyAbility::LastRite),
        0,
        "the enrage is cast once"
    );
}

// ------------------------------------------------------------------------------------------
// The Censer Warden
// ------------------------------------------------------------------------------------------

fn warden_fight() -> (Simulation, EntityId, Vec<EntityId>) {
    let (mut simulation, warden) = duel(EnemyKind::CenserWarden, 10.0);
    let mut players = vec![simulation.player().entity];
    players.extend(add_players(
        &mut simulation,
        &[Vec3::new(4.0, 0.0, 0.0), Vec3::new(-4.0, 0.0, 0.0)],
    ));
    (simulation, warden, players)
}

#[test]
fn the_warden_throws_smoke_and_curses_and_flares_at_half_health() {
    let archetype = EnemyKind::CenserWarden.archetype();
    assert_eq!(archetype.phase_count(), 2);
    assert!(
        !archetype
            .phase_abilities(0)
            .any(|a| a == EnemyAbility::CenserBlaze)
    );
    assert!(
        archetype
            .phase_abilities(1)
            .any(|a| a == EnemyAbility::CenserBlaze)
    );
    let (mut simulation, warden, players) = warden_fight();
    let events = run(&mut simulation, 60 * 45);
    assert!(casts_of(&events, warden, EnemyAbility::SmokeVeil) >= 1);
    assert!(casts_of(&events, warden, EnemyAbility::LitanyOfAsh) >= 1);
    assert_eq!(casts_of(&events, warden, EnemyAbility::CenserBlaze), 0);
    assert!(
        players
            .iter()
            .any(|player| has_status(&simulation, *player, StatusKind::Ashen)),
        "the warden's litany left no curse"
    );
    wound_to(&mut simulation, warden, 0.49);
    let events = run(&mut simulation, 60 * 25);
    assert!(casts_of(&events, warden, EnemyAbility::CenserBlaze) >= 1);
    // The flare is a circle around the warden, drawn for the whole cast.
    let (_, _, shape, delay) = first_telegraph(&mut simulation, 60 * 25, EnemyAbility::CenserBlaze);
    assert!(matches!(shape, TelegraphShape::Circle { radius } if radius >= 8.0));
    assert!((delay - EnemyAbility::CenserBlaze.definition().cast_time).abs() < 0.05);
}

#[test]
fn smoke_is_dodged_by_leaving_the_circle_and_laid_by_nothing_when_kicked() {
    let (mut simulation, warden, players) = warden_fight();
    let (_, _, shape, delay) = first_telegraph(&mut simulation, 60 * 20, EnemyAbility::SmokeVeil);
    assert!(matches!(shape, TelegraphShape::Circle { .. }));
    assert!(delay >= 1.5);
    // Everybody walks away from wherever the circles are.
    for (index, player) in players.iter().enumerate() {
        put(
            &mut simulation,
            *player,
            player_spawn() + Vec3::new(30.0 + index as f32 * 3.0, 0.0, 14.0),
        );
    }
    let events = run(&mut simulation, 60 * 3);
    for player in &players {
        assert!(damage_from(&events, EnemyAbility::SmokeVeil, *player).is_empty());
    }
    let (mut simulation, warden2, _) = warden_fight();
    kick(&mut simulation, warden2, EnemyAbility::SmokeVeil, 0.6);
    let events = run(&mut simulation, 60);
    assert!(!events.iter().any(|event| matches!(
        event,
        CombatEvent::TelegraphStarted { ability, .. } if ability.enemy() == Some(EnemyAbility::SmokeVeil)
    )));
    let _ = warden;
}

// ------------------------------------------------------------------------------------------
// The dungeon, built from its level file
// ------------------------------------------------------------------------------------------

use engine_game::dungeon::{DungeonScaling, build_level_file, populate_from_scene};

fn scene_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenes/sanctum_of_ash.json")
}

fn sanctum() -> Simulation {
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

const SHRINES: [&str; 3] = [
    "SHRINE OF MIRRORS",
    "SHRINE OF EMBERS",
    "SHRINE OF THE HUNT",
];

const BEFORE_THE_GATE: [&str; 6] = [
    "ASHEN NAVE",
    "CINDER WATCH",
    "SHRINE OF MIRRORS",
    "SHRINE OF EMBERS",
    "SHRINE OF THE HUNT",
    "GALLERY OF CENSERS",
];

#[test]
fn the_level_loads_with_no_unknown_ids_and_the_boss_and_mini_boss_packs() {
    let scene = engine_environment::scene_file::load(scene_path()).expect("loads");
    let mut simulation = Simulation::new(engine_game::sim::SimConfig::default());
    let report = populate_from_scene(&mut simulation, &scene, DungeonScaling::default());
    assert!(report.unknown.is_empty(), "{:?}", report.unknown);
    assert_eq!(report.packs, 7, "six packs and the boss");
    assert_eq!(report.monsters, scene.monsters.len());
    let boss = report.boss_encounter.expect("a boss encounter");
    assert_eq!(simulation.encounters()[boss].name(), "THE ASHEN PONTIFF");
    assert_eq!(
        simulation
            .brain(simulation.encounters()[boss].members()[0])
            .expect("a boss")
            .kind(),
        EnemyKind::AshenPontiff
    );
    // The mini-boss pack is there, and it is a boss of its own.
    let gallery = simulation
        .encounters()
        .iter()
        .find(|encounter| encounter.name() == "GALLERY OF CENSERS")
        .expect("the mini-boss pack");
    assert_eq!(
        simulation
            .brain(gallery.members()[0])
            .expect("a warden")
            .kind(),
        EnemyKind::CenserWarden
    );
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
    assert_eq!(
        scene
            .monsters
            .iter()
            .filter(|m| !m.patrol.is_empty())
            .count(),
        1,
        "the cinder watch walks"
    );
    // Every new monster that is placed, is; the pylons are called, never placed.
    for id in [
        "mirror_penitent",
        "ash_cantor",
        "ash_stalker",
        "pyre_immolator",
        "censer_warden",
        "ashen_pontiff",
    ] {
        assert!(
            scene.monsters.iter().any(|m| m.monster == id),
            "{id} is not placed"
        );
    }
    assert!(!scene.monsters.iter().any(|m| m.monster == "ember_pylon"));
}

#[test]
fn the_boss_is_sealed_and_every_route_through_the_sanctum_is_walkable() {
    let mut simulation = sanctum();
    assert!(simulation.gate_is_closed());
    let pontiff = pack(&simulation, "THE ASHEN PONTIFF")[0];
    assert_eq!(
        simulation.brain(pontiff).expect("a boss").kind(),
        EnemyKind::AshenPontiff
    );
    simulation.unlock_dungeon();
    // The spine, each spoke off the rotunda, and the road north to the boss.
    let routes: Vec<Vec<(f32, f32)>> = vec![
        vec![
            (0.0, -8.0),
            (0.0, -28.0),
            (0.0, -44.0),
            (0.0, -60.0),
            (0.0, -72.0),
            (10.0, -84.0),
            (0.0, -96.0),
            (0.0, -108.0),
            (0.0, -124.0),
            (0.0, -148.0),
            (0.0, -166.0),
            (0.0, -180.0),
            (0.0, -196.0),
            (0.0, -214.0),
            (0.0, -236.0),
        ],
        // West door: the rotunda to the shrine of mirrors.
        vec![
            (10.0, -84.0),
            (-30.0, -84.0),
            (-50.0, -84.0),
            (-64.0, -84.0),
            (-76.0, -86.0),
            (-88.0, -92.0),
        ],
        // East door.
        vec![
            (10.0, -84.0),
            (30.0, -84.0),
            (50.0, -84.0),
            (64.0, -84.0),
            (76.0, -86.0),
            (88.0, -92.0),
        ],
        // The cinder watch's rounds.
        vec![(-18.0, -92.0), (0.0, -92.0), (18.0, -92.0)],
        // Round the pillars of the boss floor.
        vec![
            (0.0, -204.0),
            (-6.0, -222.0),
            (-20.0, -222.0),
            (-30.0, -236.0),
            (-6.0, -248.0),
            (20.0, -222.0),
            (30.0, -236.0),
        ],
    ];
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
fn every_shrine_and_the_guardian_are_needed_to_open_the_gate() {
    let clear = |names: &[&str]| {
        let mut simulation = sanctum();
        for name in names {
            for member in pack(&simulation, name) {
                simulation.slay(member);
            }
        }
        simulation.step_once();
        (
            simulation.dungeon_run().expect("a run").percent(),
            simulation.gate_is_closed(),
        )
    };
    assert_eq!(clear(&BEFORE_THE_GATE), (100, false));
    // Skip any one pack and the gate stays shut.
    for skipped in BEFORE_THE_GATE {
        let rest: Vec<&str> = BEFORE_THE_GATE
            .into_iter()
            .filter(|name| *name != skipped)
            .collect();
        let (percent, closed) = clear(&rest);
        assert!(percent < 100 && closed, "skipping {skipped} was enough");
    }
    for shrine in SHRINES {
        assert!(BEFORE_THE_GATE.contains(&shrine));
    }
}

#[test]
fn the_whole_dungeon_can_be_cleared_and_the_run_completes() {
    let mut simulation = sanctum();
    for name in BEFORE_THE_GATE {
        for member in pack(&simulation, name) {
            simulation.slay(member);
        }
    }
    simulation.step_once();
    assert_eq!(simulation.dungeon_run().expect("a run").percent(), 100);
    assert!(!simulation.gate_is_closed());
    // Pulled, as a real fight is, then killed with whatever he called.
    let pontiff = pack(&simulation, "THE ASHEN PONTIFF")[0];
    let at = simulation.world().position(pontiff).expect("a boss");
    let player = simulation.player().entity;
    put(&mut simulation, player, at + Vec3::new(0.0, 0.0, 10.0));
    run(&mut simulation, 90);
    // Taken through the pylons: he is held until they fall, then dies.
    wound_to(&mut simulation, pontiff, 0.49);
    run(&mut simulation, 90);
    for member in pack(&simulation, "THE ASHEN PONTIFF") {
        if member != pontiff {
            simulation.slay(member);
        }
    }
    simulation.slay(pontiff);
    run(&mut simulation, 10);
    assert!(simulation.dungeon_run().expect("a run").is_completed());
    assert!(health(&simulation, player) > 0.0);
}

#[test]
fn the_real_floors_pillars_stop_the_rite_and_the_open_floor_does_not() {
    // The great pillars the generator stands on the sanctum floor are solid to sight as well
    // as to feet: shelter behind each is real, in the level that ships.
    let scene = engine_environment::scene_file::load(scene_path()).expect("loads");
    let great: Vec<Vec3> = scene
        .objects
        .iter()
        .filter(|object| object.asset.name() == "pillar" && object.transform.scale.x > 2.0)
        .map(|object| object.transform.translation)
        .collect();
    assert_eq!(great.len(), 6, "six great pillars");
    let mut simulation = sanctum();
    simulation.unlock_dungeon();
    let pontiff = pack(&simulation, "THE ASHEN PONTIFF")[0];
    let boss_at = simulation.world().position(pontiff).expect("a boss");
    let player = simulation.player().entity;
    // Pull him onto the floor and into the rite.
    put(&mut simulation, player, boss_at + Vec3::new(0.0, 0.0, 14.0));
    wound_to(&mut simulation, pontiff, 0.74);
    let (_, anchor, _, delay) =
        first_telegraph(&mut simulation, 60 * 40, EnemyAbility::RiteOfCinders);
    // Behind the pillar nearest the anchor, on the far side from it.
    let column = *great
        .iter()
        .min_by(|a, b| {
            horizontal_distance(**a, anchor)
                .partial_cmp(&horizontal_distance(**b, anchor))
                .expect("finite")
        })
        .expect("a pillar");
    let away = (column - anchor).normalize();
    let hidden = Vec3::new(column.x, 0.0, column.z) + Vec3::new(away.x, 0.0, away.z) * 2.6;
    put(&mut simulation, player, hidden);
    // A second player stands in the open, as the control.
    let open = add_players(&mut simulation, &[Vec3::ZERO])[0];
    put(&mut simulation, open, anchor + Vec3::new(0.0, 0.0, 6.0));
    let events = run(&mut simulation, (delay * 60.0) as u32 + 30);
    assert!(
        damage_from(&events, EnemyAbility::RiteOfCinders, player).is_empty(),
        "the pillar at {column:?} did not shelter {hidden:?} from {anchor:?}"
    );
    assert!(
        !damage_from(&events, EnemyAbility::RiteOfCinders, open).is_empty(),
        "the rite missed a player in the open"
    );
}

#[test]
fn the_mini_boss_pays_a_table_of_its_own_and_the_last_boss_pays_through_the_chest() {
    use engine_game::character::{
        CatalogItem, ItemFamily, ItemIcon, ItemQuality, Stat, item_by_key, register_catalog_items,
    };
    use engine_game::loot::{
        BossTable, CurrencyRules, EpicRules, GoldRules, LootRules, LootTable, MobLootRules,
        QualityBucket, QualityRules, WeightedItem,
    };
    let row = |key: &str, quality| CatalogItem {
        key: key.to_owned(),
        name: key.to_owned(),
        family: ItemFamily::Ring,
        quality,
        item_level: 13,
        icon: ItemIcon::Ring,
        stats: vec![(Stat::Stamina, 10.0)],
        active: None,
        set: None,
        sockets: 0,
        gem_level: 0,
    };
    register_catalog_items(vec![
        row("sa_warden_ring", ItemQuality::Rare),
        row("sa_pontiff_ring", ItemQuality::Rare),
    ]);
    let table = |key: &str| LootTable {
        entries: vec![WeightedItem {
            item: item_by_key(key).expect("registered").id,
            weight: 1.0,
        }],
    };
    let rules = LootRules {
        equipment_per_completion: 1,
        quality: QualityRules {
            buckets: vec![QualityBucket {
                min: 50,
                max: 110,
                weight: 1.0,
            }],
            high_from: 100,
            high_bucket_weight_per_key_level: 0.0,
        },
        epic: EpicRules {
            base_chance: 0.0,
            per_key_level: 0.0,
            max_chance: 0.0,
        },
        boss_tables: vec![
            BossTable {
                boss: "censer_warden".to_owned(),
                table: table("sa_warden_ring"),
            },
            BossTable {
                boss: "ashen_pontiff".to_owned(),
                table: table("sa_pontiff_ring"),
            },
        ],
        mob: MobLootRules {
            equipment_chance: 0.0,
            table: table("sa_pontiff_ring"),
        },
        currency: CurrencyRules {
            resource: "aether_shard",
            per_mob_chance: 0.0,
            elite_chance: 0.0,
            amount: 1,
        },
        gold: GoldRules {
            resource: "gold",
            per_mob_min: 2,
            per_mob_max: 2,
            boss_amount: 40,
        },
        sharpening: engine_game::loot::SharpeningDrops::none(),
        gems: engine_game::loot::GemDrops::none(),
        books: None,
    };
    let mut simulation = sanctum();
    simulation.set_loot_rules(rules, 1, 0);
    let player = simulation.player().entity;
    let warden = pack(&simulation, "GALLERY OF CENSERS")[0];
    let events = hit(&mut simulation, warden, 1.0e9);
    let drops: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::MobLoot { player, item, .. } => Some((*player, *item)),
            _ => None,
        })
        .collect();
    assert_eq!(drops.len(), 1, "one roll for the one player: {drops:?}");
    assert_eq!(drops[0].0, player);
    assert_eq!(drops[0].1, item_by_key("sa_warden_ring").expect("item").id);
    // The last boss does not drop at his feet: his table is the chest's.
    simulation.unlock_dungeon();
    let pontiff = pack(&simulation, "THE ASHEN PONTIFF")[0];
    let events = hit(&mut simulation, pontiff, 1.0e9);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CombatEvent::MobLoot { .. })),
        "the final boss dropped an item at his feet"
    );
}

#[test]
fn the_ember_pylon_is_never_placed_and_effects_table_covers_it() {
    // A pylon in a level would hold nothing: it only means something when a boss calls it.
    assert!(EnemyKind::EmberPylon.archetype().shields_boss);
    assert!(matches!(
        EnemyAbility::AshTithe.definition().effect,
        EnemyEffect::Damage
    ));
}

#[test]
fn the_pylon_immunity_is_a_status_that_appears_and_disappears_with_the_pylons() {
    let (mut simulation, pontiff, _players, pylons) = pylon_fight();
    // The tick the pylons rose, the boss was marked, and the start was announced once.
    assert!(has_status(&simulation, pontiff, StatusKind::Shielded));
    let events = run(&mut simulation, 60);
    assert!(
        !events.iter().any(|event| matches!(
            event,
            CombatEvent::StatusApplied { status: StatusKind::Shielded, .. }
        )),
        "the shield is announced once, not every tick"
    );
    // Still held while any pylon stands.
    for pylon in &pylons[..2] {
        simulation.slay(*pylon);
    }
    run(&mut simulation, 5);
    assert!(has_status(&simulation, pontiff, StatusKind::Shielded));
    // The last one falls: the status goes, with an event the interface and audio read.
    simulation.slay(pylons[2]);
    let events = run(&mut simulation, 5);
    assert!(!has_status(&simulation, pontiff, StatusKind::Shielded));
    assert!(events.iter().any(|event| matches!(
        event,
        CombatEvent::StatusRemoved { target, status: StatusKind::Shielded, .. } if *target == pontiff
    )));
}
