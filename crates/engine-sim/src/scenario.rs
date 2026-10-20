//! One fight: the targets, how long it runs, and how imperfect it is allowed to be.
//!
//! ```text
//! ScenarioConfig -> Encounter::run(build) -> RunResult
//! ```
//!
//! The encounter builds a real [`Simulation`] and steps it. Nothing here re-implements a
//! cooldown, a burn or a point of damage: the rotation AI presses buttons and the game
//! decides what happens, which is why a number this crate prints is a number the game will
//! produce.
//!
//! ## Targets are not dummies
//!
//! They have finite health and they die; when one does, its burn dies with it, area
//! abilities start hitting fewer enemies and casts against it are wasted. They also *move*:
//! every target has a chance to wander each interval, and whether that takes it out of a
//! Firestorm or a Flame Cone is decided by the game's own geometry rather than by a fudge
//! factor applied to the damage afterwards.

use engine_core::math::Vec3;
use engine_game::arena::{self, ArenaConfig};
use engine_game::combat::Faction;
use engine_game::entity::EntityId;
use engine_game::firemage::Rng;
use engine_game::sim::{FireCounters, SimConfig, Simulation};
use engine_game::world::{EntityDesc, EntityKind};

use engine_game::character::stat::{Stat, health_from_stamina};
use engine_game::hero::Hero;

use crate::ai::Rotation;
use crate::build::ClassBuild;
use crate::gear::Worn;
use crate::metrics::{Metrics, Recorder};

/// How a fight is set up.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioConfig {
    /// How many enemies stand in the pack.
    pub targets: usize,
    /// How much health each of them holds.
    pub target_health: f32,
    /// How long the fight runs, in seconds.
    pub duration: f32,
    /// Metres between neighbouring targets.
    pub spacing: f32,
    /// How far the pack stands from the mage.
    pub distance: f32,
    /// Seconds between chances for a target to move.
    pub move_interval: f32,
    /// Chance a target wanders when the interval comes round.
    pub wander_chance: f32,
    /// How far it goes when it does, in metres.
    pub wander_distance: f32,
    /// Chance it comes back towards where it started instead.
    pub return_chance: f32,
}

impl Default for ScenarioConfig {
    fn default() -> Self {
        Self {
            targets: 1,
            // Enough that a single target survives a sixty-second fight against the opening
            // build, and low enough that a pack of fifteen does not.
            target_health: 60_000.0,
            duration: 60.0,
            spacing: 3.0,
            distance: 12.0,
            move_interval: 2.0,
            wander_chance: 0.25,
            wander_distance: 4.0,
            return_chance: 0.5,
        }
    }
}

#[allow(
    dead_code,
    reason = "configuration helpers the report does not use are exercised by the tests"
)]
impl ScenarioConfig {
    /// The same scenario with a different number of targets.
    #[must_use]
    pub fn with_targets(&self, targets: usize) -> Self {
        Self {
            targets,
            ..self.clone()
        }
    }

    /// The same scenario over a different length of fight.
    #[must_use]
    pub fn with_duration(&self, duration: f32) -> Self {
        Self {
            duration,
            ..self.clone()
        }
    }

    /// A scenario with no movement at all, for a test that needs a still target.
    #[must_use]
    pub fn stationary(&self) -> Self {
        Self {
            wander_chance: 0.0,
            ..self.clone()
        }
    }
}

/// What one fight produced.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// Total damage dealt.
    pub damage: f64,
    /// Damage per second over the fight's whole length.
    pub dps: f64,
    /// Damage that fell on health a target did not have.
    pub overkill: f64,
    /// How many targets died.
    pub deaths: u32,
    /// Damage by ability, indexed by [`engine_game::ability::AbilityId`] slot.
    pub by_ability: [f64; engine_game::ability::ABILITY_SLOTS],
    /// Everything the class counted about itself.
    pub counters: FireCounters,
    /// Seconds the fight ran.
    pub seconds: f64,
    /// The player's maximum health at the end of the fight: Stamina from gear, through the
    /// game's own curve.
    pub max_health: f64,
    /// Casts, resources, cooldown usage and aura uptimes.
    pub metrics: Metrics,
}

impl Default for RunResult {
    /// Written out rather than derived: the per-ability array is longer than the thirty-two
    /// `Default` covers, and every field of it is zero.
    fn default() -> Self {
        Self {
            damage: 0.0,
            dps: 0.0,
            overkill: 0.0,
            deaths: 0,
            by_ability: [0.0; engine_game::ability::ABILITY_SLOTS],
            counters: FireCounters::default(),
            seconds: 0.0,
            max_health: 0.0,
            metrics: Metrics::default(),
        }
    }
}

