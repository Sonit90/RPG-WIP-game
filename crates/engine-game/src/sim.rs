//! The simulation: commands in, fixed ticks, events out.
//!
//! ```text
//! push_command(..)          advance(real_delta)
//!        |                        |
//!        v                        v
//!  CommandBuffer  ---->  FixedTimestep -> N x tick
//!                                          |
//!                    +---------------------+---------------------+
//!                    |          |          |          |          |
//!                 commands   movement   abilities  statuses   deaths
//!                                          |          |          |
//!                                          +----> CombatEvent <---+
//!                                                    |
//!                                                    v
//!                                               CombatMeter
//! ```
//!
//! Everything in this module is `engine-core` and arithmetic. There is no device, no
//! window, no renderer and no clock of its own: time arrives as a `Duration` and is spent
//! in whole 60 Hz ticks, so the same command sequence produces the same state on any
//! machine at any frame rate. That is what Milestones 21 and 22 assert.
//!
//! ## Ordering within a tick
//!
//! Fixed, and load-bearing for determinism:
//!
//! 1. **Commands** — the frame's input becomes intent and discrete actions.
//! 2. **Movement** — every body integrates.
//! 3. **Cooldowns and resource** — so an ability used this tick sees this tick's regen.
//! 4. **Abilities** — validated, resolved, applied; events emitted.
//! 5. **Statuses** — ticked; their damage lands after the abilities of the same tick.
//! 6. **Telemetry** — observes what the tick produced.
//!
//! Two abilities in one tick apply in the order the commands arrived.

use std::time::{Duration, Instant};

use engine_core::math::Vec3;
use engine_core::time::FixedTimestep;

use crate::ability::{AbilityBook, AbilityEffect, AbilityError, AbilityId};
use crate::combat::{DamageType, Resource};
use crate::command::{CommandBuffer, PlayerCommand};
use crate::entity::EntityId;
use crate::event::{CombatEvent, DamageEvent, EventStream};
use crate::movement::{self, Body, MoveIntent, MovementConfig};
use crate::status::{Status, StatusEnd, StatusOutcome};
use crate::targeting::TargetQuery;
use crate::telemetry::CombatMeter;
use crate::world::{EntityKind, GameWorld};

/// How the simulation is configured.
#[derive(Debug, Clone, PartialEq)]
pub struct SimConfig {
    /// Fixed simulation ticks per second.
    pub tick_hz: u32,
    /// How the player's body moves.
    pub movement: MovementConfig,
    /// The player's resource pool.
    pub resource_max: f32,
    /// Resource regenerated per second.
    pub resource_regeneration: f32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            tick_hz: engine_core::time::DEFAULT_TICK_HZ,
            movement: MovementConfig::default(),
            resource_max: 100.0,
            // Fireball costs 12 and comes off cooldown every 1.5 s, which is 8 a second:
            // the filler is sustainable and the expensive abilities are not.
            resource_regeneration: 9.0,
        }
    }
}

/// The player's gameplay state that is not shared with every other entity.
///
/// The body — position, velocity, grounded, movement state — lives in the world beside
/// everyone else's, addressed by [`Player::entity`]. Duplicating it here would create two
/// truths about where the character is, and the movement system would have to be told which
/// one to believe. Use [`Simulation::player_body`] to read it.
#[derive(Debug, Clone)]
pub struct Player {
    /// Which entity in the world the player controls.
    pub entity: EntityId,
    /// The ten ability slots and their cooldowns.
    pub abilities: AbilityBook,
    /// What abilities are paid for with.
    pub resource: Resource,
    /// The currently selected hostile, if any.
    pub target: Option<EntityId>,
    /// The cast in progress, if any.
    ///
    /// A cast is a *commitment*: while one is running the character is channelling, and
    /// anything that breaks the channel cancels it. That is the whole reason it is worth
    /// drawing a bar for.
    pub cast: Option<ActiveCast>,
    /// Why the last ability attempt failed, for the overlay. Cleared by a success.
    pub last_error: Option<AbilityError>,
    /// The last ability that was successfully used.
    pub last_ability: Option<AbilityId>,
}

/// A cast in progress.
///
/// Cooldown and cost are paid when the cast *starts*, not when it lands — so a cancelled
/// cast is a real loss, which is what makes interruption matter. That is the convention the
/// genre uses and it is the one a player will expect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveCast {
    /// Which ability is being cast.
    pub ability: AbilityId,
    /// Simulated seconds spent so far.
    pub elapsed: f32,
    /// Simulated seconds the cast takes.
    pub duration: f32,
    /// The target the cast was started against.
    ///
    /// Held so a cast that lands after its target died fails cleanly rather than hitting
    /// whatever the player selected in the meantime.
    pub target: Option<EntityId>,
}

impl ActiveCast {
    /// Progress in [0, 1].
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Simulated seconds left.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

/// Why a cast ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastEnd {
    /// It finished and its effect landed.
    Completed,
    /// The caster moved.
    Moved,
    /// The caster started another ability.
    Replaced,
    /// The caster died.
    CasterDied,
}

/// Where the simulation spent its time, and how much of it there was.
///
/// Measured with the real clock, because that is what a frame budget is spent in. The
/// simulation's *behaviour* never reads these numbers.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SimStats {
    /// Fixed ticks run by the last [`Simulation::advance`].
    pub ticks: u32,
    /// Total wall-clock time those ticks took.
    pub total: Duration,
    /// Of which: movement integration.
    pub movement: Duration,
    /// Of which: ability validation, resolution and application.
    pub abilities: Duration,
    /// Of which: status ticking and the damage it deals.
    pub statuses: Duration,
    /// Of which: telemetry observing the tick's events.
    pub telemetry: Duration,
    /// Ticks the timestep discarded because the catch-up bound was reached.
    pub dropped_ticks: u64,
}

impl SimStats {
    /// Total simulation time in milliseconds, the form the overlay wants.
    #[must_use]
    pub fn total_ms(&self) -> f64 {
        self.total.as_secs_f64() * 1e3
    }

