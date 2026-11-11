//! The Graveyard Crypt: its six monsters, its completion, its gate, its boss and his adds.
//!
//! Every test here is deterministic — a fixed timestep, a fixed seed — and headless: the
//! dungeon is built by the same function the game builds it with, from the same scene the
//! editor saves.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_environment::EnvironmentScene;
use engine_game::arena::{self, ArenaConfig};
use engine_game::dungeon::{DungeonScaling, GRAVEYARD_CRYPT_FILE, build_level_file};
use engine_game::encounter::{Encounter, EncounterState, EnemySpawn};
use engine_game::enemy::brain::AiState;
use engine_game::enemy::{
    EnemyAbility, EnemyEffect, EnemyKind, GRAVEYARD_ROSTER, MonsterRole, MonsterTier,
    TelegraphShape,
};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::status::StatusKind;

// ------------------------------------------------------------------------------------------
// The level, read from its file
// ------------------------------------------------------------------------------------------

/// The Graveyard Crypt's level file: the one the editor opens and saves.
fn level_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(GRAVEYARD_CRYPT_FILE)
}

/// The level as the file says it.
fn level() -> EnvironmentScene {
    engine_environment::scene_file::load(level_path()).expect("the crypt's level file loads")
}

/// The crypt for a party of `scaling`, built from the file.
fn build_graveyard_crypt(scaling: DungeonScaling) -> Simulation {
    build_level_file(level_path(), scaling)
        .expect("the crypt's level file builds")
        .0
}

/// Which way through the crypt a pack belongs to.
///
/// The design's own grouping of the level's packs, by name — the one piece of the dungeon
/// the tests state rather than read, because the file names packs and not routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Main,
    Crypt,
    Graveyard,
    Boss,
}

const ROUTES: [(&str, Route); 14] = [
    ("GATE WARDENS", Route::Main),
    ("BONE CHOIR", Route::Main),
    ("CROSSING SENTRIES", Route::Main),
    ("MOURNERS", Route::Main),
    ("CLOISTER GUARD", Route::Main),
    ("OSSUARY WATCH", Route::Crypt),
    ("CANDLE VIGIL", Route::Crypt),
    ("BONE CHAPTER", Route::Crypt),
    ("CRYPT CHOIR", Route::Crypt),
    ("GRAVEDIGGERS", Route::Graveyard),
    ("RESTLESS DEAD", Route::Graveyard),
    ("CHARNEL PIT", Route::Graveyard),
    ("FEEDING FRENZY", Route::Graveyard),
    ("HOLLOW STEWARD", Route::Boss),
];

/// One pack of the level: its name, its route and its members' ids and positions.
#[derive(Debug, Clone)]
struct Pack {
    name: String,
    route: Route,
    members: Vec<(String, Vec3)>,
}

/// Every pack in the level file, in the order the file first names them.
fn all_packs() -> Vec<Pack> {
    let mut packs: Vec<Pack> = Vec::new();
    for (index, monster) in level().monsters.iter().enumerate() {
        let name = monster.pack_key(index);
        let route = ROUTES
            .iter()
            .find(|(pack, _)| *pack == name)
            .map(|(_, route)| *route)
            .unwrap_or_else(|| panic!("the level has a pack the design does not: {name}"));
        match packs.iter_mut().find(|pack| pack.name == name) {
            Some(pack) => pack
                .members
                .push((monster.monster.clone(), monster.position)),
            None => packs.push(Pack {
                name,
                route,
                members: vec![(monster.monster.clone(), monster.position)],
            }),
        }
    }
    packs
}

// ------------------------------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------------------------------

/// A player with a great deal of health, so a test about what a monster *does* is not cut
/// short by what it does.
fn sturdy(simulation: &mut Simulation) {
    let player = simulation.player().entity;
    simulation.world_mut().set_max_health(player, 1_000_000.0);
    simulation.world_mut().restore(player);
}

/// One monster, `distance` metres in front of the player, in the training arena's floor.
fn duel(kind: EnemyKind, distance: f32) -> (Simulation, EntityId) {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("DUEL", vec![EnemySpawn::new(kind, spawn)]);
    let mut simulation = arena::encounter_arena(&config, encounter);
    sturdy(&mut simulation);
    let enemy = simulation
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    (simulation, enemy)
}

fn run(simulation: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
    }
    events
}

fn state(simulation: &Simulation, enemy: EntityId) -> AiState {
    simulation
        .brain(enemy)
        .map(engine_game::EnemyBrain::state)
        .expect("the entity is an enemy")
}

fn health(simulation: &Simulation, entity: EntityId) -> f32 {
    simulation
        .world()
        .health(entity)
        .map_or(0.0, |health| health.current())
}

/// Every enemy ability that dealt damage in `events`.
fn damaging_abilities(events: &[CombatEvent]) -> Vec<EnemyAbility> {
    let mut seen: Vec<EnemyAbility> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) => damage.ability.enemy(),
            _ => None,
        })
        .collect();
    seen.sort_by_key(|ability| ability.index());
    seen.dedup();
    seen
}

/// Damage `ability` dealt in `events`, in order.
fn hits_of(events: &[CombatEvent], ability: EnemyAbility) -> Vec<f32> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) if damage.ability.enemy() == Some(ability) => {
                Some(damage.amount)
            }
            _ => None,
        })
        .collect()
}

/// The first cast `enemy` started in `events`.
fn first_cast(events: &[CombatEvent], enemy: EntityId) -> Option<EnemyAbility> {
    events.iter().find_map(|event| match event {
        CombatEvent::CastStarted {
            source, ability, ..
        } if *source == enemy => ability.enemy(),
        _ => None,
    })
}

/// Moves the player to `position`, at rest.
fn put_player(simulation: &mut Simulation, position: Vec3) {
    let player = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(player) {
        *body = engine_game::Body::at(position);
    }
}

/// The crypt for a party of three, one tick in, with a sturdy player.
fn crypt() -> Simulation {
    let mut simulation = build_graveyard_crypt(DungeonScaling::default());
    sturdy(&mut simulation);
    simulation.step_once();
    simulation
}

/// The encounter index of a crypt pack, by its name.
fn pack_index(simulation: &Simulation, name: &str) -> usize {
    simulation
        .encounters()
        .iter()
        .position(|encounter| encounter.name() == name)
        .unwrap_or_else(|| panic!("no pack called {name}"))
}

/// Kills every member of a pack through the ordinary damage path.
fn clear_pack(simulation: &mut Simulation, name: &str) {
    let index = pack_index(simulation, name);
    let members = simulation.encounters()[index].members().to_vec();
    for member in members {
        simulation.slay(member);
    }
    simulation.step_once();
}

fn percent(simulation: &Simulation) -> u32 {
    simulation.dungeon_run().map_or(0, |run| run.percent())
}

fn packs_on(route: Route) -> Vec<Pack> {
    all_packs()
        .into_iter()
        .filter(|pack| pack.route == route)
        .collect()
}

fn boss_index(simulation: &Simulation) -> usize {
    simulation
        .dungeon_run()
        .and_then(|run| run.rules().boss_encounter)
        .expect("the crypt has a boss")
}

fn boss(simulation: &Simulation) -> EntityId {
    simulation.encounters()[boss_index(simulation)].members()[0]
}

/// The crypt with the main path and Route B cleared: exactly 100 %.
fn crypt_at_full_completion() -> Simulation {
    let mut simulation = crypt();
    for pack in packs_on(Route::Main)
        .into_iter()
        .chain(packs_on(Route::Graveyard))
    {
        clear_pack(&mut simulation, &pack.name);
    }
    simulation
}

/// Walks the player into the unlocked boss hall and lets the boss notice them.
fn pull_boss(simulation: &mut Simulation) -> Vec<CombatEvent> {
    let boss = boss(simulation);
    let position = simulation.world().position(boss).expect("a boss");
    put_player(simulation, position + Vec3::new(0.0, 0.0, 6.0));
    run(simulation, 90)
}