impl RunResult {
    /// Adds another run into this one, for averaging afterwards.
    pub fn accumulate(&mut self, other: &RunResult) {
        self.damage += other.damage;
        self.dps += other.dps;
        self.overkill += other.overkill;
        self.deaths += other.deaths;
        self.seconds += other.seconds;
        self.max_health += other.max_health;
        self.metrics.accumulate(&other.metrics);
        for (slot, value) in other.by_ability.iter().enumerate() {
            self.by_ability[slot] += value;
        }
        let counters = &mut self.counters;
        let source = &other.counters;
        counters.casts_started += source.casts_started;
        counters.casts_completed += source.casts_completed;
        counters.casts_wasted += source.casts_wasted;
        counters.casts_interrupted += source.casts_interrupted;
        counters.detonations += source.detonations;
        counters.ignite_applications += source.ignite_applications;
        counters.crits += source.crits;
        counters.hits += source.hits;
        counters.cooldown_reduction += source.cooldown_reduction;
        counters.wasted_cooldown_reduction += source.wasted_cooldown_reduction;
        counters.heat_cycles += source.heat_cycles;
        counters.pyroblast_bounces += source.pyroblast_bounces;
        counters.heat_sum += source.heat_sum;
        counters.heat_samples += source.heat_samples;
        counters.seconds_warm += source.seconds_warm;
        counters.seconds_hot += source.seconds_hot;
        counters.seconds_inferno += source.seconds_inferno;
        counters.ignite_stack_sum += source.ignite_stack_sum;
        counters.ignite_stack_samples += source.ignite_stack_samples;
    }

    /// Divides every total by `runs`, turning an accumulation into an average.
    #[must_use]
    pub fn averaged(&self, runs: u32) -> RunResult {
        if runs == 0 {
            return self.clone();
        }
        let divisor = f64::from(runs);
        let scale = |value: f64| value / divisor;
        let mut result = self.clone();
        result.damage = scale(self.damage);
        result.dps = scale(self.dps);
        result.overkill = scale(self.overkill);
        result.seconds = scale(self.seconds);
        result.max_health = scale(self.max_health);
        result.metrics = self.metrics.averaged(runs);
        for value in &mut result.by_ability {
            *value = scale(*value);
        }
        result.deaths = self.deaths / runs;
        let counters = &mut result.counters;
        counters.casts_started /= runs;
        counters.casts_completed /= runs;
        counters.casts_wasted /= runs;
        counters.casts_interrupted /= runs;
        counters.detonations /= runs;
        counters.ignite_applications /= runs;
        counters.crits /= runs;
        counters.hits /= runs;
        counters.cooldown_reduction /= runs as f32;
        counters.wasted_cooldown_reduction /= runs as f32;
        counters.heat_cycles /= runs;
        counters.pyroblast_bounces /= runs;
        counters.seconds_warm = scale(self.counters.seconds_warm);
        counters.seconds_hot = scale(self.counters.seconds_hot);
        counters.seconds_inferno = scale(self.counters.seconds_inferno);
        result
    }
}

/// One fight, ready to run.
pub struct Encounter {
    simulation: Simulation,
    targets: Vec<EntityId>,
    homes: Vec<Vec3>,
    config: ScenarioConfig,
    rng: Rng,
    until_move: f32,
    hero: Hero,
}

impl Encounter {
    /// Builds the fight described by `config` for `build`, wearing `gear`.
    ///
    /// A [`Worn::Sheet`] is set on the roster and the character refreshed, which is what a
    /// realm fight does with a player's loadout, so the item arithmetic, the rating curve and
    /// the main-stat bridge are the game's own. A [`Worn::Bonus`] is a stat block granted on
    /// top of a bare sheet.
    #[must_use]
    pub fn new(config: &ScenarioConfig, build: &ClassBuild, gear: &Worn, seed: u64) -> Self {
        let arena = ArenaConfig {
            arena_half_extent: 60.0,
            ..ArenaConfig::default()
        };
        let mut simulation = Simulation::new(SimConfig {
            movement: arena.movement(),
            rng_seed: seed,
            ..SimConfig::default()
        });
        arena::build_collision(simulation.collision_world_mut(), &arena);

        let world = simulation.world_mut();
        world.clear();
        let player = world.spawn(EntityDesc::player(Vec3::ZERO));

        // A square-ish pack in front of the mage, which is what an area ability is aimed at.
        let columns = (config.targets as f32).sqrt().ceil().max(1.0) as usize;
        let mut targets = Vec::with_capacity(config.targets);
        let mut homes = Vec::with_capacity(config.targets);
        for index in 0..config.targets {
            let column = index % columns;
            let row = index / columns;
            let offset_x = (column as f32 - (columns as f32 - 1.0) * 0.5) * config.spacing;
            let offset_z = -(config.distance + row as f32 * config.spacing);
            let position = Vec3::new(offset_x, 0.0, offset_z);
            homes.push(position);
            targets.push(world.spawn(EntityDesc {
                kind: EntityKind::Dummy,
                faction: Faction::Hostile,
                position,
                max_health: config.target_health,
                ..EntityDesc::default()
            }));
        }

        simulation.set_player_entity(player);
        simulation.set_hero(build.class().hero());
        match gear {
            Worn::Sheet(sheet) => {
                *simulation.roster_mut().sheet_mut(build.class().hero()) = (**sheet).clone();
                simulation.refresh_character();
            }
            Worn::Bonus(stats) => simulation.set_stat_bonus(*stats),
        }
        simulation.select_nearest_target();
        build.apply(&mut simulation);
        // The Mage's Ultimate is ready at the pull. A boss fight is entered after trash, and
        // the bar refills in about ninety seconds of casting (see the refill test), so a fresh
        // fight starts with a full bar; the beam's own ninety-second cooldown then sets how
        // often it is used. Inert for the other two heroes.
        simulation
            .mage_mut()
            .gain_energy(engine_game::firemage::ENERGY_MAXIMUM);

        Self {
            simulation,
            targets,
            homes,
            config: config.clone(),
            rng: Rng::new(seed ^ 0x9E37_79B9_7F4A_7C15),
            until_move: config.move_interval,
            hero: build.class().hero(),
        }
    }