    /// Milliseconds spent in each named part, in order: movement, abilities, statuses,
    /// telemetry.
    #[must_use]
    pub fn breakdown_ms(&self) -> [f64; 4] {
        [
            self.movement.as_secs_f64() * 1e3,
            self.abilities.as_secs_f64() * 1e3,
            self.statuses.as_secs_f64() * 1e3,
            self.telemetry.as_secs_f64() * 1e3,
        ]
    }
}

/// The gameplay simulation.
///
/// Owns the world, the player, the fixed timestep, the event stream and the combat meter.
/// Nothing outside it advances time.
#[derive(Debug)]
pub struct Simulation {
    world: GameWorld,
    player: Player,
    config: SimConfig,
    timestep: FixedTimestep,
    /// Movement intent, held between commands: a tick that falls between two frames of
    /// input still moves the character.
    intent: MoveIntent,
    /// Discrete commands waiting for the next tick.
    pending: CommandBuffer,
    events: EventStream,
    meter: CombatMeter,
    targets: TargetQuery,
    /// Targets of the ability currently being applied. Reused; never freed.
    hits: Vec<EntityId>,
    /// One entity's status outcomes for the current tick. Reused; never freed.
    outcomes: Vec<StatusOutcome>,
    /// How far into the current event batch telemetry has read.
    ///
    /// A cursor rather than clearing after every tick, because the whole batch stays
    /// readable to the application until the next advance — and because an ability used
    /// directly, outside a tick, still has to reach the meter exactly once.
    observed: usize,
    tick: u64,
    time: f64,
    stats: SimStats,
    /// The most recent advance that actually ran a tick.
    ///
    /// At 300 FPS against a 60 Hz tick, four frames in five run no ticks at all and their
    /// timings are legitimately zero. A readout showing those would report `SIM 0.000 MS`
    /// most of the time, which reads as "the simulation is free" rather than "the
    /// simulation did not run this frame".
    last_active_stats: SimStats,
}

impl Simulation {
    /// An empty simulation with no player entity yet.
    ///
    /// [`crate::arena`] is what builds a playable one; this exists for tests and for a
    /// future scenario loader.
    #[must_use]
    pub fn new(config: SimConfig) -> Self {
        let timestep = FixedTimestep::new(config.tick_hz);
        let resource = Resource::new(config.resource_max, config.resource_regeneration);
        Self {
            world: GameWorld::new(),
            player: Player {
                entity: EntityId::INVALID,
                abilities: AbilityBook::default(),
                resource,
                target: None,
                cast: None,
                last_error: None,
                last_ability: None,
            },
            config,
            timestep,
            intent: MoveIntent::NONE,
            pending: CommandBuffer::with_capacity(16),
            events: EventStream::with_capacity(256),
            meter: CombatMeter::new(),
            targets: TargetQuery::with_capacity(64),
            hits: Vec::with_capacity(64),
            outcomes: Vec::with_capacity(16),
            observed: 0,
            tick: 0,
            time: 0.0,
            stats: SimStats::default(),
            last_active_stats: SimStats::default(),
        }
    }

    /// The world.
    #[must_use]
    pub fn world(&self) -> &GameWorld {
        &self.world
    }

    /// The world, for a scenario builder. Gameplay systems go through the simulation.
    pub fn world_mut(&mut self) -> &mut GameWorld {
        &mut self.world
    }

    /// The player.
    #[must_use]
    pub fn player(&self) -> &Player {
        &self.player
    }

    /// The player, mutably.
    ///
    /// The counterpart to [`Simulation::world_mut`], and used for the same reasons: a test
    /// or a scenario setting up a state the ordinary command path would take seconds of
    /// simulated time to reach. Ordinary gameplay goes through commands.
    pub fn player_mut(&mut self) -> &mut Player {
        &mut self.player
    }

    /// Names the entity the player controls.
    pub fn set_player_entity(&mut self, entity: EntityId) {
        self.player.entity = entity;
    }

    /// The player's body: position, velocity, ground contact and movement state.
    #[must_use]
    pub fn player_body(&self) -> Option<&Body> {
        self.world.body(self.player.entity)
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &SimConfig {
        &self.config
    }

    /// The fixed timestep.
    #[must_use]
    pub fn timestep(&self) -> &FixedTimestep {
        &self.timestep
    }

    /// Seconds of simulation time elapsed.
    ///
    /// The sum of every tick's fixed step, so it is exactly `ticks / tick_hz` and does not
    /// drift with the frame rate.
    #[must_use]
    pub fn time(&self) -> f64 {
        self.time
    }

    /// Ticks run since the simulation was created.
    #[must_use]
    pub fn tick_count(&self) -> u64 {
        self.tick
    }

    /// The events emitted by the most recent [`Simulation::advance`].
    #[must_use]
    pub fn events(&self) -> &[CombatEvent] {
        self.events.events()
    }

    /// The event stream, for its lifetime counters.
    #[must_use]
    pub fn event_stream(&self) -> &EventStream {
        &self.events
    }

    /// The combat meter.
    #[must_use]
    pub fn meter(&self) -> &CombatMeter {
        &self.meter
    }

    /// Where the last advance spent its time.
    ///
    /// Zero when that advance ran no ticks, which at the engine's frame rates is most
    /// frames. Use [`Simulation::last_tick_stats`] for a readout.
    #[must_use]
    pub fn stats(&self) -> SimStats {
        self.stats
    }

    /// Where the last advance that actually ticked spent its time.
    ///
    /// What a developer overlay should show: the cost of simulating, rather than a zero
    /// from a frame that happened to fall between two ticks.
    #[must_use]
    pub fn last_tick_stats(&self) -> SimStats {
        self.last_active_stats
    }

    /// Queues a command for the next tick.
    pub fn push_command(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::Move { .. } => {
                // Held, not queued: the newest movement intent replaces the previous one,
                // so a frame that produced three of them does not move three times.
                self.intent.direction = command.move_direction();
            }
            other => self.pending.push(other),
        }
    }

    /// Queues several commands.
    pub fn push_commands(&mut self, commands: impl IntoIterator<Item = PlayerCommand>) {
        for command in commands {
            self.push_command(command);
        }
    }