// ------------------------------------------------------------------------------------------
// The six monsters
// ------------------------------------------------------------------------------------------

#[test]
fn all_six_monsters_are_defined_and_resolvable_by_their_ids() {
    let mut ranged = 0;
    let mut melee = 0;
    for kind in GRAVEYARD_ROSTER {
        let archetype = kind.archetype();
        assert_eq!(
            EnemyKind::from_id(archetype.id),
            Some(kind),
            "{}",
            archetype.name
        );
        assert_eq!(archetype.tier, MonsterTier::Normal);
        match archetype.role {
            MonsterRole::Ranged => ranged += 1,
            MonsterRole::Melee => melee += 1,
            other => panic!("{} is {other:?}", archetype.name),
        }
        let abilities: Vec<EnemyAbility> = archetype.abilities().collect();
        assert!(
            (3..=4).contains(&abilities.len()),
            "{} has {} abilities",
            archetype.name,
            abilities.len()
        );
        assert!(
            archetype.completion > 0.0,
            "{} is worth nothing",
            archetype.name
        );
        assert!(
            archetype.empowered.is_some(),
            "{} has no empowered form",
            archetype.name
        );
    }
    assert_eq!((ranged, melee), (3, 3));
}

#[test]
fn every_monster_ability_is_offensive() {
    // The design rule: no shields, no heals, no damage reduction. Everything a crypt
    // monster does either hurts somebody or makes it hurt somebody sooner.
    for kind in GRAVEYARD_ROSTER
        .iter()
        .copied()
        .chain(GRAVEYARD_ROSTER.iter().filter_map(|kind| kind.empowered()))
        .chain(std::iter::once(EnemyKind::CryptSteward))
    {
        let archetype = kind.archetype();
        for phase in 0..archetype.phase_count() {
            for ability in archetype.phase_abilities(phase) {
                let definition = ability.definition();
                let offensive = definition.damage > 0.0
                    || matches!(definition.effect, EnemyEffect::Frenzy { .. });
                assert!(offensive, "{} has {}", archetype.name, definition.name);
                assert!(
                    !definition.effect.is_healing(),
                    "{} heals with {}",
                    archetype.name,
                    definition.name
                );
            }
        }
    }
}

#[test]
fn the_six_monsters_are_six_different_problems() {
    // No two monsters share their signature ability, and no two share their whole list:
    // six monsters with different numbers on the same attacks would be one monster.
    for (index, a) in GRAVEYARD_ROSTER.iter().enumerate() {
        for b in GRAVEYARD_ROSTER.iter().skip(index + 1) {
            let first_a = a.archetype().abilities().next();
            let first_b = b.archetype().abilities().next();
            assert_ne!(first_a, first_b, "{a:?} and {b:?}");
            let list_a: Vec<_> = a.archetype().abilities().collect();
            let list_b: Vec<_> = b.archetype().abilities().collect();
            assert!(
                list_a.iter().all(|ability| !list_b.contains(ability)),
                "{a:?} {b:?}"
            );
        }
    }
}

#[test]
fn every_monster_spawns_with_its_health_and_dies_through_the_damage_path() {
    for kind in GRAVEYARD_ROSTER {
        let (mut simulation, enemy) = duel(kind, 40.0);
        let expected = kind.archetype().max_health;
        assert!(
            (health(&simulation, enemy) - expected).abs() < 1e-3,
            "{kind:?}"
        );
        simulation.slay(enemy);
        // A kill outside a tick is announced at once, in the batch the next step replaces.
        assert!(
            simulation.events().iter().any(
                |event| matches!(event, CombatEvent::Death { entity, .. } if *entity == enemy)
            ),
            "{kind:?} did not announce its death"
        );
        run(&mut simulation, 2);
        assert_eq!(state(&simulation, enemy), AiState::Dead, "{kind:?}");
    }
}

#[test]
fn every_monster_ignores_a_player_outside_its_aggro_and_notices_one_inside_it() {
    for kind in GRAVEYARD_ROSTER {
        let aggro = kind.archetype().aggro_radius;
        let (mut simulation, enemy) = duel(kind, aggro + 6.0);
        run(&mut simulation, 120);
        assert_eq!(
            state(&simulation, enemy),
            AiState::Idle,
            "{kind:?} noticed too far"
        );

        let (mut simulation, enemy) = duel(kind, aggro - 3.0);
        run(&mut simulation, 3);
        assert!(
            state(&simulation, enemy) != AiState::Idle,
            "{kind:?} did not notice the player at {} m",
            aggro - 3.0
        );
    }
}

#[test]
fn every_monster_uses_several_of_its_abilities_and_they_all_hurt() {
    // Twenty-five seconds next to a player who never fights back: every monster has to have
    // hurt the player with at least three different abilities, and every one of them has to
    // have gone on its cooldown when it was used.
    for kind in GRAVEYARD_ROSTER {
        let archetype = kind.archetype();
        let distance = archetype
            .preferred_range
            .min(archetype.aggro_radius - 2.0)
            .max(2.0);
        let (mut simulation, enemy) = duel(kind, distance);
        let before = health(&simulation, simulation.player().entity);
        let mut used = Vec::new();
        let mut events = Vec::new();
        for _ in 0..(60 * 25) {
            simulation.step_once();
            for event in simulation.events() {
                if let CombatEvent::AbilityUsed {
                    source, ability, ..
                }
                | CombatEvent::CastStarted {
                    source, ability, ..
                } = event
                    && *source == enemy
                    && let Some(ability) = ability.enemy()
                {
                    let brain = simulation.brain(enemy).expect("an enemy");
                    // A cooldown starts when the ability does.
                    assert!(
                        brain.cooldown(ability) > 0.0 || ability.definition().cooldown <= 0.0,
                        "{kind:?} used {} and it is not on cooldown",
                        ability.name()
                    );
                    used.push(ability);
                }
            }
            events.extend_from_slice(simulation.events());
        }
        let hurt = damaging_abilities(&events);
        used.sort_by_key(|ability| ability.index());
        used.dedup();
        // Three different abilities used against one player who stands still; at least two of
        // them hurt. (A leap needs somebody to leap at who is not already in reach, and a
        // frenzy hurts through the swings it hastens rather than by itself.)
        assert!(
            used.len() >= 3,
            "{kind:?} only used {:?}",
            used.iter()
                .map(|ability| ability.name())
                .collect::<Vec<_>>()
        );
        assert!(
            hurt.len() >= 2,
            "{kind:?} only hurt with {:?}",
            hurt.iter()
                .map(|ability| ability.name())
                .collect::<Vec<_>>()
        );
        assert!(
            health(&simulation, simulation.player().entity) < before,
            "{kind:?}"
        );
        for ability in &hurt {
            assert!(
                kind.archetype().abilities().any(|own| own == *ability),
                "{kind:?} dealt damage with {} which is not its own",
                ability.name()
            );
        }
    }
}

#[test]
fn a_monster_left_behind_walks_home_and_forgets_the_fight() {
    for kind in GRAVEYARD_ROSTER {
        let (mut simulation, enemy) = duel(kind, 6.0);
        run(&mut simulation, 120);
        assert!(
            state(&simulation, enemy).is_combat() || state(&simulation, enemy) == AiState::Recover
        );
        simulation.restore_enemy(enemy);
        assert_eq!(state(&simulation, enemy), AiState::Idle, "{kind:?}");
        assert!(
            (health(&simulation, enemy) - kind.archetype().max_health).abs() < 1e-3,
            "{kind:?} was not made whole"
        );
        let brain = simulation.brain(enemy).expect("an enemy");
        assert!(
            brain.threat().is_empty() && brain.cast().is_none(),
            "{kind:?}"
        );
    }
}

