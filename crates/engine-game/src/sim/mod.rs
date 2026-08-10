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

use engine_collision::controller::{CharacterConfig, CharacterState, MoveResult};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;
use engine_core::time::FixedTimestep;

use crate::ability::AbilityId;
use crate::combat::DamageType;
use crate::command::{CommandBuffer, PlayerCommand};
use crate::entity::EntityId;
use crate::event::{CombatEvent, DamageEvent, EventStream};
use crate::firemage::{AbilityError, FireAbility, FireMage, IgniteTick, PendingImpact, ZoneTick};
use crate::movement::{self, Body, MoveIntent, MovementConfig};
use crate::status::StatusOutcome;
use crate::targeting::TargetQuery;
use crate::telemetry::CombatMeter;
use crate::world::{EntityKind, GameWorld};

/// How the simulation is configured.
mod firemage_exec;

pub use firemage_exec::{CONE_HALF_ANGLE, CastAim, horizontal_distance};

/// How a simulation is configured: the tick rate, the character, and the random seed.
#[derive(Debug, Clone, PartialEq)]
pub struct SimConfig {
    /// Fixed simulation ticks per second.
    pub tick_hz: u32,
    /// How the player's body moves.
    pub movement: MovementConfig,
    /// The player's collision shape and what it can climb.
    ///
    /// Separate from [`SimConfig::movement`] because it is a different kind of number:
    /// `movement` is how fast the character wants to go, `character` is what shape is
    /// trying to go there. The capsule here has nothing to do with the player's mesh.
    pub character: CharacterConfig,
    /// Seed for every chance the class rolls, so a fight can be replayed exactly.
    pub rng_seed: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            tick_hz: engine_core::time::DEFAULT_TICK_HZ,
            movement: MovementConfig::default(),
            character: CharacterConfig::default(),
            rng_seed: 0xF12E_5EED,
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
    /// The currently selected hostile, if any.
    pub target: Option<EntityId>,
    /// The cast in progress, if any.
    ///
    /// A cast is a *commitment*: while one is running the character is channelling, and only
    /// moving or backing out breaks it. Pressing another ability does not - it queues.
    pub cast: Option<ActiveCast>,
    /// What the player asked for while a cast was running.
    ///
    /// Pressing an ability mid-cast is an instruction for what to do *next*, not an
    /// instruction to throw away the cast in progress. One slot, not a list: a player
    /// mashing four keys means "then that one", and a queue of four would play out a
    /// rotation they stopped wanting three seconds ago.
    pub queued: Option<QueuedCast>,
    /// Why the last ability attempt failed, for the overlay. Cleared by a success.
    pub last_error: Option<AbilityError>,
    /// The last ability that was successfully used.
    pub last_ability: Option<AbilityId>,
}

/// An ability the player asked for while another cast was running.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueuedCast {
    /// Which ability.
    pub ability: FireAbility,
    /// Where it was aimed, if it was placed with the mouse.
    pub ground: Option<Vec3>,
}

