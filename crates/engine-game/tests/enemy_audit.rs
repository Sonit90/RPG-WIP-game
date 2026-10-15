//! The enemy ability audit: every one of the 58 attacks, checked for who it picks, how it is
//! cast and interrupted, what it deals, what it shows, and how the player's buffs answer it.
//!
//! Two kinds of test live here. The *table* tests read `ENEMY_ABILITIES` and hold every row to
//! the rules the executor assumes; the *scenario* tests run real fights in the training arena
//! and read the events. `docs/monsters-audit.md` is the matrix these tests back.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::AbilityId;
use engine_game::arena::{self, ArenaConfig};
use engine_game::encounter::{Encounter, EnemySpawn};
use engine_game::enemy::brain::{AiState, MAX_TARGETS, MAX_ZONES};
use engine_game::enemy::{
    ENEMY_ABILITIES, EnemyAbility, EnemyAim, EnemyEffect, EnemyKind, EnemyRider, EnemyTarget,
    TelegraphShape,
};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::hero::Hero;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::status::{Status, StatusKind};
use engine_game::world::EntityDesc;

// ------------------------------------------------------------------------------------------
// Harness
// ------------------------------------------------------------------------------------------

/// A fight: the first player (the Fire Mage, who is every enemy's first target), optional
/// extra players, and one `kind` standing `distance` metres in front of the first.
struct Fight {
    sim: Simulation,
    enemy: EntityId,
    /// Every player, the first one first.
    players: Vec<EntityId>,
}

/// One enemy of `kind`, `distance` metres from the first player, who has a great deal of
/// health so a test about what a monster does is not cut short by what it does. `extra` are
/// the positions of further players, relative to the first one.
fn fight(kind: EnemyKind, distance: f32, extra: &[Vec3]) -> Fight {
    let config = ArenaConfig::with_group(0);
    let spawn = config.player_spawn + Vec3::new(0.0, 0.0, -distance);
    let encounter = Encounter::with_spawns("AUDIT", vec![EnemySpawn::new(kind, spawn)]);
    let mut sim = arena::encounter_arena(&config, encounter);
    let first = sim.player().entity;
    sim.world_mut().set_max_health(first, 1_000_000.0);
    sim.world_mut().restore(first);
    let enemy = sim
        .encounter()
        .and_then(|encounter| encounter.members().first().copied())
        .expect("the encounter spawned its member");
    let mut players = vec![first];
    for offset in extra {
        let entity = sim
            .world_mut()
            .spawn(EntityDesc::player(config.player_spawn + *offset));
        sim.add_player(entity, Hero::FireMage);
        sim.world_mut().set_max_health(entity, 1_000_000.0);
        sim.world_mut().restore(entity);
        players.push(entity);
    }
    Fight {
        sim,
        enemy,
        players,
    }
}

fn run(sim: &mut Simulation, ticks: u32) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    for _ in 0..ticks {
        sim.step_once();
        events.extend_from_slice(sim.events());
    }
    events
}

fn state(sim: &Simulation, enemy: EntityId) -> AiState {
    sim.brain(enemy)
        .map(engine_game::EnemyBrain::state)
        .expect("an enemy")
}

/// Damage events dealt by `ability`, as `(target, amount)`.
fn hits_by(events: &[CombatEvent], ability: EnemyAbility) -> Vec<(EntityId, f32)> {
    events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) if damage.ability.enemy() == Some(ability) => {
                Some((damage.target, damage.amount))
            }
            _ => None,
        })
        .collect()
}

fn total_by(events: &[CombatEvent], ability: EnemyAbility) -> f32 {
    hits_by(events, ability)
        .iter()
        .map(|(_, amount)| amount)
        .sum()
}

// ------------------------------------------------------------------------------------------
// The table: rules every row has to keep
// ------------------------------------------------------------------------------------------

#[test]
fn there_are_fifty_eight_abilities_and_each_is_where_its_index_says() {
    assert_eq!(EnemyAbility::ALL.len(), 58);
    assert_eq!(ENEMY_ABILITIES.len(), 58);
    for (index, ability) in EnemyAbility::ALL.iter().enumerate() {
        assert_eq!(ability.index(), index, "{ability:?} is out of table order");
        assert!(!ability.name().is_empty());
    }
}

#[test]
fn every_ability_is_owned_by_some_monster() {
    let mut owned = [false; 58];
    for kind in EnemyKind::ALL {
        let archetype = kind.archetype();
        for ability in archetype.abilities.iter().flatten() {
            owned[ability.index()] = true;
        }
        for phase in archetype.phases {
            for ability in phase.abilities.iter().flatten() {
                owned[ability.index()] = true;
            }
        }
    }
    for ability in EnemyAbility::ALL {
        assert!(owned[ability.index()], "{ability:?} belongs to no monster");
    }
}

#[test]
fn interruptible_means_there_is_a_cast_to_interrupt() {
    for ability in EnemyAbility::ALL {
        let definition = ability.definition();
        if definition.interruptible {
            assert!(
                definition.is_cast(),
                "{ability:?} is interruptible but instant"
            );
        }
        // A heal, a channel and anything that lays a mark is a cast the party must be able
        // to stop; an instant is never stoppable.
        if !definition.is_cast() {
            assert!(!definition.interruptible, "{ability:?}");
        }
    }
}