#[test]
fn ranged_monsters_keep_their_distance_and_melee_monsters_close() {
    for kind in GRAVEYARD_ROSTER {
        let (mut simulation, enemy) = duel(kind, kind.archetype().aggro_radius - 1.0);
        run(&mut simulation, 60 * 6);
        let player = simulation.player_position();
        let at = simulation.world().position(enemy).expect("an enemy");
        let gap = engine_game::world::horizontal_distance(player, at);
        match kind.archetype().role {
            MonsterRole::Ranged => assert!(gap > 8.0, "{kind:?} walked into melee: {gap:.1} m"),
            _ => assert!(gap < 8.0, "{kind:?} stayed away: {gap:.1} m"),
        }
    }
}

// ------------------------------------------------------------------------------------------
// Signature mechanics
// ------------------------------------------------------------------------------------------

#[test]
fn the_acolytes_drain_grows_every_tick_and_an_interrupt_ends_it() {
    let (mut simulation, acolyte) = duel(EnemyKind::GraveAcolyte, 10.0);
    let events = run(&mut simulation, 60 * 3);
    assert_eq!(first_cast(&events, acolyte), Some(EnemyAbility::GraveDrain));
    let ticks = hits_of(&events, EnemyAbility::GraveDrain);
    assert!(ticks.len() >= 3, "{ticks:?}");
    assert!(
        ticks.windows(2).all(|pair| pair[1] > pair[0]),
        "each tick must grow: {ticks:?}"
    );

    // A fresh one, kicked a second into its drain: nothing more lands.
    let (mut simulation, acolyte) = duel(EnemyKind::GraveAcolyte, 10.0);
    let mut kicked = false;
    let mut after = Vec::new();
    for _ in 0..(60 * 8) {
        simulation.step_once();
        let casting = simulation
            .brain(acolyte)
            .and_then(|brain| brain.cast())
            .filter(|cast| cast.ability == EnemyAbility::GraveDrain);
        if !kicked && casting.is_some_and(|cast| cast.elapsed > 1.0) {
            assert!(
                simulation.interrupt(acolyte),
                "the drain must be interruptible"
            );
            kicked = true;
            continue;
        }
        if kicked {
            after.extend_from_slice(simulation.events());
        }
    }
    assert!(kicked, "the acolyte never drained");
    // Its cooldown is eighteen seconds: nothing called Grave Drain lands in the eight after.
    assert!(hits_of(&after, EnemyAbility::GraveDrain).is_empty());
}

#[test]
fn the_bonecasters_mark_follows_its_victim_and_lands_on_them() {
    let (mut simulation, bonecaster) = duel(EnemyKind::SkeletonBonecaster, 10.0);
    let player = simulation.player().entity;
    let mut marked = None;
    let mut events = Vec::new();
    for _ in 0..(60 * 3) {
        simulation.step_once();
        for event in simulation.events() {
            if let CombatEvent::TelegraphStarted {
                follow: Some(follow),
                ability,
                ..
            } = event
                && ability.enemy() == Some(EnemyAbility::GraveExplosion)
            {
                marked = Some(*follow);
            }
        }
        events.extend_from_slice(simulation.events());
        if marked.is_some() {
            break;
        }
    }
    assert_eq!(
        first_cast(&events, bonecaster),
        Some(EnemyAbility::GraveExplosion)
    );
    assert_eq!(marked, Some(player), "the mark is on the player");
    // The player walks off with it; it goes with them.
    let away = simulation.player_position() + Vec3::new(9.0, 0.0, 6.0);
    put_player(&mut simulation, away);
    simulation.step_once();
    let strike = simulation.enemy_strikes().strikes()[0];
    assert!(
        engine_game::world::horizontal_distance(strike.anchor, away) < 0.5,
        "the mark stayed behind"
    );
    let events = run(&mut simulation, 60 * 5);
    assert_eq!(hits_of(&events, EnemyAbility::GraveExplosion).len(), 1);
}

#[test]
fn killing_the_bonecaster_in_time_saves_whoever_it_marked() {
    let (mut simulation, bonecaster) = duel(EnemyKind::SkeletonBonecaster, 10.0);
    for _ in 0..(60 * 3) {
        simulation.step_once();
        if !simulation.enemy_strikes().is_empty() {
            break;
        }
    }
    assert!(!simulation.enemy_strikes().is_empty(), "no mark was laid");
    simulation.slay(bonecaster);
    assert!(
        simulation.events().iter().any(|event| matches!(
            event,
            CombatEvent::TelegraphEnded {
                resolved: false,
                ..
            }
        )),
        "the mark's warning must come down with its caster"
    );
    let events = run(&mut simulation, 60 * 6);
    assert!(simulation.enemy_strikes().is_empty());
    assert!(hits_of(&events, EnemyAbility::GraveExplosion).is_empty());
}

#[test]
fn stepping_out_of_the_brutes_bone_quake_avoids_it() {
    let (mut simulation, brute) = duel(EnemyKind::GraveBrute, 2.0);
    let mut laid = false;
    let mut events = Vec::new();
    for _ in 0..(60 * 4) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if simulation
            .enemy_strikes()
            .strikes()
            .iter()
            .any(|strike| strike.ability == EnemyAbility::BoneQuake)
        {
            laid = true;
            break;
        }
    }
    assert_eq!(first_cast(&events, brute), Some(EnemyAbility::BoneQuake));
    assert!(laid, "the quake was never laid");
    // Well outside its nine metres before it lands.
    let brute_at = simulation.world().position(brute).expect("a brute");
    put_player(&mut simulation, brute_at + Vec3::new(0.0, 0.0, 14.0));
    let events = run(&mut simulation, 60 * 3);
    assert!(
        hits_of(&events, EnemyAbility::BoneQuake).is_empty(),
        "the quake hit a player outside it"
    );
}

#[test]
fn standing_in_the_brutes_bone_quake_is_punished() {
    let (mut simulation, _) = duel(EnemyKind::GraveBrute, 2.0);
    let events = run(&mut simulation, 60 * 5);
    let hits = hits_of(&events, EnemyAbility::BoneQuake);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0] > 50.0);
}

#[test]
fn a_skeleton_warrior_charges_a_distant_player_hits_them_and_slows_them() {
    let (mut simulation, warrior) = duel(EnemyKind::SkeletonWarrior, 10.0);
    let player = simulation.player().entity;
    let mut charged = false;
    let mut slowed = false;
    let mut events = Vec::new();
    for _ in 0..(60 * 5) {
        simulation.step_once();
        charged |= simulation.is_charging(warrior);
        events.extend_from_slice(simulation.events());
        // The slow is caught while it lasts: one second from the landing.
        if !hits_of(simulation.events(), EnemyAbility::BoneCharge).is_empty() {
            slowed = simulation
                .world()
                .statuses(player)
                .and_then(|statuses| statuses.get(StatusKind::Crippled))
                .is_some_and(|status| (status.duration - 1.0).abs() < 1e-3);
            assert!(simulation.move_speed_multiplier() < 0.6);
        }
    }
    assert!(charged, "the warrior never charged");
    // Instant: no cast bar and no wind-up — the run starts when it is chosen — and nothing
    // drawn on the floor.
    assert!(
        !events.iter().any(|event| matches!(
            event,
            CombatEvent::CastStarted { source, ability, .. }
                if *source == warrior && ability.enemy() == Some(EnemyAbility::BoneCharge)
        )),
        "a charge is instant, not a cast"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            CombatEvent::TelegraphStarted { ability, .. }
                if ability.enemy() == Some(EnemyAbility::BoneCharge)
        )),
        "a charge is not telegraphed on the floor"
    );
    assert_eq!(hits_of(&events, EnemyAbility::BoneCharge).len(), 1);
    assert!(slowed, "the charged player was not slowed for a second");
    let gap = engine_game::world::horizontal_distance(
        simulation.player_position(),
        simulation.world().position(warrior).expect("a warrior"),
    );
    assert!(gap < 4.0, "the warrior stopped {gap:.1} m short");
}