    /// Advances the simulation by a real frame delta, running whole fixed ticks.
    ///
    /// Returns how many ticks ran, which is usually zero or one at the engine's frame
    /// rates. The events of every tick in the batch stay readable until the next call.
    pub fn advance(&mut self, delta: Duration) -> u32 {
        self.begin_batch();
        let ticks = self.timestep.accumulate(delta);
        let started = Instant::now();
        let mut stats = SimStats {
            ticks,
            dropped_ticks: self.timestep.dropped_ticks(),
            ..SimStats::default()
        };

        for _ in 0..ticks {
            self.tick_once(&mut stats);
        }

        stats.total = started.elapsed();
        self.record_stats(stats);
        ticks
    }

    /// Runs exactly one fixed tick, whatever the clock says.
    ///
    /// The door determinism tests and benchmarks drive the simulation through: no wall
    /// clock, no accumulator, one step.
    pub fn step_once(&mut self) {
        self.begin_batch();
        let mut stats = SimStats {
            ticks: 1,
            ..SimStats::default()
        };
        let started = Instant::now();
        self.tick_once(&mut stats);
        stats.total = started.elapsed();
        self.record_stats(stats);
    }

    /// Runs `count` fixed ticks. The events of the whole run are kept.
    pub fn step_many(&mut self, count: u32) {
        self.begin_batch();
        let mut stats = SimStats {
            ticks: count,
            ..SimStats::default()
        };
        let started = Instant::now();
        for _ in 0..count {
            self.tick_once(&mut stats);
        }
        stats.total = started.elapsed();
        self.record_stats(stats);
    }

    /// Stores an advance's timings, keeping the last non-empty one for readouts.
    fn record_stats(&mut self, stats: SimStats) {
        self.stats = stats;
        if stats.ticks > 0 {
            self.last_active_stats = stats;
        }
    }

    /// Starts a new event batch, keeping the buffer.
    ///
    /// Anything emitted since the last observation — an ability used directly rather than
    /// through a command, which is what tests and a future scripted encounter do — reaches
    /// the meter before the events are dropped. Otherwise its damage would be applied to
    /// the world and missing from the meter, which is the one inconsistency a damage meter
    /// must never have.
    fn begin_batch(&mut self) {
        self.flush_observations();
        self.events.clear();
        self.observed = 0;
    }

    /// Lets telemetry read every event it has not seen yet.
    fn flush_observations(&mut self) {
        if self.observed >= self.events.len() {
            return;
        }
        let time = self.time;
        let cursor = self.observed;
        self.meter.observe(self.events.events_after(cursor), time);
        self.observed = self.events.len();
    }

    /// One fixed tick.
    fn tick_once(&mut self, stats: &mut SimStats) {
        let step = self.timestep.step_seconds();
        self.tick += 1;
        self.time += f64::from(step);

        // Before anything moves: record where every body is, so presentation can draw
        // between this tick and the next instead of snapping between them. Nothing in
        // gameplay reads the result.
        self.world.begin_tick();

        self.apply_pending_commands();

        let movement_started = Instant::now();
        self.integrate_bodies(step);
        stats.movement += movement_started.elapsed();

        let ability_started = Instant::now();
        self.player.abilities.tick(step);
        self.player.resource.regenerate(step);
        // After the frame's commands have been applied — an instant ability used this tick
        // has already interrupted any cast — and before statuses, so a cast that lands this
        // tick applies its damage before this tick's burns.
        self.tick_cast(step);
        self.retarget_if_needed();
        stats.abilities += ability_started.elapsed();

        let status_started = Instant::now();
        self.tick_statuses(step);
        stats.statuses += status_started.elapsed();

        let telemetry_started = Instant::now();
        self.flush_observations();
        stats.telemetry += telemetry_started.elapsed();
    }

    /// Applies the discrete commands that arrived since the last tick.
    ///
    /// Drained here rather than in `advance`, so a batch of catch-up ticks applies them
    /// once — a single jump press must not become eight jumps.
    fn apply_pending_commands(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        // Taken out of `self` so the loop can call `&mut self` methods; the buffer is put
        // back, keeping its allocation.
        let mut pending = std::mem::take(&mut self.pending);
        for command in pending.commands() {
            match *command {
                PlayerCommand::Jump => self.intent.jump = true,
                // The refusal, if any, is kept on the player for the overlay rather than
                // propagated: a command is a request, and a request that cannot be honoured
                // is not an error the frame should fail on.
                PlayerCommand::UseAbility(ability) => {
                    let _ = self.use_ability(ability);
                }
                PlayerCommand::SelectNearestTarget => {
                    self.player.target = self
                        .targets
                        .nearest_hostile(&self.world, self.player.entity);
                }
                PlayerCommand::CycleTarget => {
                    let current = self.player.target;
                    self.player.target =
                        self.targets
                            .next_hostile(&self.world, self.player.entity, current);
                }
                PlayerCommand::ResetDummies => self.reset_dummies(),
                PlayerCommand::Move { .. } => {}
            }
        }
        pending.clear();
        self.pending = pending;
    }

    /// Integrates every body.
    ///
    /// Dummies get the same integration as the player with an empty intent: they are on the
    /// ground and stay there, but running one code path means a dummy that is ever pushed,
    /// dropped or knocked back behaves like the player rather than like a special case.
    fn integrate_bodies(&mut self, step: f32) {
        let player = self.player.entity;
        let intent = self.intent;
        let config = self.config.movement;

        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            let body_intent = if id == player {
                intent
            } else {
                MoveIntent::NONE
            };
            if let Some(body) = self.world.body_mut(id) {
                movement::step(body, body_intent, &config, step);
            }
        }