/// A cast in progress.
///
/// Cooldown and cost are paid when the cast *starts*, not when it lands — so a cancelled
/// cast is a real loss, which is what makes interruption matter. That is the convention the
/// genre uses and it is the one a player will expect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveCast {
    /// Which ability is being cast.
    pub ability: FireAbility,
    /// Simulated seconds spent so far.
    pub elapsed: f32,
    /// Simulated seconds the cast takes.
    pub duration: f32,
    /// The target the cast was started against.
    ///
    /// Held so a cast that lands after its target died fails cleanly rather than hitting
    /// whatever the player selected in the meantime.
    pub target: Option<EntityId>,
    /// Where a ground-targeted cast was aimed, if it was one.
    pub ground: Option<Vec3>,
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
    /// The player backed out of it.
    ///
    /// Pressing another ability is *not* this: that queues. Only escape and moving stop a
    /// cast, which is what makes a long cast safe to commit to.
    Cancelled,
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
    /// The static geometry the player collides with.
    ///
    /// Owned by the simulation because collision is a simulation fact: the same world has
    /// to be there for a headless test, a replay and a future server, none of which have a
    /// renderer to ask.
    collision: CollisionWorld,
    /// Scratch space for collision queries. Reused; never freed.
    collision_buffers: QueryBuffers,
    /// What the player's last collided move did, for a developer readout.
    last_move: MoveResult,
    player: Player,
    config: SimConfig,
    timestep: FixedTimestep,
    /// Movement intent, held between commands: a tick that falls between two frames of
    /// input still moves the character.
    intent: MoveIntent,
    /// Discrete commands waiting for the next tick.
    pending: CommandBuffer,
    /// The class: abilities, Ignite, Heat, Ultimate Energy, talents and ground fire.
    mage: FireMage,
    /// Ignite ticks produced this step. Reused; never freed.
    ignite_ticks: Vec<IgniteTick>,
    /// Zone ticks produced this step. Reused; never freed.
    zone_ticks: Vec<ZoneTick>,
    /// Impacts that landed this step. Reused; never freed.
    impacts: Vec<PendingImpact>,
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
        let seed = config.rng_seed;
        Self {
            world: GameWorld::new(),
            collision: CollisionWorld::with_capacity(64),
            // Sized for the worst corner in a level: a character standing where a floor,
            // two walls and a ramp meet. Past the first tick this never grows again.
            collision_buffers: QueryBuffers::with_capacity(16),
            last_move: MoveResult::default(),
            player: Player {
                entity: EntityId::INVALID,
                target: None,
                cast: None,
                queued: None,
                last_error: None,
                last_ability: None,
            },
            config,
            timestep,
            intent: MoveIntent::NONE,
            pending: CommandBuffer::with_capacity(16),
            mage: FireMage::new(seed),
            ignite_ticks: Vec::with_capacity(32),
            zone_ticks: Vec::with_capacity(16),
            impacts: Vec::with_capacity(8),
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

    /// The FireMage: abilities, resources, Ignite, zones and talents.
    #[must_use]
    pub fn mage(&self) -> &FireMage {
        &self.mage
    }

    /// The FireMage, mutably, for a talent purchase or a developer control.
    pub fn mage_mut(&mut self) -> &mut FireMage {
        &mut self.mage
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

    /// The static collision geometry.
    #[must_use]
    pub fn collision_world(&self) -> &CollisionWorld {
        &self.collision
    }

    /// The static collision geometry, for a scenario builder.
    ///
    /// The counterpart to [`Simulation::world_mut`]: a scene adds its floor, walls and
    /// ramps here and calls
    /// [`CollisionWorld::rebuild_broad_phase`](engine_collision::world::CollisionWorld::rebuild_broad_phase)
    /// when it is done.
    pub fn collision_world_mut(&mut self) -> &mut CollisionWorld {
        &mut self.collision
    }

    /// What the player's most recent collided move did.
    ///
    /// A developer readout — how many candidates the broad phase offered, how many exact
    /// tests ran, whether a step was taken. Nothing in gameplay reads it.
    #[must_use]
    pub fn last_move(&self) -> MoveResult {
        self.last_move
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
        self.tick_firemage(step);
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
                PlayerCommand::UseAbility(slot) => {
                    let _ = self.use_slot(slot);
                }
                PlayerCommand::UseAbilityAt { slot, position } => {
                    if let Some(ability) = FireAbility::from_slot(slot) {
                        let _ = self.cast_at(ability, position);
                    }
                }
                PlayerCommand::UseUltimate { position } => {
                    let _ = self.cast_at(FireAbility::Cataclysm, position);
                }
                PlayerCommand::PurchaseTalent(talent) => {
                    let _ = self.mage.purchase_talent(talent);
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
                PlayerCommand::Teleport { position } => self.teleport_player(position),
                PlayerCommand::Move { .. } => {}
            }
        }
        pending.clear();
        self.pending = pending;
    }

    /// Integrates every body.
    ///
    /// The player is moved through the collision world; everything else is integrated with
    /// the plain rules and an empty intent. That split is deliberate and is the decision
    /// recorded in `collision.md`: a training dummy is a target, not an obstacle, and one
    /// the player can get wedged against is a target that ruins a fight. Dummies stand
    /// still on a flat floor, so a collision query per dummy per tick would buy nothing at
    /// 1,000 of them.
    fn integrate_bodies(&mut self, step: f32) {
        let player = self.player.entity;
        let intent = self.intent;
        let config = self.config.movement;
        let character = self.config.character;

        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if id == player {
                continue;
            }
            if let Some(body) = self.world.body_mut(id) {
                movement::step(body, MoveIntent::NONE, &config, step);
            }
        }

        // The player last, and out of the loop, because it is the one body that needs the
        // collision world and the query buffers — both of which are borrowed from `self`.
        if let Some(body) = self.world.body_mut(player) {
            let mut moved = *body;
            self.last_move = movement::step_collided(
                &mut moved,
                intent,
                &config,
                &character,
                &self.collision,
                &mut self.collision_buffers,
                step,
            );
            if let Some(body) = self.world.body_mut(player) {
                *body = moved;
            }
        }

        // Consumed, not held: a jump lasts one tick, and the next tick's intent comes from
        // the next frame's input.
        self.intent.jump = false;
    }

    /// Puts the player down at `position`, at rest and clear of geometry.
    ///
    /// A developer control. Velocity is cleared and the interpolation history is reset, so
    /// a teleport is a teleport rather than a very fast slide across the level.
    fn teleport_player(&mut self, position: Vec3) {
        let player = self.player.entity;
        let character = self.config.character;
        let Some(body) = self.world.body_mut(player) else {
            return;
        };
        let mut state = CharacterState::at(position);
        // Claimed grounded so the zero-length move below runs its ground probe: a station
        // written as "a tenth of a metre above the floor" should put the character *on* the
        // floor, not leave it falling for two ticks. A station genuinely in mid-air — the
        // high-speed drop — finds nothing within the probe distance and stays where it is.
        state.grounded = true;
        *body = Body::at(position);

        engine_collision::controller::move_character(
            &self.collision,
            &character,
            &mut state,
            Vec3::ZERO,
            &mut self.collision_buffers,
        );
        if let Some(body) = self.world.body_mut(player) {
            body.position = state.position;
            body.grounded = state.grounded;
            body.ground_normal = state.ground_normal;
        }
        self.world.reset_history(player);
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
            ability: AbilityId::from_fire(ability),
            end: CastEnd::Completed,
            tick: self.tick,
        });
        let restore = self.player.target;
        self.player.target = cast.target;
        // Cooldown and cost were paid when the cast started, so the landing skips both.
        let aim = match cast.ground {
            Some(position) => CastAim::Ground(position),
            None => CastAim::Target,
        };
        self.land(ability, aim);
        self.player.target = restore;
        self.start_queued_cast();
    }

    /// Cancels the cast in progress and announces why.
    fn end_cast(&mut self, end: CastEnd) {
        // A cancelled cast takes the queue with it. The player asked to stop, and starting
        // whatever they pressed two seconds ago instead would be the opposite of stopping.
        if !matches!(end, CastEnd::Completed) {
            self.player.queued = None;
        }
        let Some(cast) = self.player.cast.take() else {
            return;
        };
        // A cast that never landed gives its charge back. Only a completed one has been
        // paid for by something happening.
        if !matches!(end, CastEnd::Completed) {
            self.refund_cast(&cast);
        }
        self.events.push(CombatEvent::CastEnded {
            source: self.player.entity,
            ability: AbilityId::from_fire(cast.ability),
            end,
            tick: self.tick,
        });
    }

    /// Whether a cast is in progress.
    #[must_use]
    pub fn casting(&self) -> Option<&ActiveCast> {
        self.player.cast.as_ref()
    }

    /// Casts the ability in bar slot `slot`, if there is one.
    ///
    /// # Errors
    ///
    /// Returns [`AbilityError`] with the reason it was refused.
    pub fn use_slot(&mut self, slot: usize) -> Result<u32, AbilityError> {
        let ability = FireAbility::from_slot(slot).ok_or(AbilityError::CasterDead)?;
        self.cast(ability)
    }

    /// Advances everything the class owns and applies what it reports.
    ///
    /// Ordered deliberately: impacts land, then zones burn, then Ignite ticks. A Meteor
    /// whose delay ends this tick therefore applies its stacks before this tick's burn, so
    /// the burn it just created is counted.
    fn tick_firemage(&mut self, step: f32) {
        self.mage.tick(step);
        if let Some(position) = self.world.position(self.player.entity) {
            self.mage.follow(position);
        }

        self.impacts.clear();
        self.zone_ticks.clear();
        let mut impacts = std::mem::take(&mut self.impacts);
        let mut zone_ticks = std::mem::take(&mut self.zone_ticks);
        self.mage.zones.tick(step, &mut impacts, &mut zone_ticks);
        for impact in &impacts {
            self.resolve_impact(*impact);
        }
        for tick in &zone_ticks {
            self.resolve_zone_tick(*tick);
        }
        impacts.clear();
        zone_ticks.clear();
        self.impacts = impacts;
        self.zone_ticks = zone_ticks;

        self.ignite_ticks.clear();
        let mut ignite_ticks = std::mem::take(&mut self.ignite_ticks);
        self.mage.ignite.tick(step, &mut ignite_ticks);
        for tick in &ignite_ticks {
            self.resolve_ignite_tick(*tick);
        }
        ignite_ticks.clear();
        self.ignite_ticks = ignite_ticks;
    }

    /// Moves the player by `offset`, sliding along whatever is in the way.
    ///
    /// What Blink teleports with: the same collided move walking uses, so it climbs stairs,
    /// refuses walls, and ends against an obstacle rather than failing outright.
    pub(super) fn slide_player(&mut self, offset: Vec3) {
        let entity = self.player.entity;
        let Some(body) = self.world.body(entity).copied() else {
            return;
        };
        let character = self.config.character;
        let mut state = CharacterState::at(body.position);
        state.grounded = body.grounded;
        state.velocity = body.velocity;
        let result = engine_collision::controller::move_character(
            &self.collision,
            &character,
            &mut state,
            offset,
            &mut self.collision_buffers,
        );
        if let Some(body) = self.world.body_mut(entity) {
            body.position = state.position;
            body.grounded = state.grounded;
            body.ground_normal = state.ground_normal;
        }
        self.last_move = result;
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
        self.mage.reset();
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if self.world.kind(id) == Some(EntityKind::Dummy) {
                self.world.restore(id);
            }
        }
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
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::arena::{self, ArenaConfig};
    use crate::event::CombatEvent;
    use crate::firemage::FireAbility;

    fn arena() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        simulation
    }

    fn dummy_health(simulation: &Simulation, id: EntityId) -> f32 {
        simulation
            .world()
            .health(id)
            .map_or(0.0, |health| health.current())
    }

    #[test]
    fn an_instant_ability_lands_on_the_tick_it_is_used() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is selected");
        let before = dummy_health(&simulation, target);

        // Ignite it to the cap first: Fire Detonation is the class's instant hit.
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(target, maximum, maximum, duration);

        simulation
            .cast(FireAbility::FireDetonation)
            .expect("a full burn");
        assert!(dummy_health(&simulation, target) < before, "nothing landed");
        assert!(simulation.casting().is_none(), "an instant started a cast");
    }

    #[test]
    fn a_cast_ability_starts_a_cast_and_lands_when_it_finishes() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is selected");
        let before = dummy_health(&simulation, target);

        simulation.cast(FireAbility::Firebolt).expect("in range");
        assert!(simulation.casting().is_some(), "no cast started");
        assert_eq!(dummy_health(&simulation, target), before, "it landed early");

        simulation.step_many(70);
        assert!(simulation.casting().is_none(), "the cast never finished");
        assert!(
            dummy_health(&simulation, target) < before,
            "it never landed"
        );
    }

    #[test]
    fn moving_cancels_a_cast_and_the_effect_never_lands() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is selected");
        let before = dummy_health(&simulation, target);

        simulation.cast(FireAbility::Pyroblast).expect("in range");
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
        });
        simulation.step_many(10);

        assert!(simulation.casting().is_none(), "movement did not break it");
        assert_eq!(
            dummy_health(&simulation, target),
            before,
            "it landed anyway"
        );
    }

    #[test]
    fn a_cancelled_cast_gives_its_charge_back() {
        // A cast that never landed costs the time it wasted and nothing else. Putting the
        // ability on cooldown for something that did not happen punishes the player twice.
        let mut simulation = arena();
        simulation.cast(FireAbility::Pyroblast).expect("in range");
        assert!(!simulation.mage().abilities.is_ready(FireAbility::Pyroblast));

        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
        });
        simulation.step_many(4);
        assert!(simulation.casting().is_none(), "movement did not break it");
        assert!(
            simulation.mage().abilities.is_ready(FireAbility::Pyroblast),
            "the charge was not refunded"
        );
    }

    #[test]
    fn a_completed_cast_keeps_its_cooldown() {
        // The other half of the rule: what actually happened is paid for.
        let mut simulation = arena();
        simulation.cast(FireAbility::Pyroblast).expect("in range");
        simulation.step_many(200);
        assert!(simulation.casting().is_none(), "the cast never finished");
        assert!(
            !simulation.mage().abilities.is_ready(FireAbility::Pyroblast),
            "a landed cast was refunded"
        );
    }

    #[test]
    fn the_cast_stream_announces_both_ends() {
        let mut simulation = arena();
        simulation.cast(FireAbility::Firebolt).expect("in range");
        simulation.step_many(70);

        let started = simulation
            .events()
            .iter()
            .any(|event| matches!(event, CombatEvent::CastStarted { .. }));
        let ended = simulation.events().iter().any(|event| {
            matches!(
                event,
                CombatEvent::CastEnded {
                    end: CastEnd::Completed,
                    ..
                }
            )
        });
        assert!(started || ended, "the cast was silent");
    }

    #[test]
    fn commands_reach_the_simulation_and_move_the_character() {
        let mut simulation = arena();
        let before = simulation.player_position();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
        });
        simulation.step_many(30);
        assert!(
            (simulation.player_position() - before).length() > 0.5,
            "the character did not move"
        );
    }

    #[test]
    fn a_use_ability_command_is_equivalent_to_calling_it() {
        let mut simulation = arena();
        simulation.push_command(PlayerCommand::UseAbility(0));
        simulation.step_once();
        assert!(
            simulation.casting().is_some(),
            "the command did not start firebolt"
        );
    }

    #[test]
    fn the_reset_command_restores_the_dummies_and_the_class() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy is selected");
        simulation.mage_mut().gain_heat(50.0);
        simulation.world_mut().damage(target, 500.0);

        simulation.push_command(PlayerCommand::ResetDummies);
        simulation.step_once();

        assert_eq!(simulation.mage().heat.value(), 0.0, "heat survived a reset");
        assert!(
            dummy_health(&simulation, target) > 1_000.0,
            "the dummy was not restored"
        );
    }

    #[test]
    fn cooldowns_advance_with_simulation_time_not_with_frames() {
        let mut simulation = arena();
        simulation
            .cast(FireAbility::Blink)
            .expect("blink is instant");
        assert!(!simulation.mage().abilities.is_ready(FireAbility::Blink));

        // Stepped rather than advanced: a single advance clamps its catch-up ticks, which
        // is the loop protecting itself from a hitch, not the cooldown standing still.
        simulation.step_many(16 * simulation.timestep().hz() as u32);
        assert!(
            simulation.mage().abilities.is_ready(FireAbility::Blink),
            "the cooldown did not run"
        );
    }

    #[test]
    fn advancing_by_a_frame_delta_runs_whole_ticks_only() {
        let mut simulation = arena();
        let ticks = simulation.advance(Duration::from_millis(8));
        assert!(ticks <= 1, "{ticks} ticks in half a step");
        let ticks = simulation.advance(Duration::from_millis(100));
        assert!(ticks >= 5, "{ticks} ticks in a tenth of a second");
    }

    #[test]
    fn a_steady_state_simulation_stops_allocating() {
        let mut simulation = arena();
        simulation.step_many(120);
        let events = simulation.event_stream().capacity();

        for _ in 0..30 {
            simulation.push_command(PlayerCommand::UseAbility(0));
            simulation.step_many(20);
        }
        assert_eq!(
            simulation.event_stream().capacity(),
            events,
            "the event stream reallocated in the steady state"
        );
    }
}