#[test]
fn the_ghoul_frenzies_and_its_claws_stack_a_bleed() {
    let (mut simulation, ghoul) = duel(EnemyKind::CryptGhoul, 2.0);
    let events = run(&mut simulation, 60);
    assert!(
        events.iter().any(|event| matches!(
            event,
            CombatEvent::AbilityUsed { source, ability, .. }
                if *source == ghoul && ability.enemy() == Some(EnemyAbility::BloodFrenzy)
        )),
        "the ghoul opens with its frenzy"
    );
    assert!(simulation.brain(ghoul).expect("a ghoul").is_frenzied());
    run(&mut simulation, 60 * 12);
    let player = simulation.player().entity;
    let stacks = simulation
        .world()
        .statuses(player)
        .map_or(0, |statuses| statuses.stacks(StatusKind::Rended));
    assert!(stacks >= 2, "the bleed never stacked: {stacks}");
}

#[test]
fn a_frenzied_ghoul_attacks_faster_than_a_calm_one() {
    let (mut simulation, ghoul) = duel(EnemyKind::CryptGhoul, 2.0);
    let events = run(&mut simulation, 60 * 7);
    let frenzied = hits_of(&events, EnemyAbility::GhoulStrike).len();
    let brain = simulation.brain(ghoul).expect("a ghoul");
    assert!(brain.haste() <= 1.0 + 1e-3 || brain.is_frenzied());
    // The same seven seconds against a calm one: its frenzy is spent at once, so the only
    // difference between the two runs is whether the frenzy was up.
    assert!(frenzied >= 3, "{frenzied} strikes in a frenzy");
}

#[test]
fn the_archers_crippling_arrow_slows_the_player() {
    let (mut simulation, _) = duel(EnemyKind::CryptArcher, 10.0);
    let player = simulation.player().entity;
    let mut slowed = false;
    for _ in 0..(60 * 20) {
        simulation.step_once();
        if simulation
            .world()
            .statuses(player)
            .is_some_and(|statuses| statuses.has(StatusKind::Crippled))
        {
            slowed = true;
            assert!(simulation.move_speed_multiplier() < 0.6);
            break;
        }
    }
    assert!(slowed, "no crippling arrow in twenty seconds");
}

#[test]
fn the_archers_rain_is_drawn_before_it_lands_and_lands_where_it_was_drawn() {
    let (mut simulation, archer) = duel(EnemyKind::CryptArcher, 10.0);
    let events = run(&mut simulation, 60 * 2);
    assert_eq!(
        first_cast(&events, archer),
        Some(EnemyAbility::RainOfArrows)
    );
    let zones: Vec<(u32, f32)> = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::TelegraphStarted {
                id, ability, delay, ..
            } if ability.enemy() == Some(EnemyAbility::RainOfArrows) => Some((*id, *delay)),
            _ => None,
        })
        .collect();
    assert_eq!(zones.len(), 2, "two circles on one player");
    assert!(zones.iter().all(|(_, delay)| *delay >= 2.0), "{zones:?}");
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
fn execution_strike_winds_up_and_an_interrupt_saves_the_tank() {
    let (mut simulation, warrior) = duel(EnemyKind::SkeletonWarrior, 1.5);
    let mut kicked = false;
    let mut events = Vec::new();
    for _ in 0..(60 * 4) {
        simulation.step_once();
        events.extend_from_slice(simulation.events());
        if simulation
            .brain(warrior)
            .and_then(|brain| brain.cast())
            .is_some_and(|cast| cast.ability == EnemyAbility::ExecutionStrike && cast.elapsed > 1.0)
        {
            assert!(simulation.interrupt(warrior));
            kicked = true;
            break;
        }
    }
    assert!(kicked, "the warrior never wound up an execution");
    let events = run(&mut simulation, 60 * 2);
    assert!(hits_of(&events, EnemyAbility::ExecutionStrike).is_empty());
    assert!(events.iter().all(|event| !matches!(
        event,
        CombatEvent::CastEnded { end: CastEnd::Completed, ability, .. }
            if ability.enemy() == Some(EnemyAbility::ExecutionStrike)
    )));
}

#[test]
fn a_bone_spear_is_a_line_and_a_sidestep_escapes_it() {
    let shape = TelegraphShape::Line {
        yaw: 0.0,
        length: 30.0,
        width: 2.6,
    };
    // Yaw zero runs along -Z from the anchor.
    assert!(shape.contains(Vec3::ZERO, Vec3::new(0.0, 0.0, -20.0), 0.4));
    assert!(shape.contains(Vec3::ZERO, Vec3::new(1.5, 0.0, -20.0), 0.4));
    assert!(
        !shape.contains(Vec3::ZERO, Vec3::new(3.0, 0.0, -20.0), 0.4),
        "a sidestep"
    );
    assert!(
        !shape.contains(Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0), 0.4),
        "behind the caster"
    );
    assert!(
        !shape.contains(Vec3::ZERO, Vec3::new(0.0, 0.0, -31.0), 0.4),
        "past its end"
    );
}

// ------------------------------------------------------------------------------------------
// Completion
// ------------------------------------------------------------------------------------------

#[test]
fn the_crypt_is_built_with_every_pack_and_starts_at_zero() {
    let simulation = crypt();
    let run = simulation
        .dungeon_run()
        .expect("the crypt is a dungeon run");
    assert_eq!(run.name(), "GRAVEYARD CRYPT");
    assert_eq!(run.percent(), 0);
    assert!(!run.is_unlocked());
    assert!(simulation.gate_is_closed());
    assert_eq!(simulation.encounters().len(), all_packs().len());
    for encounter in simulation.encounters() {
        assert_eq!(
            encounter.state(),
            EncounterState::NotStarted,
            "{}",
            encounter.name()
        );
    }
}

#[test]
fn killing_a_monster_credits_exactly_what_it_is_worth() {
    let mut simulation = crypt();
    let index = pack_index(&simulation, "GATE WARDENS");
    let warrior = simulation.encounters()[index].members()[0];
    let worth = simulation.completion_value(warrior);
    assert!((worth - EnemyKind::SkeletonWarrior.archetype().completion).abs() < 1e-4);
    simulation.slay(warrior);
    simulation.step_once();
    let run = simulation.dungeon_run().expect("a run");
    assert!((run.credited() - worth).abs() < 1e-4);
    // Once: a corpse slain again is not worth it twice.
    simulation.slay(warrior);
    simulation.step_once();
    assert!((simulation.dungeon_run().expect("a run").credited() - worth).abs() < 1e-4);
}

#[test]
fn the_main_path_alone_is_not_enough() {
    let mut simulation = crypt();
    for pack in packs_on(Route::Main) {
        clear_pack(&mut simulation, &pack.name);
    }
    let reached = percent(&simulation);
    assert!(reached < 100, "{reached} %");
    assert!(
        reached > 40,
        "the main path is most of the way: {reached} %"
    );
    assert!(simulation.gate_is_closed());
}

#[test]
fn either_optional_route_on_top_of_the_main_path_reaches_one_hundred() {
    for route in [Route::Crypt, Route::Graveyard] {
        let mut simulation = crypt();
        for pack in packs_on(Route::Main).into_iter().chain(packs_on(route)) {
            clear_pack(&mut simulation, &pack.name);
        }
        assert_eq!(percent(&simulation), 100, "{route:?}");
        assert!(
            simulation.dungeon_run().expect("a run").is_unlocked(),
            "{route:?}"
        );
        assert!(!simulation.gate_is_closed(), "{route:?}: the gate stands");
    }
}