        // Consumed, not held: a jump lasts one tick, and the next tick's intent comes from
        // the next frame's input.
        self.intent.jump = false;
    }

    /// Keeps the nearest hostile selected when nothing valid is.
    ///
    /// A prototype whose target silently becomes nothing when a dummy dies looks broken.
    /// Deterministic, because "nearest" is resolved by distance with the entity id as the
    /// tie-break.
    fn retarget_if_needed(&mut self) {
        let valid = self
            .player
            .target
            .is_some_and(|target| self.world.is_alive(target));
        if valid {
            return;
        }
        self.player.target = self
            .targets
            .nearest_hostile(&self.world, self.player.entity);
    }

    /// Ticks every entity's statuses and applies the damage they deal.
    fn tick_statuses(&mut self, step: f32) {
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if !self.world.is_alive(id) {
                continue;
            }

            self.outcomes.clear();
            if let Some(statuses) = self.world.statuses_mut(id) {
                statuses.tick(step, &mut self.outcomes);
            }
            if self.outcomes.is_empty() {
                continue;
            }

            // Indexed rather than iterated: applying damage needs `&mut self`, and the
            // outcomes are `Copy`, so each is taken out before the call.
            for index in 0..self.outcomes.len() {
                match self.outcomes[index] {
                    StatusOutcome::Tick {
                        kind,
                        source,
                        ability,
                        damage,
                    } => {
                        self.deal_damage(source, id, damage, ability, kind.damage_type());
                    }
                    StatusOutcome::Ended { kind, reason } => {
                        let tick = self.tick;
                        self.events.push(CombatEvent::StatusRemoved {
                            target: id,
                            status: kind,
                            reason,
                            tick,
                        });
                    }
                }
            }
        }
    }

    /// Advances the cast in progress, completing it when it finishes.
    ///
    /// Ordered after movement within the tick, so a cast started and moved out of in the
    /// same tick is cancelled by that movement rather than landing first.
    fn tick_cast(&mut self, step: f32) {
        let Some(mut cast) = self.player.cast else {
            return;
        };

        if !self.world.is_alive(self.player.entity) {
            self.end_cast(CastEnd::CasterDied);
            return;
        }
        // Moving breaks a channel. Read from the body's own movement state rather than from
        // the command, so a body still sliding to a stop is still moving.
        if self
            .world
            .body(self.player.entity)
            .is_some_and(|body| body.state != crate::movement::MovementState::Idle)
        {
            self.end_cast(CastEnd::Moved);
            return;
        }

        cast.elapsed += step;
        if cast.elapsed + crate::TIME_EPSILON < cast.duration {
            self.player.cast = Some(cast);
            return;
        }

        // Finished. The effect lands now, against the target the cast was started on.
        self.player.cast = None;
        let ability = cast.ability;
        self.events.push(CombatEvent::CastEnded {
            source: self.player.entity,
            ability,
            end: CastEnd::Completed,
            tick: self.tick,
        });
        let restore = self.player.target;
        self.player.target = cast.target;
        // Cooldown and cost were paid when the cast started, so the landing skips both.
        self.apply_ability(ability);
        self.player.target = restore;
    }

    /// Cancels the cast in progress and announces why.
    fn end_cast(&mut self, end: CastEnd) {
        let Some(cast) = self.player.cast.take() else {
            return;
        };
        self.events.push(CombatEvent::CastEnded {
            source: self.player.entity,
            ability: cast.ability,
            end,
            tick: self.tick,
        });
    }

    /// Whether a cast is in progress.
    #[must_use]
    pub fn casting(&self) -> Option<&ActiveCast> {
        self.player.cast.as_ref()
    }

    /// Uses an ability, or says why it could not be used.
    ///
    /// Validation order is deliberate: the cheapest and most actionable refusals come
    /// first, and nothing is spent until every check has passed.
    pub fn use_ability(&mut self, id: AbilityId) -> Result<u32, AbilityError> {
        let result = self.try_use_ability(id);
        match result {
            Ok(_) => {
                self.player.last_error = None;
                self.player.last_ability = Some(id);
            }
            Err(error) => self.player.last_error = Some(error),
        }
        result
    }

    /// The body of [`Simulation::use_ability`], without the bookkeeping.
    fn try_use_ability(&mut self, id: AbilityId) -> Result<u32, AbilityError> {
        let definition = *self
            .player
            .abilities
            .get(id)
            .ok_or(AbilityError::UnknownSlot)?;
        let caster = self.player.entity;

        if !self.world.is_alive(caster) {
            return Err(AbilityError::CasterDead);
        }
        if !self.player.abilities.is_ready(id) {
            return Err(AbilityError::OnCooldown);
        }
        if !self.player.resource.can_pay(definition.cost) {
            return Err(AbilityError::NotEnoughResource);
        }

        // A cast ability commits here and lands later. Targeting is validated *now* — a cast
        // that could not have been started should be refused with the reason it was refused
        // for, not begun and then silently dropped two seconds later.
        if definition.cast_time > 0.0 {
            self.targets
                .resolve(&self.world, caster, self.player.target, &definition)?;
            self.player.resource.pay(definition.cost);
            self.player.abilities.start_cooldown(id);
            // Starting a cast replaces any cast already running.
            if self.player.cast.is_some() {
                self.end_cast(CastEnd::Replaced);
            }
            self.player.cast = Some(ActiveCast {
                ability: id,
                elapsed: 0.0,
                duration: definition.cast_time,
                target: self.player.target,
            });
            self.events.push(CombatEvent::CastStarted {
                source: caster,
                ability: id,
                duration: definition.cast_time,
                tick: self.tick,
            });
            return Ok(0);
        }

        // An instant ability interrupts a cast in progress: the player asked for something
        // else, and leaving both running would have two abilities in flight at once.
        if self.player.cast.is_some() {
            self.end_cast(CastEnd::Replaced);
        }

        // Resolved into the reusable scratch, then copied out: applying the effect needs
        // `&mut self.world`, which the resolved slice borrows.
        let resolved =
            self.targets
                .resolve(&self.world, caster, self.player.target, &definition)?;
        self.hits.clear();
        self.hits.extend_from_slice(resolved);

        self.player.resource.pay(definition.cost);
        self.player.abilities.start_cooldown(id);
        Ok(self.apply_resolved(id))
    }

    /// Applies an ability whose cost and cooldown have already been paid.
    ///
    /// The landing half of a cast: resolves targets afresh — a cast that took two seconds
    /// may have watched its target die — then applies the effect. Returns how many entities
    /// it affected, zero being a legitimate miss rather than an error.
    fn apply_ability(&mut self, id: AbilityId) -> u32 {
        let Some(definition) = self.player.abilities.get(id).copied() else {
            return 0;
        };
        let caster = self.player.entity;
        let Ok(resolved) =
            self.targets
                .resolve(&self.world, caster, self.player.target, &definition)
        else {
            // The target died or walked out of range while the cast ran. The cast is over
            // and the cost is spent; that is what committing to a cast means.
            return 0;
        };
        self.hits.clear();
        self.hits.extend_from_slice(resolved);
        self.apply_resolved(id)
    }

    /// Applies `id`'s effect to whatever is currently in the hit list.
    fn apply_resolved(&mut self, id: AbilityId) -> u32 {
        let Some(definition) = self.player.abilities.get(id).copied() else {
            return 0;
        };
        let caster = self.player.entity;
        let tick = self.tick;
        let count = self.hits.len() as u32;
        self.events.push(CombatEvent::AbilityUsed {
            source: caster,
            ability: id,
            targets: count,
            tick,
        });

        for index in 0..self.hits.len() {
            let target = self.hits[index];
            match definition.effect {
                AbilityEffect::DirectDamage { amount } => {
                    self.deal_damage(caster, target, amount, id, definition.damage_type);
                }
                AbilityEffect::ApplyStatus {
                    impact,
                    status,
                    duration,
                    damage_per_tick,
                } => {
                    if impact > 0.0 {
                        self.deal_damage(caster, target, impact, id, definition.damage_type);
                    }
                    // Only the living burn: applying a status to something the impact just
                    // killed leaves a corpse ticking.
                    if self.world.is_alive(target)
                        && let Some(statuses) = self.world.statuses_mut(target)
                    {
                        statuses.apply(Status::new(status, caster, id, duration, damage_per_tick));
                        self.events.push(CombatEvent::StatusApplied {
                            source: caster,
                            target,
                            status,
                            duration,
                            tick,
                        });
                    }
                }
                AbilityEffect::Detonate {
                    base,
                    consumes,
                    bonus_per_second_remaining,
                } => {
                    let consumed = self
                        .world
                        .statuses_mut(target)
                        .and_then(|statuses| statuses.consume(consumes));
                    let bonus = match consumed {
                        Some(status) => {
                            self.events.push(CombatEvent::StatusRemoved {
                                target,
                                status: consumes,
                                reason: StatusEnd::Consumed,
                                tick,
                            });
                            status.remaining * bonus_per_second_remaining
                        }
                        None => 0.0,
                    };
                    self.deal_damage(caster, target, base + bonus, id, definition.damage_type);
                }
            }
        }

        count
    }

    /// Applies damage and emits the events it produces.
    ///
    /// The single place health goes down as a result of combat, which is what makes "every
    /// damage instance is observable" true by construction rather than by convention.
    fn deal_damage(
        &mut self,
        source: EntityId,
        target: EntityId,
        amount: f32,
        ability: AbilityId,
        damage_type: DamageType,
    ) {
        let result = self.world.damage(target, amount);
        if result.applied <= 0.0 && !result.killed {
            return;
        }

        let tick = self.tick;
        self.events.push(CombatEvent::Damage(DamageEvent {
            source,
            target,
            amount: result.applied,
            overkill: result.overkill,
            ability,
            damage_type,
            killed: result.killed,
            tick,
        }));

        if result.killed {
            // A corpse does not burn. Reported rather than silently dropped, so a status
            // bar that is watching sees it end.
            self.outcomes.clear();
            let mut ended = std::mem::take(&mut self.outcomes);
            if let Some(statuses) = self.world.statuses_mut(target) {
                statuses.clear_into(&mut ended);
            }
            for outcome in &ended {
                if let StatusOutcome::Ended { kind, reason } = *outcome {
                    self.events.push(CombatEvent::StatusRemoved {
                        target,
                        status: kind,
                        reason,
                        tick,
                    });
                }
            }
            ended.clear();
            self.outcomes = ended;

            self.events.push(CombatEvent::Death {
                entity: target,
                killer: source,
                tick,
            });
        }
    }

    /// Restores every dummy and starts a new combat session.
    ///
    /// Health, position and statuses come back, the meter is cleared, and cooldowns and the
    /// resource are refilled — the point of the control is to try a rotation again from a
    /// known state, and waiting eight seconds for Detonate is not part of that state.
    pub fn reset_dummies(&mut self) {
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if self.world.kind(id) == Some(EntityKind::Dummy) {
                self.world.restore(id);
            }
        }
        self.player.abilities.reset_cooldowns();
        self.player.resource.restore();
        // A cast in progress is part of the state R exists to clear: the point of the
        // control is to try a rotation again from a known start, and "mid-cast" is not it.
        self.player.cast = None;
        self.player.last_error = None;
        self.meter.reset(self.time);
        self.retarget_if_needed();
        log::debug!("dummies reset at simulation time {:.2} s", self.time);
    }

    /// Selects the nearest living hostile.
    pub fn select_nearest_target(&mut self) {
        self.player.target = self
            .targets
            .nearest_hostile(&self.world, self.player.entity);
    }

    /// Where the player's feet are, or the origin if there is no player.
    #[must_use]
    pub fn player_position(&self) -> Vec3 {
        self.player_body().map_or(Vec3::ZERO, |body| body.position)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "cast tests assert on known-good values")]