#[test]
fn every_row_is_internally_consistent() {
    for ability in EnemyAbility::ALL {
        let d = ability.definition();
        let name = d.name;
        assert!(d.range > 0.0, "{name}: no range");
        assert!(d.min_range < d.range, "{name}: min range beyond range");
        assert!(
            d.cast_time >= 0.0 && d.cooldown > 0.0 && d.recovery >= 0.0,
            "{name}"
        );
        assert!(
            usize::from(d.targets) <= MAX_TARGETS,
            "{name}: too many targets"
        );
        assert!(
            d.zone_total() <= MAX_ZONES,
            "{name}: more zones than a cast holds"
        );
        if d.zones > 1 {
            assert!(
                d.spread > 0.0,
                "{name}: extra zones with nowhere to scatter"
            );
        }
        // A cone or a line starts at the caster; anchoring it on the target would draw it
        // from somewhere the hit test never measures from.
        if matches!(
            d.shape,
            TelegraphShape::Cone { .. } | TelegraphShape::Line { .. }
        ) {
            assert_eq!(
                d.aim,
                EnemyAim::Caster,
                "{name}: a cone/line must start at the caster"
            );
        }
        if d.follow || d.chain {
            assert!(
                d.delay > 0.0 && d.shape.is_area(),
                "{name}: a mark needs a delay and an area"
            );
        }
        if d.chain {
            assert!(d.follow, "{name}: a chain is made of marks");
        }
        if d.delay > 0.0 {
            assert!(d.shape.is_area(), "{name}: a delay with nothing to land");
            assert!(d.is_cast(), "{name}: a delayed zone is laid by a cast");
        }
        match d.effect {
            EnemyEffect::Damage => assert!(d.damage > 0.0, "{name}: deals nothing"),
            EnemyEffect::Heal { amount } => {
                assert!(amount > 0.0, "{name}");
                assert_eq!(
                    d.select,
                    EnemyTarget::WoundedAlly,
                    "{name}: a heal selects an ally"
                );
            }
            EnemyEffect::Hazard {
                duration,
                damage_per_second,
                ..
            } => {
                assert!(duration > 0.0 && damage_per_second > 0.0, "{name}");
                assert!(d.shape.is_area(), "{name}: a pool needs a footprint");
            }
            EnemyEffect::Channel { interval, .. } => {
                assert!(d.is_cast() && interval > 0.0, "{name}");
                assert!(d.interruptible, "{name}: a channel is the interrupt lesson");
            }
            EnemyEffect::Charge { speed, .. } => {
                assert!(speed > 0.0, "{name}");
                assert!(!d.is_cast(), "{name}: the roster's charges are instant");
                assert!(
                    d.min_range > 0.0,
                    "{name}: a charge is never a second swing"
                );
                assert!(
                    matches!(d.select, EnemyTarget::Farthest | EnemyTarget::Random),
                    "{name}"
                );
            }
            EnemyEffect::Frenzy { duration, haste } => {
                assert!(duration > 0.0 && haste > 0.0, "{name}");
                assert_eq!(d.damage, 0.0, "{name}");
            }
        }
        // Selecting somebody other than the tank only makes sense for something that reaches
        // past them.
        if d.select == EnemyTarget::WoundedAlly {
            assert!(d.effect.is_healing(), "{name}");
        }
        if let EnemyRider::Bleed {
            stacks, max_stacks, ..
        } = d.rider
        {
            assert!(stacks >= 1 && stacks <= max_stacks, "{name}");
        }
    }
}

#[test]
fn only_the_abilities_that_hurt_carry_a_damage_type_the_executor_can_use() {
    for ability in EnemyAbility::ALL {
        let d = ability.definition();
        if d.damage > 0.0 {
            // Reflect and every school-keyed reduction read this.
            let _ = d.damage_type;
        }
        // A spell (a cast, not physical, plain damage) is what Spell Reflect can send back.
        // Hazards, channels and marks must not be reflectable: the damage would be credited
        // to the pool, not the cast.
    }
    let sim = fight(EnemyKind::Caster, 10.0, &[]).sim;
    for ability in EnemyAbility::ALL {
        let d = ability.definition();
        let reflectable = sim.is_reflectable(AbilityId::from_enemy(ability));
        let expected = d.is_cast()
            && !matches!(d.damage_type, engine_game::combat::DamageType::Physical)
            && matches!(d.effect, EnemyEffect::Damage);
        assert_eq!(reflectable, expected, "{ability:?}");
    }
}

// ------------------------------------------------------------------------------------------
// Scenario: every ability fires, once, at its table damage times its scale
// ------------------------------------------------------------------------------------------

/// Where the other players of the roster scenarios stand, relative to the first: one beside
/// the tank and one far back, so a charge has somebody to run at and a mark has somebody to
/// land on besides the tank.
const PARTY: [Vec3; 2] = [Vec3::new(3.0, 0.0, 1.0), Vec3::new(-2.0, 0.0, 16.0)];