#[test]
fn optional_route_packs_contribute_and_skipping_one_falls_short() {
    for route in [Route::Crypt, Route::Graveyard] {
        let packs = packs_on(route);
        let mut simulation = crypt();
        for pack in packs_on(Route::Main)
            .into_iter()
            .chain(packs.iter().skip(1).cloned())
        {
            clear_pack(&mut simulation, &pack.name);
        }
        // Route B is exactly a hundred; route A has three to spare, and no pack is worth
        // three or less.
        assert!(
            percent(&simulation) < 100,
            "{route:?}: skipping a pack still reached 100"
        );
        let before = percent(&simulation);
        clear_pack(&mut simulation, &packs[0].name);
        assert!(
            percent(&simulation) > before,
            "{route:?}: the skipped pack is worth nothing"
        );
    }
}

#[test]
fn completion_never_passes_one_hundred() {
    let mut simulation = crypt();
    for pack in all_packs().iter().filter(|pack| pack.route != Route::Boss) {
        clear_pack(&mut simulation, &pack.name);
    }
    let run = simulation.dungeon_run().expect("a run");
    assert!(
        run.credited() > run.required(),
        "both routes are worth more than required"
    );
    assert_eq!(run.percent(), 100);
    assert!((run.fraction() - 1.0).abs() < f32::EPSILON);
}

#[test]
fn the_progress_is_announced_with_each_kill_and_the_unlock_once() {
    let mut simulation = crypt();
    let mut progress = 0;
    let mut unlocks = 0;
    for pack in packs_on(Route::Main)
        .into_iter()
        .chain(packs_on(Route::Graveyard))
    {
        let index = pack_index(&simulation, &pack.name);
        for member in simulation.encounters()[index].members().to_vec() {
            simulation.slay(member);
        }
        // One read of the batch the kills went into, then the step that replaces it.
        for event in simulation.events() {
            match event {
                CombatEvent::DungeonProgress { .. } => progress += 1,
                CombatEvent::BossUnlocked { .. } => unlocks += 1,
                _ => {}
            }
        }
        simulation.step_once();
    }
    let kills: usize = packs_on(Route::Main)
        .into_iter()
        .chain(packs_on(Route::Graveyard))
        .map(|pack| pack.members.len())
        .sum();
    assert_eq!(progress, kills);
    assert_eq!(unlocks, 1);
}

// ------------------------------------------------------------------------------------------
// The gate and the boss
// ------------------------------------------------------------------------------------------

#[test]
fn the_boss_is_sealed_below_one_hundred_per_cent() {
    let mut simulation = crypt();
    let boss = boss(&simulation);
    let position = simulation.world().position(boss).expect("a boss");
    // Standing right next to him, having somehow got past the gate.
    put_player(&mut simulation, position + Vec3::new(0.0, 0.0, 3.0));
    let before = health(&simulation, boss);
    run(&mut simulation, 120);
    assert_eq!(
        state(&simulation, boss),
        AiState::Idle,
        "a sealed boss noticed somebody"
    );
    // Nothing reaches him through the bars.
    simulation.slay(boss);
    simulation.step_once();
    assert!((health(&simulation, boss) - before).abs() < 1e-3);
    assert!(simulation.dungeon_run().expect("a run").adds().is_empty());
}