    /// Runs the fight to its end and reports what happened.
    pub fn run(&mut self, ai: &mut dyn Rotation) -> RunResult {
        let step = self.simulation.timestep().step_seconds();
        let ticks = (self.config.duration / step).round() as u32;
        let mut recorder = Recorder::new(self.hero);

        for _ in 0..ticks {
            if self.living().is_empty() {
                // Nothing left to kill: the fight is over, and running the clock down would
                // dilute the DPS with time the player would not have spent.
                break;
            }
            self.wander(step);
            ai.act(&mut self.simulation, &self.targets);
            recorder.observe_events(&self.simulation);
            self.simulation.step_once();
            recorder.observe(&self.simulation, &self.targets);
        }

        let metrics = recorder.finish(&self.simulation);
        self.result(metrics)
    }

    /// The living targets.
    fn living(&self) -> Vec<EntityId> {
        self.targets
            .iter()
            .copied()
            .filter(|id| self.simulation.world().is_alive(*id))
            .collect()
    }

    /// Moves the pack about, so no ability can assume a stationary line of dummies.
    fn wander(&mut self, step: f32) {
        self.until_move -= step;
        if self.until_move > 0.0 {
            return;
        }
        self.until_move += self.config.move_interval;
        if self.config.wander_chance <= 0.0 {
            return;
        }

        for index in 0..self.targets.len() {
            let id = self.targets[index];
            if !self.simulation.world().is_alive(id) {
                continue;
            }
            let home = self.homes[index];
            let Some(position) = self.simulation.world().position(id) else {
                continue;
            };
            let away = (position.x - home.x).abs() + (position.z - home.z).abs() > 0.1;

            let destination = if away && self.rng.chance(self.config.return_chance) {
                home
            } else if self.rng.chance(self.config.wander_chance) {
                let angle = self.rng.next_f32() * std::f32::consts::TAU;
                Vec3::new(
                    home.x + angle.cos() * self.config.wander_distance,
                    home.y,
                    home.z + angle.sin() * self.config.wander_distance,
                )
            } else {
                continue;
            };
            if let Some(body) = self.simulation.world_mut().body_mut(id) {
                body.position = destination;
            }
        }
    }

    /// Collects the meter and the counters into a result.
    fn result(&self, metrics: Metrics) -> RunResult {
        let meter = self.simulation.meter();
        let mut by_ability = [0.0; engine_game::ability::ABILITY_SLOTS];
        for (slot, totals) in meter.abilities().iter().enumerate() {
            by_ability[slot] = totals.damage;
        }
        let seconds = f64::from(self.config.duration).min(self.simulation.time());
        let seconds = if seconds <= 0.0 { 1.0 } else { seconds };

        RunResult {
            damage: meter.total_damage(),
            dps: meter.total_damage() / seconds,
            overkill: meter.total_overkill(),
            deaths: meter.deaths(),
            by_ability,
            counters: self.simulation.fire_counters(),
            seconds,
            max_health: f64::from(health_from_stamina(
                self.simulation.character_stats().get(Stat::Stamina),
            )),
            metrics,
        }
    }

    /// The simulation, for a test that wants to look inside a finished fight.
    #[must_use]
    #[allow(dead_code, reason = "used by the crate's tests")]
    pub fn simulation(&self) -> &Simulation {
        &self.simulation
    }
}

/// Runs `iterations` fights and averages them.
#[must_use]
pub fn run_scenario(
    config: &ScenarioConfig,
    build: &ClassBuild,
    gear: &Worn,
    iterations: u32,
    seed: u64,
) -> RunResult {
    let mut total = RunResult::default();
    for iteration in 0..iterations.max(1) {
        // A different seed per iteration, derived rather than random, so a whole sweep can
        // be replayed exactly from one number.
        let run_seed = seed
            .wrapping_mul(0x5851_F42D_4C95_7F2D)
            .wrapping_add(u64::from(iteration) + 1);
        let mut encounter = Encounter::new(config, build, gear, run_seed);
        let mut ai = build.rotation();
        let result = encounter.run(ai.as_mut());
        total.accumulate(&result);
    }
    total.averaged(iterations.max(1))
}