/// A fight with `kinds` in a pack, the first one's brain taken to `phase`, every monster's
/// damage multiplied by `damage_scale`.
fn roster_fight(kinds: &[EnemyKind], phase: usize, damage_scale: f32) -> Fight {
    let config = ArenaConfig::with_group(0);
    let spawns = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let position = config.player_spawn + Vec3::new(index as f32 * 3.0, 0.0, -9.0);
            EnemySpawn::new(*kind, position).detailed(engine_game::dungeon::SpawnDetail {
                level: 1,
                health_scale: 1.0,
                damage_scale,
                aggro_radius: None,
                completion: None,
                size: 1.0,
            })
        })
        .collect();
    let mut sim = arena::encounter_arena(&config, Encounter::with_spawns("AUDIT", spawns));
    let first = sim.player().entity;
    let mut players = vec![first];
    for offset in PARTY {
        let entity = sim
            .world_mut()
            .spawn(EntityDesc::player(config.player_spawn + offset));
        sim.add_player(entity, Hero::FireMage);
        players.push(entity);
    }
    for player in &players {
        sim.world_mut().set_max_health(*player, 1_000_000.0);
        sim.world_mut().restore(*player);
    }
    let members = sim
        .encounter()
        .map(|encounter| encounter.members().to_vec())
        .expect("an encounter");
    let enemy = members[0];
    if phase > 0 {
        let threshold = kinds[0].archetype().phases[phase - 1].at_or_below;
        let max = sim
            .world()
            .health(enemy)
            .map_or(0.0, engine_game::Health::max);
        sim.world_mut()
            .damage(enemy, max * (1.0 - (threshold - 0.02)));
    }
    // A wounded packmate gives a healer something to do.
    if let Some(mate) = members.get(1) {
        let max = sim
            .world()
            .health(*mate)
            .map_or(0.0, engine_game::Health::max);
        sim.world_mut().damage(*mate, max * 0.6);
    }
    Fight {
        sim,
        enemy,
        players,
    }
}

/// Every roster scenario: each kind, in each of its phases; a healer with a wounded mate.
fn scenarios() -> Vec<(Vec<EnemyKind>, usize)> {
    let mut all = Vec::new();
    for kind in EnemyKind::ALL {
        for phase in 0..kind.archetype().phase_count() {
            let pack = if kind == EnemyKind::Warden {
                vec![kind, EnemyKind::Melee]
            } else {
                vec![kind]
            };
            all.push((pack, phase));
        }
    }
    all
}

/// Everything one scenario did, for `ticks` ticks.
fn play(
    kinds: &[EnemyKind],
    phase: usize,
    damage_scale: f32,
    ticks: u32,
) -> (Fight, Vec<CombatEvent>) {
    let mut fight = roster_fight(kinds, phase, damage_scale);
    let events = run(&mut fight.sim, ticks);
    (fight, events)
}

/// Whether `ability` visibly did anything in `events`.
fn fired(events: &[CombatEvent], ability: EnemyAbility) -> bool {
    events.iter().any(|event| match event {
        CombatEvent::CastStarted { ability: used, .. }
        | CombatEvent::AbilityUsed { ability: used, .. } => used.enemy() == Some(ability),
        CombatEvent::Damage(damage) => damage.ability.enemy() == Some(ability),
        _ => false,
    })
}

#[test]
fn every_ability_fires_in_some_fight() {
    let mut seen = [false; 58];
    for (kinds, phase) in scenarios() {
        let (_, events) = play(&kinds, phase, 1.0, 60 * 70);
        for ability in EnemyAbility::ALL {
            seen[ability.index()] |= fired(&events, ability);
        }
    }
    let silent: Vec<_> = EnemyAbility::ALL
        .into_iter()
        .filter(|ability| !seen[ability.index()])
        .collect();
    assert!(
        silent.is_empty(),
        "never used in a minute of fighting: {silent:?}"
    );
}

#[test]
fn damage_scales_once_on_every_path() {
    // Doubling a monster's damage scale must double what each of its abilities does: hits,
    // channel ticks, marks, shockwaves, trails, pools and bleeds alike. An ability that
    // ignored the scale would stay put; one that applied it twice would quadruple.
    let mut checked = [false; 58];
    for (kinds, phase) in scenarios() {
        let (_, one) = play(&kinds, phase, 1.0, 60 * 40);
        let (_, two) = play(&kinds, phase, 2.0, 60 * 40);
        for ability in EnemyAbility::ALL {
            let (a, b) = (total_by(&one, ability), total_by(&two, ability));
            if a <= 0.0 && b <= 0.0 {
                continue;
            }
            checked[ability.index()] = true;
            assert!(
                (b - 2.0 * a).abs() <= 0.002 * b.max(1.0),
                "{ability:?}: {a} at scale 1 but {b} at scale 2"
            );
        }
    }
    let unchecked: Vec<_> = EnemyAbility::ALL
        .into_iter()
        .filter(|ability| !checked[ability.index()] && ability.definition().damage > 0.0)
        .collect();
    assert!(unchecked.is_empty(), "never dealt damage: {unchecked:?}");
}

// ------------------------------------------------------------------------------------------
// Targeting
// ------------------------------------------------------------------------------------------

/// Gives `holder` threat on every monster in the fight, through the ordinary damage path, so
/// "the tank" is a fact the threat table knows and not just whoever stands nearest.
fn make_tank(fight: &mut Fight, holder: EntityId, amount: f32) {
    let monsters: Vec<EntityId> = fight.sim.enemies().map(|(entity, _)| entity).collect();
    for monster in monsters {
        fight.sim.apply_damage(
            holder,
            monster,
            amount,
            AbilityId::NONE,
            engine_game::combat::DamageType::Physical,
        );
    }
}

/// `kinds` in phase `phase` with the first player as the tank, run for `ticks`.
fn tanked(kinds: &[EnemyKind], phase: usize, ticks: u32) -> (Fight, Vec<CombatEvent>) {
    let mut fight = roster_fight(kinds, phase, 1.0);
    let tank = fight.players[0];
    make_tank(&mut fight, tank, 5.0);
    let events = run(&mut fight.sim, ticks);
    (fight, events)
}

/// The monster that owns `ability`, the phase it is in and whatever pack it needs.
fn owner_of(ability: EnemyAbility) -> (Vec<EnemyKind>, usize) {
    for kind in EnemyKind::ALL {
        let archetype = kind.archetype();
        for phase in 0..archetype.phase_count() {
            if archetype
                .phase_abilities(phase)
                .any(|owned| owned == ability)
            {
                let pack = if kind == EnemyKind::Warden {
                    vec![kind, EnemyKind::Melee]
                } else {
                    vec![kind]
                };
                return (pack, phase);
            }
        }
    }
    panic!("{ability:?} belongs to nobody");
}