#[test]
fn the_gate_blocks_the_boss_hall_until_the_dungeon_is_complete() {
    let mut simulation = crypt();
    let gate = level()
        .dungeon
        .and_then(|settings| settings.boss_gate)
        .expect("the level has a gate");
    // Walk at the gate from the cloister side for three seconds.
    put_player(&mut simulation, gate.center + Vec3::new(0.0, 0.0, 6.0));
    for _ in 0..180 {
        // Yaw zero looks along -Z: straight at the gate.
        simulation.push_command(engine_game::PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_once();
    }
    assert!(
        simulation.player_position().z > gate.center.z,
        "the player walked through a closed gate: {:?}",
        simulation.player_position()
    );
}

#[test]
fn at_one_hundred_per_cent_the_boss_can_be_pulled_and_calls_exactly_two_empowered_adds() {
    let mut simulation = crypt_at_full_completion();
    assert!(!simulation.gate_is_closed());
    let events = pull_boss(&mut simulation);
    let boss = boss(&simulation);
    assert!(
        state(&simulation, boss) != AiState::Idle,
        "the boss did not notice the player"
    );

    let run = simulation.dungeon_run().expect("a run");
    assert!(run.is_engaged());
    assert_eq!(run.adds().len(), 2, "exactly two adds");
    assert_eq!(run.chosen().len(), 2);
    assert_ne!(run.chosen()[0], run.chosen()[1], "two different monsters");
    for (add, kind) in run.adds().iter().zip(run.chosen()) {
        assert!(
            GRAVEYARD_ROSTER.contains(kind),
            "{kind:?} is not from the pool"
        );
        let brain = simulation.brain(*add).expect("the add is an enemy");
        assert_eq!(
            Some(brain.kind()),
            kind.empowered(),
            "the add is the empowered form"
        );
        assert_eq!(brain.archetype().tier, MonsterTier::Empowered);
        assert!(simulation.world().is_alive(*add));
    }
    let summoned = events
        .iter()
        .filter(|event| matches!(event, CombatEvent::AddSummoned { .. }))
        .count();
    assert_eq!(summoned, 2);
    // And they are part of the boss's fight: the encounter is not won while they stand.
    let index = boss_index(&simulation);
    assert_eq!(simulation.encounters()[index].summon_count(), 2);
}

#[test]
fn the_same_seed_calls_the_same_adds_and_a_forced_pair_is_honoured() {
    let chosen = || {
        let mut simulation = crypt_at_full_completion();
        pull_boss(&mut simulation);
        simulation.dungeon_run().expect("a run").chosen().to_vec()
    };
    assert_eq!(chosen(), chosen(), "a seed is a replay");

    let mut simulation = crypt_at_full_completion();
    simulation
        .dungeon_run_mut()
        .expect("a run")
        .force_adds(Some(vec![EnemyKind::GraveBrute, EnemyKind::CryptGhoul]));
    pull_boss(&mut simulation);
    assert_eq!(
        simulation.dungeon_run().expect("a run").chosen(),
        &[EnemyKind::GraveBrute, EnemyKind::CryptGhoul]
    );
}

#[test]
fn over_many_seeds_every_monster_is_called_and_never_twice_at_once() {
    let mut seen = Vec::new();
    for seed in 0..24_u64 {
        let scene = level();
        let mut simulation =
            arena::environment_arena_for_party(&scene, 0, DungeonScaling::default());
        simulation.reseed_enemies(seed);
        sturdy(&mut simulation);
        simulation.unlock_dungeon();
        pull_boss(&mut simulation);
        let chosen = simulation.dungeon_run().expect("a run").chosen().to_vec();
        assert_eq!(chosen.len(), 2, "seed {seed}");
        assert_ne!(chosen[0], chosen[1], "seed {seed}");
        seen.extend(chosen);
    }
    seen.sort_by_key(|kind| kind.index());
    seen.dedup();
    assert_eq!(seen.len(), 6, "only {seen:?} were ever called");
}

#[test]
fn every_empowered_add_escalates_mechanically_rather_than_by_numbers_alone() {
    for kind in GRAVEYARD_ROSTER {
        let normal = kind.archetype();
        let empowered = kind.empowered().expect("an empowered form").archetype();
        let normal_list: Vec<EnemyAbility> = normal.abilities().collect();
        let empowered_list: Vec<EnemyAbility> = empowered.abilities().collect();
        let new: Vec<&EnemyAbility> = empowered_list
            .iter()
            .filter(|ability| !normal_list.contains(ability))
            .collect();
        assert!(
            new.len() >= 2,
            "{} escalates {} abilities",
            empowered.name,
            new.len()
        );
        // And it keeps something the party already knows, so it is recognisable.
        assert!(
            empowered_list
                .iter()
                .any(|ability| normal_list.contains(ability)),
            "{} shares nothing with {}",
            empowered.name,
            normal.name
        );
        // Each new ability does something its counterpart structurally does not: more zones,
        // more targets, less warning, a chain, a trail, a shockwave, growth or a requirement.
        for ability in new {
            let definition = ability.definition();
            let escalates = definition.zone_total() > 1
                || definition.chain
                || matches!(
                    definition.effect,
                    EnemyEffect::Charge { shockwave_radius, trail_damage_per_second, .. }
                        if shockwave_radius > 0.0 || trail_damage_per_second > 0.0
                )
                || matches!(definition.effect, EnemyEffect::Hazard { grow_to, radius, .. } if grow_to > 0.0 || radius > 0.0)
                || definition.requires != engine_game::enemy::EnemyRequirement::None
                || matches!(definition.shape, TelegraphShape::Cone { half_angle, .. } if half_angle > engine_game::enemy::WIDE_CONE_HALF_ANGLE)
                || matches!(definition.rider, engine_game::enemy::EnemyRider::Bleed { stacks, .. } if stacks > 1);
            assert!(escalates, "{} is only bigger numbers", definition.name);
        }
    }
}

#[test]
fn a_wipe_on_the_boss_takes_the_adds_away_and_lets_him_be_pulled_again() {
    let mut simulation = crypt_at_full_completion();
    pull_boss(&mut simulation);
    let adds = simulation.dungeon_run().expect("a run").adds().to_vec();
    let boss = boss(&simulation);
    // The party leaves: the boss loses them and walks home.
    put_player(&mut simulation, level().player_spawn().position);
    simulation.kill_player();
    run(&mut simulation, 60 * 20);
    assert_eq!(state(&simulation, boss), AiState::Idle);
    let run_state = simulation.dungeon_run().expect("a run");
    assert!(!run_state.is_engaged());
    for add in &adds {
        assert!(
            !simulation.world().contains(*add),
            "an add outlived the fight"
        );
    }
    assert!(
        (health(&simulation, boss) - simulation.world().health(boss).expect("a boss").max()).abs()
            < 1e-3
    );
    // Revived, back in, and he calls a fresh pair.
    simulation.revive_player();
    sturdy(&mut simulation);
    pull_boss(&mut simulation);
    assert_eq!(simulation.dungeon_run().expect("a run").adds().len(), 2);
}

#[test]
fn killing_the_adds_and_the_boss_completes_the_dungeon_once() {
    let mut simulation = crypt_at_full_completion();
    pull_boss(&mut simulation);
    let adds = simulation.dungeon_run().expect("a run").adds().to_vec();
    for add in adds {
        simulation.slay(add);
    }
    simulation.step_once();
    assert!(
        !simulation.dungeon_run().expect("a run").is_completed(),
        "the boss still stands"
    );
    let boss = boss(&simulation);
    simulation.slay(boss);
    let events = run(&mut simulation, 3);
    let completions = events
        .iter()
        .filter(|event| matches!(event, CombatEvent::DungeonCompleted { .. }))
        .count();
    assert_eq!(completions, 1);
    assert!(simulation.dungeon_run().expect("a run").is_completed());
    run(&mut simulation, 60);
    assert!(simulation.dungeon_run().expect("a run").is_completed());
}

#[test]
fn a_charge_follows_a_player_who_runs() {
    // Cast on a player, not at a spot: stepping sideways during the wind-up does not escape.
    let (mut simulation, warrior) = duel(EnemyKind::SkeletonWarrior, 10.0);
    for _ in 0..(60 * 3) {
        simulation.step_once();
        if simulation.is_charging(warrior) {
            break;
        }
    }
    assert!(simulation.is_charging(warrior), "the warrior never charged");
    let aside = simulation.player_position() + Vec3::new(6.0, 0.0, 0.0);
    put_player(&mut simulation, aside);
    let events = run(&mut simulation, 60 * 2);
    assert_eq!(
        hits_of(&events, EnemyAbility::BoneCharge).len(),
        1,
        "the charge lost its target"
    );
}

#[test]
fn a_wipe_resets_the_living_and_leaves_the_dead_dead() {
    let mut simulation = crypt();
    // The first pack cleared, the second pulled and hurt, then the party dies.
    clear_pack(&mut simulation, "GATE WARDENS");
    let credited = simulation.dungeon_run().expect("a run").credited();
    let index = pack_index(&simulation, "BONE CHOIR");
    let choir = simulation.encounters()[index].members().to_vec();
    let position = simulation.world().position(choir[1]).expect("a warrior");
    put_player(&mut simulation, position + Vec3::new(0.0, 0.0, 2.0));
    run(&mut simulation, 60 * 3);
    assert!(simulation.player_in_combat());
    simulation.slay(choir[0]);
    simulation.kill_player();
    // The respawn: three seconds on the floor, then back at the start.
    run(&mut simulation, 60 * 5);
    assert!(simulation.player_alive());

    let wardens = pack_index(&simulation, "GATE WARDENS");
    for member in simulation.encounters()[wardens].members() {
        assert!(
            !simulation.world().is_alive(*member),
            "a killed monster came back"
        );
    }
    assert!(
        !simulation.world().is_alive(choir[0]),
        "the Bonecaster killed before the wipe came back"
    );
    let survivor = choir[1];
    assert!(simulation.world().is_alive(survivor));
    assert_eq!(
        state(&simulation, survivor),
        AiState::Idle,
        "the survivor did not reset"
    );
    assert!(
        (health(&simulation, survivor)
            - simulation
                .world()
                .health(survivor)
                .expect("a warrior")
                .max())
        .abs()
            < 1e-3,
        "the survivor was not made whole"
    );
    // Progress is kept: what was killed before the wipe is still credited.
    let after = simulation.dungeon_run().expect("a run").credited();
    assert!(after >= credited, "the wipe took completion away");
}

#[test]
fn backspace_after_dying_recovers_without_reviving_the_dead() {
    let mut simulation = crypt();
    clear_pack(&mut simulation, "GATE WARDENS");
    simulation.kill_player();
    simulation.step_once();
    // The reset key, pressed while dead.
    simulation.reset_dummies();
    assert!(simulation.player_alive());
    let wardens = pack_index(&simulation, "GATE WARDENS");
    for member in simulation.encounters()[wardens].members() {
        assert!(!simulation.world().is_alive(*member));
    }
    assert!(simulation.dungeon_run().expect("a run").credited() > 0.0);
}

#[test]
fn a_dungeon_reset_puts_everything_back() {
    let mut simulation = crypt_at_full_completion();
    pull_boss(&mut simulation);
    let adds = simulation.dungeon_run().expect("a run").adds().to_vec();
    simulation.reset_encounter();
    simulation.step_once();
    let run = simulation.dungeon_run().expect("a run");
    assert_eq!(run.percent(), 0);
    assert!(!run.is_unlocked() && !run.is_engaged() && !run.is_completed());
    assert!(simulation.gate_is_closed());
    assert!(simulation.enemy_strikes().is_empty());
    for add in adds {
        assert!(!simulation.world().contains(add));
    }
    for encounter in simulation.encounters() {
        assert_eq!(encounter.state(), EncounterState::NotStarted);
        assert_eq!(encounter.summon_count(), 0);
        for member in encounter.members() {
            assert!(simulation.world().is_alive(*member), "{}", encounter.name());
            assert_eq!(state(&simulation, *member), AiState::Idle);
        }
    }
    // And it can be run again: the first pack is worth what it was worth the first time.
    clear_pack(&mut simulation, "GATE WARDENS");
    assert!(simulation.dungeon_run().expect("a run").credited() > 0.0);
}

// ------------------------------------------------------------------------------------------
// Balance for a party of three
// ------------------------------------------------------------------------------------------

/// Everybody's health: the three heroes all start at 500.
const PARTY_HEALTH: f32 = 500.0;

/// What a Warrior's base armour takes off a physical hit: 500 / (500 + 1000).
const TANK_ARMOUR: f32 = 1.0 / 3.0;

/// The healing one Druid is assumed to sustain, per second.
const HEALER_HPS: f32 = 45.0;

/// The party's single-target damage per second: a damage dealer, a tank and a healer's
/// spare global cooldowns.
const PARTY_DPS: f32 = 180.0;

fn avoidable(ability: EnemyAbility) -> bool {
    let definition = ability.definition();
    // A charge is cast on a player and follows them: it is not avoided, it is endured.
    definition.shape.is_area() || definition.delay > 0.0
}

#[test]
fn an_avoidable_hit_is_survivable_once_and_lethal_if_ignored_twice() {
    // Survivable once: under a third of a full health bar, so a player at full health who
    // stood in it is punished, not killed. Lethal if ignored: three of them kill.
    for kind in GRAVEYARD_ROSTER
        .iter()
        .copied()
        .chain(GRAVEYARD_ROSTER.iter().filter_map(|kind| kind.empowered()))
        .chain(std::iter::once(EnemyKind::CryptSteward))
    {
        for phase in 0..kind.archetype().phase_count() {
            for ability in kind.archetype().phase_abilities(phase) {
                if !avoidable(ability) {
                    continue;
                }
                // Everything one use deals to a player who stood in all of it: a channel's
                // ticks summed, a charge and its shockwave.
                // A pool is the mechanic rather than its landing: two seconds of standing in it
                // is what ignoring it costs.
                let pool = match ability.definition().effect {
                    EnemyEffect::Hazard {
                        damage_per_second, ..
                    } => damage_per_second * 2.0,
                    _ => 0.0,
                };
                let hit = ability.definition().damage_per_use() + pool;
                assert!(
                    hit <= PARTY_HEALTH * 0.25,
                    "{}: {hit} from an avoidable {} is more than a quarter of a player",
                    kind.name(),
                    ability.name()
                );
                // And big enough to matter: a telegraph worth ignoring teaches ignoring them.
                assert!(
                    hit >= PARTY_HEALTH * 0.08,
                    "{}: {} is too small to matter",
                    kind.name(),
                    ability.name()
                );
            }
        }
    }
}

#[test]
fn every_telegraph_gives_the_party_time_to_react() {
    for ability in EnemyAbility::ALL {
        let definition = ability.definition();
        // What a death leaves has no cast to warn with: the warning is the monster's own health.
        let on_death = EnemyKind::ALL
            .iter()
            .any(|kind| kind.archetype().on_death == Some(ability));
        if !avoidable(ability)
            || on_death
            || EnemyKind::ALL[..7].iter().any(|legacy| {
                legacy.archetype().abilities().any(|own| own == ability)
                    || (0..legacy.archetype().phase_count()).any(|phase| {
                        legacy
                            .archetype()
                            .phase_abilities(phase)
                            .any(|own| own == ability)
                    })
            })
        {
            continue;
        }
        let travel = match definition.effect {
            EnemyEffect::Charge { speed, .. } => definition.min_range / speed,
            _ => 0.0,
        };
        let warning = definition.cast_time + definition.delay + travel;
        assert!(
            warning >= 0.9,
            "{}: {warning:.2} s of warning is not a reaction window",
            definition.name
        );
    }
}

#[test]
fn a_packs_unavoidable_damage_on_the_tank_is_within_one_healers_reach() {
    // What a pack does to a tank who does everything right: every melee filler and
    // single-target cast, through armour, at its cooldown. It has to sit under what one
    // healer sustains, or a clean pull still wipes.
    for pack in all_packs().iter().filter(|pack| pack.route != Route::Boss) {
        let mut on_tank = 0.0;
        for (id, _) in &pack.members {
            let kind = EnemyKind::from_id(id).expect("a monster");
            let filler = kind
                .archetype()
                .abilities()
                .filter(|ability| !avoidable(*ability))
                .map(|ability| {
                    let definition = ability.definition();
                    let per_use = definition.damage_per_use();
                    let mitigated = if definition.damage_type == engine_game::DamageType::Physical {
                        per_use * (1.0 - TANK_ARMOUR)
                    } else {
                        per_use
                    };
                    mitigated / (definition.cooldown + definition.cast_time).max(1.0)
                })
                .fold(0.0_f32, f32::max);
            on_tank += filler;
        }
        assert!(
            on_tank <= HEALER_HPS * 1.25,
            "{}: {on_tank:.1} a second on the tank against a healer's {HEALER_HPS}",
            pack.name
        );
    }
}

#[test]
fn packs_take_long_enough_for_their_mechanics_and_not_so_long_they_drag() {
    for pack in all_packs().iter().filter(|pack| pack.route != Route::Boss) {
        let total: f32 = pack
            .members
            .iter()
            .map(|(id, _)| {
                EnemyKind::from_id(id)
                    .expect("a monster")
                    .archetype()
                    .max_health
            })
            .sum();
        let seconds = total / PARTY_DPS;
        assert!(
            (10.0..=40.0).contains(&seconds),
            "{}: {seconds:.0} s at {PARTY_DPS} damage a second",
            pack.name
        );
    }
}

#[test]
fn the_boss_fight_is_a_fight_and_not_a_siege() {
    let boss = EnemyKind::CryptSteward.archetype().max_health;
    // The two largest adds he can call.
    let mut adds: Vec<f32> = GRAVEYARD_ROSTER
        .iter()
        .filter_map(|kind| kind.empowered())
        .map(|kind| kind.archetype().max_health)
        .collect();
    adds.sort_by(f32::total_cmp);
    let worst = boss + adds[adds.len() - 1] + adds[adds.len() - 2];
    let minutes = worst / PARTY_DPS / 60.0;
    assert!((1.5..=3.5).contains(&minutes), "{minutes:.1} minutes");
}

#[test]
fn every_crypt_monster_is_slower_than_the_player_in_every_phase() {
    let player = engine_game::movement::MovementConfig::default().max_speed;
    for kind in EnemyKind::ALL[7..].iter() {
        let archetype = kind.archetype();
        for phase in 0..archetype.phase_count() {
            assert!(
                archetype.phase_speed(phase) < player,
                "{} in phase {phase} runs {:.2}",
                archetype.name,
                archetype.phase_speed(phase)
            );
        }
    }
}

#[test]
fn a_party_of_one_meets_the_same_mechanics_with_smaller_numbers() {
    let solo = build_graveyard_crypt(DungeonScaling::for_party(1));
    let party = build_graveyard_crypt(DungeonScaling::default());
    let index = |simulation: &Simulation| pack_index(simulation, "GATE WARDENS");
    let solo_warrior = solo.encounters()[index(&solo)].members()[0];
    let party_warrior = party.encounters()[index(&party)].members()[0];
    assert!(health(&solo, solo_warrior) < health(&party, party_warrior));
    // Same kind, same abilities, same completion value.
    assert_eq!(
        solo.brain(solo_warrior).map(|brain| brain.kind()),
        party.brain(party_warrior).map(|brain| brain.kind())
    );
    assert!(
        (solo.completion_value(solo_warrior) - party.completion_value(party_warrior)).abs() < 1e-4
    );
}

// ------------------------------------------------------------------------------------------
// The level file itself
// ------------------------------------------------------------------------------------------

#[test]
fn the_level_file_names_only_monsters_the_game_knows_and_is_a_valid_dungeon() {
    let scene = level();
    assert!(!scene.monsters.is_empty());
    for (index, monster) in scene.monsters.iter().enumerate() {
        assert!(monster.is_valid(), "monsters[{index}]");
        assert!(
            EnemyKind::from_id(&monster.monster).is_some(),
            "monsters[{index}] is '{}', which is not a monster",
            monster.monster
        );
    }
    let settings = scene.dungeon.as_ref().expect("the crypt is a dungeon");
    assert!(settings.is_valid());
    assert_eq!(settings.add_count, 2);
    assert!(!settings.allow_duplicate_adds);
    let pool: Vec<EnemyKind> = settings
        .add_pool
        .iter()
        .filter_map(|id| EnemyKind::from_id(id))
        .collect();
    assert_eq!(
        pool,
        GRAVEYARD_ROSTER.to_vec(),
        "the pool is the six monsters"
    );
    assert!(settings.boss_gate.is_some());
    assert_eq!(settings.add_spawns.len(), 2);
    // Every route of the design is in the file, and nothing that is not.
    for (name, _) in ROUTES {
        assert!(
            all_packs().iter().any(|pack| pack.name == name),
            "{name} is missing"
        );
    }
}

#[test]
fn every_gate_arch_in_the_crypt_stands_on_a_pier_at_each_foot() {
    // An arch placed anywhere but between two piers is an arch hanging in the air: its feet
    // are built to rest on piers exactly a span apart, and nothing else holds it up.
    use engine_environment::asset::{CRYPT_GATE, CRYPT_GATE_SPAN, WALL_PIER};
    let scene = level();
    let piers: Vec<Vec3> = scene
        .objects
        .iter()
        .filter(|object| object.asset == WALL_PIER)
        .map(|object| object.position())
        .collect();
    let gates: Vec<_> = scene
        .objects
        .iter()
        .filter(|object| object.asset == CRYPT_GATE)
        .collect();
    assert!(
        gates.len() >= 18,
        "every doorway has a gate: {}",
        gates.len()
    );
    for gate in gates {
        let across = gate.transform.rotation.rotate(Vec3::X);
        for side in [-1.0_f32, 1.0] {
            let foot = gate.position() + across * (side * CRYPT_GATE_SPAN * 0.5);
            assert!(
                piers.iter().any(|pier| (*pier - foot).length() < 0.1),
                "the gate at {:?} has no pier under its foot at {foot:?}",
                gate.position()
            );
        }
    }
}

#[test]
fn two_packs_stand_further_apart_than_anything_in_them_can_notice() {
    // Entering a room pulls one pack, not two. Twenty-two metres: the Crypt Archer's aggro
    // radius, the widest in the roster.
    let packs = all_packs();
    for (index, a) in packs.iter().enumerate() {
        for b in packs.iter().skip(index + 1) {
            for (_, at) in &a.members {
                for (_, bt) in &b.members {
                    let gap = engine_game::world::horizontal_distance(*at, *bt);
                    assert!(gap > 22.0, "{} and {} are {gap:.1} m apart", a.name, b.name);
                }
            }
        }
    }
}

#[test]
fn no_monster_is_placed_inside_something_solid() {
    let scene = level();
    for monster in &scene.monsters {
        for object in scene
            .objects
            .iter()
            .filter(|object| object.collision.blocks())
        {
            let Some(solid) = object.world_collision() else {
                continue;
            };
            let local = solid
                .rotation
                .conjugate()
                .rotate(monster.position - solid.center);
            let inside = local.x.abs() < solid.half_extents.x + 0.8
                && local.z.abs() < solid.half_extents.z + 0.8;
            assert!(
                !inside,
                "{} at {:?} stands inside {} at {:?}",
                monster.monster,
                monster.position,
                object.asset.name(),
                object.position()
            );
        }
    }
}

#[test]
fn the_player_starts_out_of_every_packs_reach() {
    let scene = level();
    let start = scene.player_spawn().position;
    for monster in &scene.monsters {
        let kind = EnemyKind::from_id(&monster.monster).expect("a monster");
        let gap = engine_game::world::horizontal_distance(start, monster.position);
        assert!(
            gap > kind.archetype().aggro_radius,
            "{} notices the player before they have moved",
            monster.monster
        );
    }
}

// ------------------------------------------------------------------------------------------
// Edit, save, reload, play
// ------------------------------------------------------------------------------------------

#[test]
fn a_monster_placed_and_saved_is_in_the_fight_when_the_level_is_loaded_again() {
    // The editor's round trip, without the editor's window: take the level, add a Crypt
    // Ghoul to the lychgate in a pack of its own, write the file, read it back, build it.
    let mut scene = level();
    let start = scene.player_spawn().position;
    let mut ghoul = engine_environment::MonsterPlacement::new(
        "crypt_ghoul",
        start + Vec3::new(8.0, 0.0, -10.0),
    )
    .in_pack("TEST GHOUL");
    ghoul.level = 2;
    scene.push_monster(ghoul);
    let path = std::env::temp_dir().join(format!(
        "mygame_graveyard_roundtrip_{}.json",
        std::process::id()
    ));
    engine_environment::scene_file::save(&scene, &path).expect("the level saves");
    let (mut simulation, loaded) =
        build_level_file(&path, DungeonScaling::default()).expect("and loads");
    std::fs::remove_file(&path).ok();
    assert_eq!(loaded.monsters.len(), level().monsters.len() + 1);

    let index = pack_index(&simulation, "TEST GHOUL");
    let placed = simulation.encounters()[index].members()[0];
    let brain = simulation.brain(placed).expect("the ghoul is an enemy");
    assert_eq!(brain.kind(), EnemyKind::CryptGhoul);
    // Its level is applied: level two has more health than the table's number.
    assert!(health(&simulation, placed) > EnemyKind::CryptGhoul.archetype().max_health);
    // And it fights: the player standing next to it is noticed and hurt.
    sturdy(&mut simulation);
    let position = simulation.world().position(placed).expect("a ghoul");
    put_player(&mut simulation, position + Vec3::new(0.0, 0.0, 2.0));
    let events = run(&mut simulation, 60 * 4);
    assert!(
        state(&simulation, placed).is_combat() || state(&simulation, placed) == AiState::Recover
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            CombatEvent::Damage(damage) if damage.source == placed
        )),
        "the placed ghoul never hit anybody"
    );
    // A monster the author added is worth its monster's completion.
    let before = simulation.dungeon_run().expect("a run").credited();
    simulation.slay(placed);
    simulation.step_once();
    let after = simulation.dungeon_run().expect("a run").credited();
    assert!((after - before - EnemyKind::CryptGhoul.archetype().completion).abs() < 1e-4);
}