mod cast_tests {
    use super::*;
    use crate::arena::{self, ArenaConfig};

    /// Slot 7 (`LANCE`) is the short cast; slot 10 (`SMITE`) is the long one.
    const LANCE: usize = 6;

    fn sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    fn lance() -> AbilityId {
        AbilityId::from_slot(LANCE).expect("slot 7 exists")
    }

    #[test]
    fn an_instant_ability_still_lands_on_the_tick_it_is_used() {
        // The regression that matters most: adding casts must not have made every ability
        // one.
        let mut simulation = sim();
        let before = simulation.meter().total_damage();
        simulation.push_command(PlayerCommand::UseAbility(
            AbilityId::from_slot(0).expect("fireball"),
        ));
        simulation.step_once();
        assert!(simulation.meter().total_damage() > before);
        assert!(simulation.casting().is_none(), "fireball is instant");
    }

    #[test]
    fn a_cast_ability_starts_a_cast_and_lands_when_it_finishes() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();

        let cast = *simulation.casting().expect("a cast is running");
        assert_eq!(cast.ability, lance());
        assert!(cast.duration > 0.0);
        assert_eq!(simulation.meter().total_damage(), 0.0, "nothing landed yet");

        // Run past the cast time; the damage arrives with it.
        simulation.step_many(120);
        assert!(simulation.casting().is_none(), "the cast finished");
        assert!(
            simulation.meter().total_damage() > 0.0,
            "the effect should have landed"
        );
    }

    #[test]
    fn a_cast_progresses_on_simulation_time() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        let first = simulation.casting().expect("casting").elapsed;
        simulation.step_many(10);
        let later = simulation.casting().expect("still casting").elapsed;
        assert!(later > first);
        assert!(simulation.casting().expect("casting").fraction() > 0.0);
        assert!(simulation.casting().expect("casting").remaining() > 0.0);
    }

    #[test]
    fn moving_cancels_a_cast_and_the_effect_never_lands() {
        // What makes a cast a commitment rather than a delay.
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        assert!(simulation.casting().is_some());

        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
        });
        simulation.step_many(3);
        assert!(simulation.casting().is_none(), "moving broke the channel");

        simulation.step_many(240);
        assert_eq!(
            simulation.meter().total_damage(),
            0.0,
            "a cancelled cast must not land later"
        );
    }

    #[test]
    fn a_cancelled_cast_keeps_its_cost_and_its_cooldown() {
        // The convention the genre uses: cancelling wastes the resource. Refunding it would
        // make starting a cast free and interruption meaningless.
        let mut simulation = sim();
        let full = simulation.player().resource.current();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        let spent = simulation.player().resource.current();
        assert!(spent < full, "the cast paid up front");

        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
        });
        simulation.step_many(3);
        assert!(
            !simulation.player().abilities.is_ready(lance()),
            "the cooldown is still running"
        );
    }

    #[test]
    fn starting_another_ability_replaces_the_cast() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        assert!(simulation.casting().is_some());

        simulation.push_command(PlayerCommand::UseAbility(
            AbilityId::from_slot(0).expect("fireball"),
        ));
        simulation.step_once();
        assert!(
            simulation.casting().is_none(),
            "an instant ability interrupts the channel"
        );
        assert!(simulation.meter().total_damage() > 0.0, "fireball landed");
    }

    #[test]
    fn a_cast_that_cannot_be_started_is_refused_with_its_reason() {
        // Validated when it starts, not two seconds later: a cast begun against nothing and
        // silently dropped on landing would look like the ability was broken.
        let mut simulation = sim();
        simulation.player_mut().target = None;
        let error = simulation.use_ability(lance()).expect_err("no target");
        assert_eq!(error, AbilityError::NoTarget);
        assert!(simulation.casting().is_none());
    }

    #[test]
    fn a_cast_whose_target_dies_ends_without_landing() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        let target = simulation.player().target.expect("a target");
        simulation.world_mut().damage(target, 1_000_000.0);

        simulation.step_many(120);
        assert!(simulation.casting().is_none());
        // The dummy died to the direct damage above, which the meter does not see; the cast
        // itself must have added nothing.
        assert_eq!(simulation.meter().total_damage(), 0.0);
    }

    #[test]
    fn the_cast_stream_announces_both_ends() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        assert!(
            simulation
                .events()
                .iter()
                .any(|event| matches!(event, CombatEvent::CastStarted { .. })),
            "starting a cast is announced"
        );

        simulation.step_many(120);
        assert!(
            simulation.event_stream().total_recorded() > 0,
            "the stream recorded the cast"
        );
    }

    #[test]
    fn a_reset_clears_a_cast_in_progress() {
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(lance()));
        simulation.step_once();
        assert!(simulation.casting().is_some());

        simulation.reset_dummies();
        assert!(
            simulation.casting().is_none(),
            "R returns the arena to a known state, which includes not casting"
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::arena;
    use crate::status::StatusKind;
    use crate::world::EntityDesc;

    /// Slot of each designed ability.
    const FIREBALL: usize = 0;
    const IGNITE: usize = 1;
    const DETONATE: usize = 2;

    fn ability(slot: usize) -> AbilityId {
        AbilityId::from_slot(slot).expect("the slot exists")
    }

    /// A default training arena, already targeted at the single dummy.
    fn arena() -> Simulation {
        let mut simulation = arena::training_arena(&arena::ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    fn dummy_health(simulation: &Simulation, id: EntityId) -> f32 {
        simulation
            .world()
            .health(id)
            .map_or(0.0, crate::combat::Health::current)
    }

    #[test]
    fn an_ability_damages_the_selected_target_and_says_so() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is targeted");
        let before = dummy_health(&simulation, target);

        let hits = simulation
            .use_ability(ability(FIREBALL))
            .expect("a dummy is targeted and in range");
        assert_eq!(hits, 1);
        assert!(dummy_health(&simulation, target) < before);

        let damage: f32 = simulation
            .events()
            .iter()
            .filter_map(|event| event.as_damage().map(|damage| damage.amount))
            .sum();
        assert!((damage - (before - dummy_health(&simulation, target))).abs() < 1e-3);
    }

    #[test]
    fn an_ability_cannot_be_spammed_and_comes_back_on_its_own() {
        let mut simulation = arena();
        assert!(simulation.use_ability(ability(FIREBALL)).is_ok());
        assert_eq!(
            simulation.use_ability(ability(FIREBALL)),
            Err(AbilityError::OnCooldown)
        );

        // 1.5 s of cooldown at 60 Hz.
        simulation.step_many(90);
        assert!(
            simulation.use_ability(ability(FIREBALL)).is_ok(),
            "the cooldown never expired"
        );
    }

    #[test]
    fn an_unaffordable_ability_is_refused_without_spending_anything() {
        // The pool is 100 and the three most expensive placeholders cost 40, 35 and 30:
        // the first two drain it, and the third has to be refused for a reason that is not
        // its cooldown.
        let mut simulation = arena();
        simulation.use_ability(ability(9)).expect("40 of 100");
        simulation.use_ability(ability(8)).expect("35 of the rest");

        let resource = simulation.player().resource.current();
        assert!(resource < 30.0, "the pool was not drained: {resource}");

        let unaffordable = ability(7);
        assert!(simulation.player().abilities.is_ready(unaffordable));
        assert_eq!(
            simulation.use_ability(unaffordable),
            Err(AbilityError::NotEnoughResource)
        );
        assert_eq!(
            simulation.player().resource.current(),
            resource,
            "a refused ability spent resource"
        );
        assert!(
            simulation.player().abilities.is_ready(unaffordable),
            "a refused ability started its cooldown"
        );
    }

    #[test]
    fn ignite_burns_the_target_over_time_and_the_damage_is_credited_to_ignite() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is targeted");
        simulation
            .use_ability(ability(IGNITE))
            .expect("the dummy is in range");

        assert!(
            simulation
                .world()
                .statuses(target)
                .is_some_and(|statuses| statuses.has(StatusKind::Burning))
        );

        let after_impact = dummy_health(&simulation, target);
        simulation.step_many(180); // three seconds
        let after_burn = dummy_health(&simulation, target);
        assert!(
            after_burn < after_impact,
            "the burn dealt no damage: {after_impact} -> {after_burn}"
        );

        let totals = simulation.meter().ability(ability(IGNITE));
        assert!(totals.damage > 0.0);
        assert_eq!(totals.casts, 1);
        assert!(
            totals.hits > 1,
            "the impact and its ticks are separate hits"
        );
    }

    #[test]
    fn detonate_is_worth_more_against_a_burning_target_than_a_cold_one() {
        // The synergy, asserted as the thing that makes it a synergy: the same ability,
        // twice, differing only in whether the setup was applied.
        let cold = {
            let mut simulation = arena();
            let target = simulation.player().target.expect("a dummy is targeted");
            let before = dummy_health(&simulation, target);
            simulation
                .use_ability(ability(DETONATE))
                .expect("the dummy is in range");
            before - dummy_health(&simulation, target)
        };

        let hot = {
            let mut simulation = arena();
            let target = simulation.player().target.expect("a dummy is targeted");
            simulation
                .use_ability(ability(IGNITE))
                .expect("the dummy is in range");
            let before = dummy_health(&simulation, target);
            simulation
                .use_ability(ability(DETONATE))
                .expect("the dummy is in range");
            let dealt = before - dummy_health(&simulation, target);

            assert!(
                !simulation
                    .world()
                    .statuses(target)
                    .is_some_and(|statuses| statuses.has(StatusKind::Burning)),
                "detonate did not consume the burn"
            );
            dealt
        };

        assert!(
            hot > cold * 2.0,
            "detonate on a burning target dealt {hot} against {cold} on a cold one"
        );
    }

    #[test]
    fn an_area_ability_hits_the_group_and_nothing_outside_it() {
        let config = arena::ArenaConfig::default();
        let mut simulation = arena::training_arena(&config);
        simulation.step_once();

        // Target a member of the group so the blast is centred on it.
        let group: Vec<EntityId> = simulation
            .world()
            .entities()
            .filter(|id| {
                simulation.world().kind(*id) == Some(EntityKind::Dummy)
                    && simulation
                        .world()
                        .position(*id)
                        .is_some_and(|position| position.x > 5.0)
            })
            .collect();
        assert!(group.len() >= 4, "the arena has a dummy group");

        let single = simulation
            .world()
            .entities()
            .find(|id| {
                simulation.world().kind(*id) == Some(EntityKind::Dummy) && !group.contains(id)
            })
            .expect("the arena has a single dummy too");

        simulation.player.target = Some(group[0]);
        let single_before = dummy_health(&simulation, single);
        let group_before: Vec<f32> = group
            .iter()
            .map(|id| dummy_health(&simulation, *id))
            .collect();

        // Walk the player into range of the group.
        let player_entity = simulation.player().entity;
        if let Some(position) = simulation.world().position(group[0])
            && let Some(body) = simulation.world_mut().body_mut(player_entity)
        {
            body.position = position;
        }

        let hits = simulation
            .use_ability(ability(DETONATE))
            .expect("the group is in range");
        assert!(hits >= 4, "the blast hit {hits} dummies");

        for (id, before) in group.iter().zip(&group_before) {
            assert!(
                dummy_health(&simulation, *id) < *before,
                "{id:?} was in the radius and took nothing"
            );
        }
        assert_eq!(
            dummy_health(&simulation, single),
            single_before,
            "a dummy outside the radius took damage"
        );
    }

    #[test]
    fn killing_a_dummy_emits_a_death_and_leaves_a_corpse() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is targeted");
        // A dummy with very little health, so one cast finishes it.
        if let Some(health) = simulation.world_mut().body_mut(target) {
            let _ = health;
        }
        simulation.world_mut().damage(target, 10_000.0);
        simulation.step_once();

        assert!(!simulation.world().is_alive(target));
        assert!(
            simulation.world().contains(target),
            "a corpse is still an entity"
        );
        // The target moves on by itself rather than staying on a dead dummy.
        assert_ne!(simulation.player().target, Some(target));
    }

    #[test]
    fn a_dead_target_cannot_be_hit_and_is_replaced_by_a_living_one() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is targeted");
        simulation.world_mut().damage(target, 10_000.0);
        simulation.step_once();

        let new_target = simulation.player().target.expect("another dummy exists");
        assert_ne!(new_target, target);
        assert!(simulation.world().is_alive(new_target));
    }

    #[test]
    fn a_simulation_with_no_hostiles_refuses_a_targeted_ability() {
        let mut simulation = Simulation::new(SimConfig::default());
        let player = simulation.world_mut().spawn(EntityDesc::player(Vec3::ZERO));
        simulation.set_player_entity(player);
        simulation.step_once();

        assert_eq!(
            simulation.use_ability(ability(FIREBALL)),
            Err(AbilityError::NoTarget)
        );
        assert_eq!(
            simulation.player().last_error,
            Some(AbilityError::NoTarget),
            "the refusal is reported for the overlay"
        );
    }

    #[test]
    fn commands_reach_the_simulation_and_move_the_character() {
        let mut simulation = arena();
        let start = simulation.player_position();

        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
        });
        simulation.step_many(60);

        let moved = simulation.player_position();
        assert!(moved.z < start.z - 1.0, "{start:?} -> {moved:?}");
    }

    #[test]
    fn a_jump_command_lifts_the_character_once_per_press() {
        let mut simulation = arena();
        simulation.push_command(PlayerCommand::Jump);
        simulation.step_once();

        let body = simulation
            .player_body()
            .copied()
            .expect("the player exists");
        assert!(!body.grounded);
        assert!(body.position.y > 0.0);

        // A catch-up batch of eight ticks must not become eight jumps.
        let mut second = arena();
        second.push_command(PlayerCommand::Jump);
        second.step_many(8);
        let apex = second.player_body().map_or(0.0, |body| body.position.y);
        assert!(
            apex < second.config().movement.jump_height() + 0.05,
            "one press produced a {apex} metre climb"
        );
    }

    #[test]
    fn a_use_ability_command_is_equivalent_to_calling_it() {
        let mut simulation = arena();
        simulation.push_command(PlayerCommand::UseAbility(ability(FIREBALL)));
        simulation.step_once();

        assert!(simulation.meter().total_damage() > 0.0);
        assert_eq!(simulation.player().last_ability, Some(ability(FIREBALL)));
    }

    #[test]
    fn the_reset_command_restores_the_dummies_and_the_session() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is targeted");
        simulation
            .use_ability(ability(IGNITE))
            .expect("the dummy is in range");
        simulation.step_many(60);
        assert!(simulation.meter().total_damage() > 0.0);

        simulation.push_command(PlayerCommand::ResetDummies);
        simulation.step_once();

        assert_eq!(simulation.meter().total_damage(), 0.0);
        assert_eq!(
            simulation.world().health(target).map(|h| h.current()),
            simulation.world().health(target).map(|h| h.max()),
            "the dummy is not at full health"
        );
        assert!(
            simulation
                .world()
                .statuses(target)
                .is_some_and(crate::status::StatusSet::is_empty),
            "the burn survived the reset"
        );
        assert!(simulation.player().abilities.is_ready(ability(IGNITE)));
    }

    #[test]
    fn cooldowns_advance_with_simulation_time_not_with_frames() {
        // Two runs of the same simulated second, delivered as very different frame rates.
        let mut fast = arena();
        let mut slow = arena();
        fast.use_ability(ability(DETONATE)).expect("in range");
        slow.use_ability(ability(DETONATE)).expect("in range");

        for _ in 0..300 {
            fast.advance(Duration::from_micros(3_333));
        }
        for _ in 0..30 {
            slow.advance(Duration::from_micros(33_333));
        }

        let fast_remaining = fast
            .player()
            .abilities
            .cooldown_remaining(ability(DETONATE));
        let slow_remaining = slow
            .player()
            .abilities
            .cooldown_remaining(ability(DETONATE));
        assert!(
            (fast_remaining - slow_remaining).abs() < 0.05,
            "{fast_remaining} vs {slow_remaining}"
        );
    }

    #[test]
    fn advancing_by_a_frame_delta_runs_whole_ticks_only() {
        let mut simulation = arena();
        let before = simulation.tick_count();
        assert_eq!(simulation.advance(Duration::from_micros(1_000)), 0);
        assert_eq!(simulation.tick_count(), before, "a partial tick never runs");

        // 101 ms of accumulated time is six whole 16.667 ms ticks with 1 ms left over.
        assert_eq!(simulation.advance(Duration::from_millis(100)), 6);
        assert_eq!(simulation.tick_count(), before + 6);
        assert!(simulation.timestep().alpha() > 0.0, "the leftover is kept");
    }

    #[test]
    fn the_events_of_a_whole_advance_stay_readable() {
        let mut simulation = arena();
        simulation.push_command(PlayerCommand::UseAbility(ability(IGNITE)));
        // Two simulated seconds: the cast, the status, and at least one burn tick. Driven
        // by `step_many` rather than one huge `advance`, because the timestep's catch-up
        // bound deliberately refuses to run two seconds of ticks in one frame.
        simulation.step_many(120);

        let events = simulation.events();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, CombatEvent::AbilityUsed { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, CombatEvent::StatusApplied { .. }))
        );
        assert!(
            events.iter().filter_map(CombatEvent::as_damage).count() > 1,
            "the impact and at least one burn tick"
        );
    }

    #[test]
    fn a_frame_that_runs_no_ticks_does_not_report_a_free_simulation() {
        // Four frames in five run no ticks at 300 FPS. Their timings are legitimately zero,
        // and a readout showing those would say the simulation costs nothing.
        let mut simulation = arena();
        simulation.advance(Duration::from_millis(20));
        let ticked = simulation.last_tick_stats();
        assert_eq!(ticked.ticks, 1);

        assert_eq!(simulation.advance(Duration::from_micros(500)), 0);
        assert_eq!(simulation.stats().ticks, 0, "this frame ran no ticks");
        assert_eq!(
            simulation.last_tick_stats(),
            ticked,
            "the last real tick's cost is what a readout shows"
        );
    }

    #[test]
    fn statistics_are_recorded_for_the_last_advance() {
        let mut simulation = arena();
        let ticks = simulation.advance(Duration::from_millis(100));
        let stats = simulation.stats();
        assert_eq!(stats.ticks, ticks);
        assert!(stats.ticks >= 5, "100 ms is five or six 16.667 ms ticks");
        assert!(stats.total_ms() >= 0.0);
        assert_eq!(stats.dropped_ticks, 0);
    }

    #[test]
    fn a_steady_state_simulation_stops_allocating() {
        // Milestone 25: the combat path reuses its buffers.
        let mut simulation = arena();
        for _ in 0..600 {
            simulation.push_command(PlayerCommand::UseAbility(ability(FIREBALL)));
            simulation.step_many(10);
        }
        let events = simulation.event_stream().capacity();
        let hits = simulation.hits.capacity();
        let outcomes = simulation.outcomes.capacity();
        let commands = simulation.pending.capacity();
        let targets = simulation.meter().target_capacity();

        for _ in 0..600 {
            simulation.push_command(PlayerCommand::UseAbility(ability(FIREBALL)));
            simulation.push_command(PlayerCommand::Move {
                forward: 1.0,
                right: 0.0,
                yaw: 0.5,
            });
            simulation.step_many(10);
        }

        assert_eq!(simulation.event_stream().capacity(), events);
        assert_eq!(simulation.hits.capacity(), hits);
        assert_eq!(simulation.outcomes.capacity(), outcomes);
        assert_eq!(simulation.pending.capacity(), commands);
        assert_eq!(simulation.meter().target_capacity(), targets);
    }
}