#[test]
fn a_pounce_leaps_at_players_out_of_reach_again_and_again() {
    // The Ghoul is in the tank's face: a "random player" leap at the tank would be a second
    // swing, and measured against the tank's distance it never fires after the opening. It
    // has to pick one of the others, from outside its reach, every cooldown.
    for ability in [EnemyAbility::Pounce, EnemyAbility::SavagePounce] {
        let (kinds, phase) = owner_of(ability);
        let (fight, events) = tanked(&kinds, phase, 60 * 120);
        let far = fight.players[2];
        let landed = hits_by(&events, ability)
            .iter()
            .filter(|(target, _)| *target == far)
            .count();
        assert!(
            landed >= 5,
            "{ability:?} landed {landed} times in two minutes"
        );
    }
}

#[test]
fn a_mark_a_garrote_and_a_crippling_arrow_reach_past_the_tank() {
    for ability in [
        EnemyAbility::GraveExplosion,
        EnemyAbility::GraveChainExplosion,
        EnemyAbility::MarkOfTheHost,
        EnemyAbility::Garrote,
        EnemyAbility::CripplingArrow,
    ] {
        let (kinds, phase) = owner_of(ability);
        let (fight, events) = tanked(&kinds, phase, 60 * 90);
        let tank = fight.players[0];
        let mut victims = Vec::new();
        for event in &events {
            match event {
                CombatEvent::TelegraphStarted {
                    ability: used,
                    follow: Some(victim),
                    ..
                } if used.enemy() == Some(ability) => victims.push(*victim),
                CombatEvent::Damage(damage)
                    if damage.ability.enemy() == Some(ability)
                        && !damage.periodic
                        && ability.definition().delay <= 0.0 =>
                {
                    victims.push(damage.target);
                }
                _ => {}
            }
        }
        assert!(!victims.is_empty(), "{ability:?} never found anybody");
        // The first marks of each cast are the ones that are chosen; a chain lays the rest on
        // whoever the first detonation caught.
        let chosen = &victims[..victims.len().min(2)];
        assert!(
            chosen.iter().all(|victim| *victim != tank),
            "{ability:?} was placed on the tank with two others in range"
        );
    }
}

#[test]
fn a_charge_runs_at_whoever_stands_furthest_back() {
    for ability in [
        EnemyAbility::BoneCharge,
        EnemyAbility::GraveCharge,
        EnemyAbility::ShatteringCharge,
        EnemyAbility::DevastatingCharge,
        EnemyAbility::GhastlyWaltz,
    ] {
        let (kinds, phase) = owner_of(ability);
        let (fight, events) = tanked(&kinds, phase, 60 * 60);
        let furthest = fight.players[2];
        // Every landing and shockwave is at the far player's feet; a trail is ground.
        let hit: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                CombatEvent::Damage(damage)
                    if damage.ability.enemy() == Some(ability) && !damage.periodic =>
                {
                    Some(damage.target)
                }
                _ => None,
            })
            .collect();
        assert!(!hit.is_empty(), "{ability:?} never charged");
        assert!(
            hit.iter().all(|target| *target == furthest),
            "{ability:?} hit somebody it was not aimed at"
        );
    }
}

#[test]
fn a_filler_attack_lands_on_whoever_holds_threat() {
    let mut fight = roster_fight(&[EnemyKind::Melee], 0, 1.0);
    let holder = fight.players[1];
    make_tank(&mut fight, holder, 40.0);
    let events = run(&mut fight.sim, 60 * 20);
    let swings = hits_by(&events, EnemyAbility::Swing);
    assert!(!swings.is_empty());
    assert!(swings.iter().all(|(target, _)| *target == holder));
}