#[test]
fn a_switched_off_placement_is_not_spawned() {
    let mut scene = level();
    let off = scene.monsters.len();
    scene.push_monster(
        engine_environment::MonsterPlacement::new("grave_brute", Vec3::new(0.0, 0.0, -20.0))
            .in_pack("SWITCHED OFF"),
    );
    scene.monsters[off].enabled = false;
    let simulation = arena::environment_arena_for_party(&scene, 0, DungeonScaling::default());
    assert!(
        simulation
            .encounters()
            .iter()
            .all(|encounter| encounter.name() != "SWITCHED OFF")
    );
}

#[test]
fn clearing_and_rebuilding_the_monsters_puts_the_dungeon_back_as_the_level_says() {
    // What leaving the editor does to a running game.
    let mut simulation = crypt_at_full_completion();
    pull_boss(&mut simulation);
    simulation.clear_enemies();
    assert_eq!(simulation.enemy_count(), 0);
    assert!(simulation.dungeon_run().is_none());
    assert!(!simulation.gate_is_closed());
    let report = engine_game::dungeon::populate_from_scene(
        &mut simulation,
        &level(),
        DungeonScaling::default(),
    );
    assert_eq!(report.monsters, level().monsters.len());
    assert!(report.unknown.is_empty());
    assert_eq!(percent(&simulation), 0);
    assert!(
        simulation.gate_is_closed(),
        "the rebuilt dungeon is sealed again"
    );
}