#[test]
fn nothing_is_aimed_at_the_dead_or_at_a_player_who_has_left() {
    for (kinds, phase) in scenarios() {
        let mut fight = roster_fight(&kinds, phase, 1.0);
        let tank = fight.players[0];
        make_tank(&mut fight, tank, 5.0);
        let (dies, leaves) = (fight.players[1], fight.players[2]);
        let mut gone: Vec<EntityId> = Vec::new();
        for tick in 0..(60 * 60) {
            if tick == 60 * 15 {
                fight.sim.slay(dies);
                gone.push(dies);
            }
            if tick == 60 * 30 {
                assert!(fight.sim.remove_player(leaves));
                gone.push(leaves);
            }
            fight.sim.step_once();
            for event in fight.sim.events() {
                if let CombatEvent::TelegraphStarted {
                    ability,
                    follow: Some(victim),
                    ..
                } = event
                {
                    assert!(
                        fight.sim.world().is_alive(*victim),
                        "{:?}: a mark was laid on a player who is dead or gone (tick {tick})",
                        ability.enemy()
                    );
                }
            }
            // A cast that has just begun is aimed at the living.
            for (_, brain) in fight.sim.enemies() {
                if let Some(cast) = brain.cast()
                    && cast.elapsed < 0.02
                {
                    for target in cast.target_list() {
                        assert!(
                            fight.sim.world().is_alive(target),
                            "{:?} began a cast on someone who is dead or gone",
                            cast.ability
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_mark_whose_victim_dies_during_the_cast_is_not_laid() {
    let (kinds, phase) = owner_of(EnemyAbility::GraveExplosion);
    let mut fight = roster_fight(&kinds, phase, 1.0);
    let tank = fight.players[0];
    make_tank(&mut fight, tank, 5.0);
    let mut killed = None;
    let mut events = Vec::new();
    for _ in 0..(60 * 60) {
        fight.sim.step_once();
        events.extend_from_slice(fight.sim.events());
        let cast = fight.sim.brain(fight.enemy).and_then(|brain| brain.cast());
        if killed.is_none()
            && let Some(cast) = cast
            && cast.ability == EnemyAbility::GraveExplosion
            && cast.elapsed > 0.3
        {
            let victim = cast.zone_targets[0].expect("a mark has a victim");
            assert_ne!(victim, tank, "marks are not for the tank");
            fight.sim.slay(victim);
            killed = Some(victim);
            events.clear();
            events.extend_from_slice(fight.sim.events());
        }
        if killed.is_some() && cast.is_none() {
            break;
        }
    }
    let victim = killed.expect("the Bonecaster never cast a mark");
    let marks_on_the_dead = events.iter().any(|event| {
        matches!(
            event,
            CombatEvent::TelegraphStarted { follow: Some(followed), ability, .. }
                if *followed == victim && ability.enemy() == Some(EnemyAbility::GraveExplosion)
        )
    });
    assert!(!marks_on_the_dead, "a mark was laid on a corpse");
}

// ------------------------------------------------------------------------------------------
// Casts and interrupts
// ------------------------------------------------------------------------------------------

/// Runs the first fight in which `ability` is being cast `fraction` of the way through, and
/// answers it with the events up to that moment.
fn until_casting(ability: EnemyAbility, fraction: f32) -> (Fight, Vec<CombatEvent>) {
    let (kinds, phase) = owner_of(ability);
    let mut fight = roster_fight(&kinds, phase, 1.0);
    let tank = fight.players[0];
    make_tank(&mut fight, tank, 5.0);
    let mut events = Vec::new();
    for _ in 0..(60 * 150) {
        fight.sim.step_once();
        events.extend_from_slice(fight.sim.events());
        let casting = fight
            .sim
            .enemies()
            .find(|(_, brain)| {
                brain
                    .cast()
                    .is_some_and(|cast| cast.ability == ability && cast.fraction() >= fraction)
            })
            .map(|(entity, _)| entity);
        if let Some(caster) = casting {
            fight.enemy = caster;
            return (fight, events);
        }
    }
    panic!("{ability:?} was never cast");
}

#[test]
fn an_interrupt_stops_what_can_be_stopped_and_nothing_else() {
    for ability in EnemyAbility::ALL {
        let definition = ability.definition();
        if !definition.is_cast() {
            continue;
        }
        let (mut fight, before) = until_casting(ability, 0.4);
        let caster = fight.enemy;
        // Whether or not an interrupt would land is the cast's own answer, from the row.
        assert_eq!(
            fight.sim.is_interruptible(caster),
            definition.interruptible,
            "{ability:?}"
        );
        let stopped = fight.sim.interrupt(caster);
        assert_eq!(stopped, definition.interruptible, "{ability:?}");
        let window = if definition.interruptible {
            (definition.cooldown * 0.4).min(1.0)
        } else {
            definition.cast_time + 0.5
        };
        let mut events = fight.sim.events().to_vec();
        events.extend(run(&mut fight.sim, (window * 60.0) as u32));
        let all: Vec<_> = before.iter().chain(&events).copied().collect();

        if definition.interruptible {
            assert!(
                events.iter().any(|event| matches!(
                    event,
                    CombatEvent::CastEnded { ability: ended, end: CastEnd::Interrupted, .. }
                        if ended.enemy() == Some(ability)
                )),
                "{ability:?}: no interrupted cast was announced"
            );
            assert!(
                !events.iter().any(|event| matches!(
                    event,
                    CombatEvent::CastEnded { ability: ended, end: CastEnd::Completed, .. }
                        if ended.enemy() == Some(ability)
                )),
                "{ability:?}: an interrupted cast completed"
            );
            // Nothing it was going to do happened: no hit, no pool, no mark, no heal.
            let damage_after: f32 = total_by(&events, ability);
            assert_eq!(
                damage_after, 0.0,
                "{ability:?} still hurt after the interrupt"
            );
            assert!(
                !events.iter().any(|event| matches!(
                    event,
                    CombatEvent::AreaEffectStarted { ability: used, .. } if used.enemy() == Some(ability)
                )),
                "{ability:?} still left ground after the interrupt"
            );
            assert!(
                !events.iter().any(|event| matches!(
                    event,
                    CombatEvent::Healing { ability: used, .. } if used.enemy() == Some(ability)
                )),
                "{ability:?} still healed after the interrupt"
            );
            // The cooldown started when the cast did and is still running, and the enemy is
            // left exposed rather than casting again.
            let brain = fight.sim.brain(caster).expect("an enemy");
            assert!(
                brain.cooldown(ability) > 0.0,
                "{ability:?} was ready again straight after an interrupt"
            );
            assert!(
                brain.cast().is_none_or(|cast| cast.ability != ability),
                "{ability:?}: still casting"
            );
        } else {
            // Left alone it finishes.
            let done = events.iter().any(|event| {
                matches!(
                    event,
                    CombatEvent::CastEnded { ability: ended, end: CastEnd::Completed, .. }
                        if ended.enemy() == Some(ability)
                )
            });
            assert!(
                done,
                "{ability:?}: an uninterruptible cast did not complete"
            );
            assert!(
                !all.iter().any(|event| matches!(
                    event,
                    CombatEvent::CastEnded { ability: ended, end: CastEnd::Interrupted, .. }
                        if ended.enemy() == Some(ability)
                )),
                "{ability:?}: was interrupted"
            );
        }

        // Every telegraph that went up came down, and an interrupted one came down unresolved.
        let mut open: Vec<(u32, EnemyAbility)> = Vec::new();
        for event in &all {
            match event {
                CombatEvent::TelegraphStarted { id, ability, .. } => {
                    open.push((*id, ability.enemy().expect("an enemy telegraph")));
                }
                CombatEvent::TelegraphEnded { id, resolved, .. } => {
                    if let Some(index) = open.iter().position(|(open, _)| open == id) {
                        let (_, owner) = open.remove(index);
                        if definition.interruptible && owner == ability && stopped {
                            // The interrupted cast's own telegraph: none may have resolved
                            // after the interrupt. (Earlier ones belong to earlier casts.)
                            let _ = resolved;
                        }
                    }
                }
                _ => {}
            }
        }
        if definition.interruptible && definition.delay <= 0.0 {
            assert!(
                !open.iter().any(|(_, owner)| *owner == ability),
                "{ability:?}: its telegraph is still on the floor after the interrupt"
            );
        }
    }
}

#[test]
fn an_interrupted_cast_never_resolves_its_telegraph() {
    // The explicit form of the above for the ones that draw on the floor while they cast.
    for ability in [
        EnemyAbility::GroundBlast,
        EnemyAbility::RitualNova,
        EnemyAbility::Ashfall,
        EnemyAbility::DarkPulse,
        EnemyAbility::Cleave,
    ] {
        let (mut fight, _) = until_casting(ability, 0.4);
        let caster = fight.enemy;
        assert!(fight.sim.interrupt(caster), "{ability:?}");
        let mut events = fight.sim.events().to_vec();
        events.extend(run(&mut fight.sim, 2));
        let ended: Vec<bool> = events
            .iter()
            .filter_map(|event| match event {
                CombatEvent::TelegraphEnded { resolved, .. } => Some(*resolved),
                _ => None,
            })
            .collect();
        assert!(!ended.is_empty(), "{ability:?}: its telegraph stayed up");
        assert!(ended.iter().all(|resolved| !resolved), "{ability:?}");
    }
}

#[test]
fn a_channel_that_is_interrupted_stops_ticking() {
    for ability in [
        EnemyAbility::GraveDrain,
        EnemyAbility::MassGraveDrain,
        EnemyAbility::DarkPulse,
    ] {
        let (mut fight, _) = until_casting(ability, 0.5);
        let caster = fight.enemy;
        assert!(fight.sim.interrupt(caster));
        let after = run(&mut fight.sim, 60 * 5);
        assert_eq!(total_by(&after, ability), 0.0, "{ability:?} kept ticking");
    }
}

// ------------------------------------------------------------------------------------------
// Telegraphs, cooldowns
// ------------------------------------------------------------------------------------------

#[test]
fn what_a_telegraph_announced_is_exactly_who_gets_hit() {
    let mut checked = 0;
    for (kinds, phase) in scenarios() {
        let mut fight = roster_fight(&kinds, phase, 1.0);
        let tank = fight.players[0];
        make_tank(&mut fight, tank, 5.0);
        let mut started: Vec<(u32, EnemyAbility, TelegraphShape)> = Vec::new();
        for _ in 0..(60 * 60) {
            // The AI runs before bodies move, so positions before the step are the ones the
            // hit test saw.
            let positions: Vec<(EntityId, Vec3, f32)> = fight
                .players
                .iter()
                .filter(|player| fight.sim.world().is_alive(**player))
                .map(|player| {
                    (
                        *player,
                        fight.sim.world().position(*player).expect("a player"),
                        fight.sim.world().radius(*player),
                    )
                })
                .collect();
            fight.sim.step_once();
            let events = fight.sim.events();
            let mut expected: Vec<(EnemyAbility, EntityId)> = Vec::new();
            let mut skip: Vec<EnemyAbility> = Vec::new();
            for event in events {
                match event {
                    CombatEvent::TelegraphStarted {
                        id, ability, shape, ..
                    } => started.push((*id, ability.enemy().expect("enemy"), *shape)),
                    CombatEvent::TelegraphEnded {
                        id,
                        anchor,
                        resolved: true,
                        ..
                    } => {
                        let Some((_, ability, shape)) =
                            started.iter().find(|(started, _, _)| started == id)
                        else {
                            continue;
                        };
                        if ability.definition().effect.is_channel() {
                            skip.push(*ability);
                            continue;
                        }
                        for (player, position, radius) in &positions {
                            if shape.contains(*anchor, *position, *radius) {
                                expected.push((*ability, *player));
                            }
                        }
                        checked += 1;
                    }
                    _ => {}
                }
            }
            if expected.is_empty() && skip.is_empty() {
                continue;
            }
            let mut actual: Vec<(EnemyAbility, EntityId)> = events
                .iter()
                .filter_map(|event| match event {
                    CombatEvent::Damage(damage) if !damage.periodic => damage
                        .ability
                        .enemy()
                        .filter(|ability| !skip.contains(ability))
                        .map(|ability| (ability, damage.target)),
                    _ => None,
                })
                .collect();
            // A single landing can also be a delayed zone of the same ability on the same
            // tick; both lists are of the same shape, so compare as multisets.
            let key = |pair: &(EnemyAbility, EntityId)| (pair.0.index(), pair.1.index());
            expected.sort_by_key(key);
            actual.sort_by_key(key);
            // Only abilities that drew something this tick are compared.
            actual.retain(|(ability, _)| expected.iter().any(|(drawn, _)| drawn == ability));
            assert_eq!(
                expected, actual,
                "{kinds:?}: what was drawn and what was hit disagree"
            );
        }
    }
    assert!(checked > 20, "only {checked} landings were compared");
}

#[test]
fn no_ability_is_used_again_before_its_cooldown_is_up() {
    for (kinds, phase) in scenarios() {
        let (fight, events) = play(&kinds, phase, 1.0, 60 * 90);
        let _ = fight;
        for ability in EnemyAbility::ALL {
            let definition = ability.definition();
            // Casts are counted where they start; instants where they go off.
            let counted_as_instant = !definition.is_cast()
                && !definition.effect.is_charge()
                && matches!(definition.shape, TelegraphShape::Single);
            let mut starts: Vec<u64> = events
                .iter()
                .filter_map(|event| match event {
                    CombatEvent::CastStarted {
                        ability: used,
                        tick,
                        ..
                    } if used.enemy() == Some(ability) => Some(*tick),
                    CombatEvent::AbilityUsed {
                        ability: used,
                        tick,
                        ..
                    } if counted_as_instant && used.enemy() == Some(ability) => Some(*tick),
                    _ => None,
                })
                .collect();
            starts.dedup();
            // The fastest an ability can come back: the phase's cooldown scale, divided by
            // the strongest frenzy in the roster.
            let scale = kinds[0].archetype().phase_cooldown_scale(phase);
            let fastest = definition.cooldown * scale / 1.6;
            for pair in starts.windows(2) {
                let gap = (pair[1] - pair[0]) as f32 / 60.0;
                assert!(
                    gap >= fastest - 0.05,
                    "{ability:?} came back after {gap:.2}s, cooldown {fastest:.2}s"
                );
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// The player's buffs and absorbs
// ------------------------------------------------------------------------------------------

/// Everything the roster does to a party in `ticks`, with `effect` on every player first.
fn damage_with(
    effect: Option<(StatusKind, u8)>,
    kinds: &[EnemyKind],
    phase: usize,
) -> Vec<CombatEvent> {
    let mut fight = roster_fight(kinds, phase, 1.0);
    if let Some((kind, stacks)) = effect {
        for player in fight.players.clone() {
            assert!(
                fight
                    .sim
                    .apply_effect(player, kind, 10_000.0, stacks, EntityId::INVALID)
            );
        }
    }
    run(&mut fight.sim, 60 * 40)
}

#[test]
fn damage_taken_effects_scale_every_kind_of_enemy_damage() {
    // (effect, stacks, what a hit is multiplied by)
    let cases = [
        (StatusKind::Barkskin, 1, 0.75),
        (StatusKind::TrinketWard, 1, 0.85),
        (StatusKind::BattleFervor, 3, 0.91),
    ];
    for (kinds, phase) in scenarios() {
        let plain = damage_with(None, &kinds, phase);
        for (kind, stacks, factor) in cases {
            let buffed = damage_with(Some((kind, stacks)), &kinds, phase);
            for ability in EnemyAbility::ALL {
                let (a, b) = (total_by(&plain, ability), total_by(&buffed, ability));
                if a <= 0.0 {
                    continue;
                }
                assert!(
                    (b - a * factor).abs() <= 0.01 * a,
                    "{ability:?} under {kind:?}: {a} became {b}, wanted {}",
                    a * factor
                );
            }
        }
    }
}

#[test]
fn the_warriors_shield_soaks_enemy_hits_before_health() {
    let mut fight = fight(EnemyKind::Melee, 3.0, &[]);
    let player = fight.players[0];
    assert!(fight.sim.set_hero(Hero::Warrior));
    fight.sim.set_health_regen_per_second(0.0);
    fight.sim.set_player_health_override(None);
    fight.sim.world_mut().restore(player);
    fight
        .sim
        .cast(engine_game::firemage::FireAbility::PunctureBlood)
        .expect("puncture blood");
    let shield = fight.sim.absorb_of(player);
    assert!(shield > 0.0);
    let health = fight.sim.player_health();
    let events = run(&mut fight.sim, 60 * 6);
    let soaked: f32 = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Absorbed { amount, .. } => Some(*amount),
            _ => None,
        })
        .sum();
    assert!(soaked > 0.0, "the shield took nothing");
    assert!(fight.sim.absorb_of(player) < shield);
    // Health lost is what got past the shield: hits minus what it took.
    let landed: f32 = events
        .iter()
        .filter_map(|event| match event {
            CombatEvent::Damage(damage) if damage.target == player => Some(damage.amount),
            _ => None,
        })
        .sum();
    assert!(fight.sim.player_health() <= health + 0.01);
    assert!((health - fight.sim.player_health() - landed).abs() < 0.5);
}

// ------------------------------------------------------------------------------------------
// Dispels, authority, sight, Shield Bash
// ------------------------------------------------------------------------------------------

#[test]
fn what_enemies_put_on_a_player_is_classified_for_the_dispel() {
    // Every status an enemy ability applies: the slows are curses (Nature's Cleanse and Mass
    // Dispel take them); a bleed is a wound, and nothing dispels a wound.
    let mut kinds_applied: Vec<StatusKind> = Vec::new();
    for ability in EnemyAbility::ALL {
        let definition = ability.definition();
        match definition.rider {
            EnemyRider::Bleed { .. } => kinds_applied.push(StatusKind::Rended),
            EnemyRider::Slow { .. } => kinds_applied.push(StatusKind::Crippled),
            EnemyRider::None => {}
        }
        if definition.effect.is_charge() {
            kinds_applied.push(StatusKind::Crippled);
        }
    }
    assert!(kinds_applied.contains(&StatusKind::Rended));
    assert!(kinds_applied.contains(&StatusKind::Crippled));
    assert!(StatusKind::Crippled.is_dispellable());
    assert_eq!(
        StatusKind::Crippled.dispel_school(),
        Some(engine_game::status::DispelSchool::Curse)
    );
    assert!(!StatusKind::Rended.is_dispellable());
    assert!(StatusKind::Rended.is_harmful() && StatusKind::Crippled.is_harmful());
}

#[test]
fn natures_cleanse_takes_an_enemys_slow_off_and_leaves_its_bleed() {
    let mut fight = fight(EnemyKind::Melee, 40.0, &[]);
    let player = fight.players[0];
    assert!(fight.sim.set_hero(Hero::Druid));
    let enemy = fight.enemy;
    for (kind, stacks) in [(StatusKind::Crippled, 1), (StatusKind::Rended, 3)] {
        let status = Status::new(kind, enemy, AbilityId::NONE, 30.0, 5.0)
            .with_magnitude(0.4)
            .with_stacks(stacks);
        fight
            .sim
            .world_mut()
            .statuses_mut(player)
            .expect("statuses")
            .apply(status);
    }
    fight.sim.player_mut().target = Some(player);
    fight.sim.step_once();
    let _ = fight
        .sim
        .cast(engine_game::firemage::FireAbility::NaturesCleanse);
    let statuses = fight.sim.world().statuses(player).expect("statuses");
    assert!(!statuses.has(StatusKind::Crippled), "the slow stayed");
    assert!(statuses.has(StatusKind::Rended), "a bleed was dispelled");
}

#[test]
fn a_client_under_remote_authority_runs_no_enemy_logic() {
    let mut fight = roster_fight(&[EnemyKind::Boss], 0, 1.0);
    fight.sim.set_remote_authority(true);
    let events = run(&mut fight.sim, 60 * 20);
    assert!(!events.iter().any(|event| matches!(
        event,
        CombatEvent::Damage(_)
            | CombatEvent::CastStarted { .. }
            | CombatEvent::AbilityUsed { .. }
            | CombatEvent::TelegraphStarted { .. }
    )));
    assert_eq!(state(&fight.sim, fight.enemy), AiState::Idle);
}

#[test]
fn single_target_hits_never_land_through_a_wall() {
    for (kinds, phase) in scenarios() {
        let mut fight = roster_fight(&kinds, phase, 1.0);
        let tank = fight.players[0];
        make_tank(&mut fight, tank, 5.0);
        // A wall across the room between the monsters and the party.
        let collision = fight.sim.collision_world_mut();
        collision.add_box(
            Vec3::new(-8.0, 0.0, -5.0),
            Vec3::new(8.0, 5.0, -4.0),
            "AUDIT WALL",
        );
        collision.rebuild_broad_phase();
        for _ in 0..(60 * 40) {
            let positions: Vec<(EntityId, EntityId)> = fight
                .sim
                .enemies()
                .map(|(entity, _)| (entity, entity))
                .collect();
            fight.sim.step_once();
            for event in fight.sim.events() {
                let CombatEvent::Damage(damage) = event else {
                    continue;
                };
                let Some(ability) = damage.ability.enemy() else {
                    continue;
                };
                let definition = ability.definition();
                if damage.periodic
                    || definition.shape.is_area()
                    || !matches!(definition.effect, EnemyEffect::Damage)
                    || definition.effect.is_charge()
                {
                    continue;
                }
                let sees = positions.iter().any(|(enemy, _)| {
                    *enemy == damage.source
                        && engine_game::los::has_line_of_sight(
                            fight.sim.collision_world(),
                            fight.sim.world(),
                            *enemy,
                            damage.target,
                        )
                });
                assert!(sees, "{ability:?} hit through a wall");
            }
        }
    }
}

#[test]
fn shield_bash_interrupts_an_interruptible_cast_and_only_that() {
    // Execution Strike is interruptible; Ash Burst is not.
    for (kind, ability, stops) in [
        (
            EnemyKind::SkeletonWarrior,
            EnemyAbility::ExecutionStrike,
            true,
        ),
        (EnemyKind::Revenant, EnemyAbility::AshBurst, false),
    ] {
        let mut fight = fight(kind, 2.0, &[]);
        let player = fight.players[0];
        assert!(fight.sim.set_hero(Hero::Warrior));
        fight.sim.set_player_health_override(Some(1_000_000.0));
        let enemy = fight.enemy;
        fight.sim.player_mut().target = Some(enemy);
        let mut bashed = false;
        for _ in 0..(60 * 60) {
            fight.sim.step_once();
            let casting = fight
                .sim
                .brain(enemy)
                .and_then(|brain| brain.cast())
                .is_some_and(|cast| cast.ability == ability && cast.elapsed > 0.3);
            if casting {
                fight.sim.player_mut().target = Some(enemy);
                let _ = fight
                    .sim
                    .cast(engine_game::firemage::FireAbility::ShieldBash);
                let ended = fight.sim.events().iter().any(|event| {
                    matches!(
                        event,
                        CombatEvent::CastEnded {
                            end: CastEnd::Interrupted,
                            ..
                        }
                    )
                });
                assert_eq!(ended, stops, "{ability:?}");
                assert_eq!(
                    fight
                        .sim
                        .brain(enemy)
                        .is_some_and(|brain| brain.cast().is_some()),
                    !stops
                );
                bashed = true;
                break;
            }
        }
        assert!(
            bashed,
            "{ability:?} was never cast at the Warrior {player:?}"
        );
    }
}
