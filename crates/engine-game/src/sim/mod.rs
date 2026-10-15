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

use std::time::Duration;

use engine_collision::controller::{CharacterConfig, CharacterState, MoveResult};
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;
use engine_core::time::FixedTimestep;

use crate::ability::AbilityId;
use crate::character::spirit::SpiritSurge;
use crate::character::stat::{Stat, health_from_stamina};
use crate::character::{CharacterSheet, ItemSlot, MoveError, Roster};
use crate::combat::DamageType;
use crate::command::{CommandBuffer, PlayerCommand};
use crate::druid::Druid;
use crate::enemy::EnemyKind;
use crate::enemy::brain::EnemyBrain;
use crate::entity::EntityId;
use crate::event::{CombatEvent, DamageEvent, EventStream};
use crate::firemage::{AbilityError, FireAbility, FireMage, IgniteTick, PendingImpact, ZoneTick};
use crate::hero::Hero;
use crate::los::LineOfSightCheck;
use crate::movement::{self, Body, MoveIntent, MovementConfig};
use crate::party::Party;
use crate::status::StatusOutcome;
use crate::targeting::TargetQuery;
use crate::telemetry::CombatMeter;
use crate::warrior::Warrior;
use crate::world::{EntityKind, GameWorld};

/// How the simulation is configured.
/// Casting the Druid: Mana, Astral Power, the empowerment and the channel.
pub mod druid_exec;
/// A dungeon run's simulation half: sealing, crediting, the gate and the boss's adds.
pub mod dungeon_exec;
/// Effects applied and removed by name, with the registry's numbers; what they make of a body.
pub mod effect_exec;
/// Enemy brains, their attacks and their telegraphs: the combat half of the AI.
pub mod enemy_exec;
mod firemage_exec;
/// The boss's chest: dropped on dungeon completion, one rolled item per player.
pub mod loot;
/// Dungeon modifiers, run: engaged packs light beams, dead monsters fester.
mod modifier_exec;
/// More than one player in one simulation: per-player state swapped in and out.
pub mod players;
/// The test bed's controls: reset the player, dismiss chosen enemies, find standing ground.
mod playground_exec;
/// A networked client's simulation: the server runs the fight, this one presents it.
pub mod remote;
pub mod respawn;
/// Casting the Warrior: Valor, the incoming-damage pipeline and the charge.
mod warrior_exec;

pub use loot::{
    CHEST_DROP_HEIGHT, CHEST_FALL_SECONDS, CHEST_INTERACT_RANGE, ChestClaim, LootChest,
};

pub use firemage_exec::{
    CONE_HALF_ANGLE, CastAim, DETONATE_FRACTION, FireCounters, horizontal_distance,
};

/// The share of Versatility that also reduces damage taken: half.
///
/// A player with Versatility `v` deals and heals `1 + v` as much and takes `1 - v / 2`.
pub const VERSATILITY_MITIGATION_SHARE: f32 = 0.5;

/// How long the player may be below the world before the fall is fatal, in seconds.
///
/// Ten. Long enough that nothing survivable trips it — there is no way to be under the level
/// for ten seconds and come back — and short enough that a fall through a hole an author left
/// is a death and a respawn rather than a session that has to be restarted.
pub const VOID_KILL_SECONDS: f32 = 10.0;

/// The void height a simulation has until a level tells it otherwise.
///
/// Far enough down that a layout with a floor plane at zero — the training arena, every test
/// bed, every benchmark — can never reach it, so nothing that was built before levels had
/// terrain can die of falling.
pub const DEFAULT_VOID_HEIGHT: f32 = -10_000.0;

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
    /// Health the player regains per second out of combat, as a fraction of their maximum.
    ///
    /// Zero turns regeneration off entirely. The player can set this in `settings.cfg`; the
    /// default is [`engine_core::tuning::player::HEALTH_REGEN_PER_SECOND`].
    pub health_regen_per_second: f32,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            tick_hz: engine_core::time::DEFAULT_TICK_HZ,
            movement: MovementConfig::default(),
            character: CharacterConfig::default(),
            rng_seed: engine_core::tuning::simulation::RNG_SEED,
            health_regen_per_second: engine_core::tuning::player::HEALTH_REGEN_PER_SECOND,
        }
    }
}

/// How long a refusal stays on screen, in seconds.
pub const NOTIFICATION_SECONDS: f64 = engine_core::tuning::simulation::NOTIFICATION_SECONDS;

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
    /// Why the last ability attempt failed, for the interface. Cleared by a success.
    pub last_error: Option<AbilityError>,
    /// When that refusal happened, in simulation seconds.
    ///
    /// A message about a mis-click three minutes ago is furniture. The time is kept beside
    /// the reason so the interface can stop showing it without a timer of its own.
    pub last_error_at: f64,
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
    /// The target went out of line of sight before it landed.
    ///
    /// Its own reason rather than [`CastEnd::Interrupted`]: a player who walked their
    /// target behind a pillar has to be told which of the two things happened, and a cast
    /// bar that says "interrupted" for both teaches the wrong lesson.
    LineOfSightLost,
    /// Something interrupted it.
    ///
    /// Only enemy casts can reach this today, because the player has nothing that interrupts
    /// them. The reason still belongs on the shared enum rather than on a second one: a cast
    /// bar that has to know whose cast it is watching is a cast bar with two code paths.
    Interrupted,
}

/// When a line-of-sight check was run.
///
/// A targeted ability with a cast time is checked twice — once when the player presses it
/// and once when it lands — and a developer looking at a refusal has to know which of the
/// two they are reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOfSightPhase {
    /// When the ability was activated.
    Activation,
    /// When a cast finished, immediately before its effect landed.
    CastCompletion,
}

/// The last line-of-sight question a targeted ability asked, kept for the debug view.
///
/// Developer tooling. Nothing in gameplay reads it: it is written where the rule is applied
/// so that the debug drawing and the readout report the query that was actually run, rather
/// than a second query performed on their behalf that could disagree with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineOfSightDebug {
    /// Which ability asked.
    pub ability: FireAbility,
    /// What it was pointed at.
    pub target: EntityId,
    /// Whether that ability requires line of sight at all.
    pub required: bool,
    /// Whether the target was within the ability's range when the question was asked.
    pub in_range: bool,
    /// When the question was asked.
    pub phase: LineOfSightPhase,
    /// The answer, absent only if one of the two entities had no position.
    pub check: Option<LineOfSightCheck>,
    /// The tick it was asked on.
    pub tick: u64,
}

impl LineOfSightDebug {
    /// Whether the target was reachable.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.check.is_some_and(|check| check.visible)
    }

    /// The blocking object's label, or an empty string when nothing blocked.
    #[must_use]
    pub fn blocker_label(&self) -> &'static str {
        self.check.map_or("", |check| check.blocker_label)
    }
}

/// The profiling clock behind [`SimStats`].
///
/// `std::time::Instant::now` panics on `wasm32-unknown-unknown`, which is where the realm
/// server runs the shared gameplay code. The numbers are telemetry only — the simulation's
/// behaviour never reads them — so there they read zero and nothing else changes.
#[derive(Clone, Copy)]
struct Stopwatch {
    #[cfg(not(target_family = "wasm"))]
    started: std::time::Instant,
}

impl Stopwatch {
    fn start() -> Self {
        Self {
            #[cfg(not(target_family = "wasm"))]
            started: std::time::Instant::now(),
        }
    }

    #[cfg(not(target_family = "wasm"))]
    fn elapsed(self) -> Duration {
        self.started.elapsed()
    }

    #[cfg(target_family = "wasm")]
    fn elapsed(self) -> Duration {
        Duration::ZERO
    }
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
    /// Of which: every enemy's brain, and the attacks they asked for.
    pub ai: Duration,
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
    /// How monsters find their way through `collision`: what it has learned about the ground,
    /// forgotten whenever the geometry is handed out for editing.
    nav: crate::nav::Navigator,
    /// Scratch space for collision queries. Reused; never freed.
    collision_buffers: QueryBuffers,
    /// What the player's last collided move did, for a developer readout.
    last_move: MoveResult,
    /// The last line-of-sight check a targeted ability ran, for the debug view.
    ///
    /// One slot, overwritten. A history would be a log, and the question a developer asks
    /// is always about the cast that just failed.
    last_los: Option<LineOfSightDebug>,
    player: Player,
    /// Where the player comes back, and which way they face when they do.
    ///
    /// The level's answer, carried into the simulation at load — see
    /// `engine_environment::spawn`. `None` for a simulation nobody gave a level to, which is
    /// most tests and every benchmark: those respawn the player where they first stood, which
    /// is the same place a level with no spawn point would have put them.
    player_spawn: Option<(Vec3, f32)>,
    /// When the player comes back, and whether they are allowed to yet.
    respawn: respawn::Respawn,
    config: SimConfig,
    /// Below this y the player has left the level. See [`Simulation::set_void_height`].
    void_height: f32,
    /// How long the player has been below it, in seconds.
    void_seconds: f32,
    timestep: FixedTimestep,
    /// Movement intent, held between commands: a tick that falls between two frames of
    /// input still moves the character.
    intent: MoveIntent,
    /// Discrete commands waiting for the next tick.
    pending: CommandBuffer,
    /// Which hero the player is: which abilities the bar holds and which model they wear.
    ///
    /// The class *state* below is the FireMage's and stays whatever the player is — Heat and
    /// Ignite simply do not move for a Warrior, because no Warrior row generates any. One
    /// resource block that the other two heroes leave at rest is a great deal less to go
    /// wrong than three that have to be kept in step.
    hero: Hero,
    /// One character sheet per hero: what each is wearing and carrying.
    ///
    /// Per hero rather than one shared sheet, because a hero swap is a change of character
    /// and a character's gear is theirs. The sheet of the hero the player currently is, is
    /// what every derived number in this struct reads.
    roster: Roster,
    /// The Spirit buff's stacks and their clocks.
    ///
    /// Inert on the current roster — nothing grants Spirit, so nothing can proc — and
    /// ticked anyway, so enabling the stat is a change to the item table and nowhere here.
    spirit: SpiritSurge,
    /// The class: abilities, Ignite, Heat, Ultimate Energy, talents and ground fire.
    mage: FireMage,
    /// Stats granted to the player on top of their sheet and the Spirit buff.
    ///
    /// What a *temporary* grant looks like: a scripted encounter's buff, a developer
    /// control, or a test putting a character in the gear it wants to measure. Added in
    /// [`Simulation::character_stats`] with everything else rather than written into the
    /// sheet, because the sheet is what the character owns and this is not.
    stat_bonus: crate::character::StatBlock,
    /// The last incoming attack's outcome, for the debug readout and for a test.
    ///
    /// One slot, overwritten, exactly like [`Simulation::last_line_of_sight`]: the question
    /// a developer asks is always about the swing that just landed.
    last_defense: Option<crate::character::DefenseOutcome>,
    /// The Warrior: Valor, its timers, its absorb and its banked pool.
    ///
    /// Beside the FireMage rather than instead of it, for the reason the FireMage's own
    /// state sits here while a Druid is being played: the class that is not being played
    /// costs a handful of floats and is ticked to rest, and one block of state per class
    /// that is always present is a great deal less to go wrong than a block that appears
    /// and disappears with the hero.
    warrior: Warrior,
    /// The Druid: Mana, Astral Power and the channel.
    ///
    /// Beside the other two for the reason they are beside each other: one block of state
    /// per class that is always present is a great deal less to go wrong than a block that
    /// appears and disappears with the hero.
    druid: Druid,
    /// Whether a damage-to-healing conversion is already running.
    ///
    /// Astral Sunfire's guard against feeding itself. Healing is not damage and cannot
    /// re-enter the damage path today, so this is belt and braces for a future effect that
    /// turns one into the other — and it costs a bool.
    converting: bool,
    /// What the class did this fight, for the combat simulator's report.
    ///
    /// Counts only. Nothing in gameplay reads them, and they are cleared with the dummies.
    fire: FireCounters,
    /// Ignite ticks produced this step. Reused; never freed.
    ignite_ticks: Vec<IgniteTick>,
    /// Zone ticks produced this step. Reused; never freed.
    zone_ticks: Vec<ZoneTick>,
    /// Impacts that landed this step. Reused; never freed.
    impacts: Vec<PendingImpact>,
    /// Zones that burned out this step, with their identities. Reused; never freed.
    expired_zones: Vec<(u32, crate::firemage::Zone)>,
    /// Every enemy's brain, indexed by entity slot. `None` where the slot is not an enemy.
    ///
    /// A parallel array like every other per-entity store in this crate: an enemy is an
    /// entity that happens to have a mind, not a different kind of object.
    enemies: Vec<Option<enemy_exec::EnemyRuntime>>,
    /// The fights the enemies belong to, in the order the scene built them.
    ///
    /// A list rather than one, because a dungeon is a list of encounters and the alternative
    /// — an encounter that holds sub-encounters — puts pack membership inside the thing that
    /// is supposed to answer "who is in this pack".
    encounters: Vec<crate::encounter::Encounter>,
    /// Ground enemies have left burning.
    hazards: crate::enemy::hazard::EnemyHazardField,
    /// Hazard damage produced this step. Reused; never freed.
    hazard_ticks: Vec<crate::enemy::hazard::HazardTick>,
    /// Hazards that stopped burning this step, with their identities. Reused; never freed.
    hazard_expired: Vec<(u32, Vec3)>,
    /// Identities for telegraphs, so an end can be matched to its start.
    next_telegraph: u32,
    /// Zones laid by a cast that land later: marks, rains, quakes, shockwaves.
    strikes: crate::enemy::strike::StrikeField,
    /// Strikes landed, dropped or removed this step. Reused; never freed.
    strike_scratch: Vec<crate::enemy::strike::PendingStrike>,
    /// Who a chained detonation caught, while their marks are laid. Reused; never freed.
    chain_scratch: Vec<EntityId>,
    /// Who the last area an enemy resolved caught. Reused; never freed.
    hits_last: Vec<EntityId>,
    /// Hazards that spread this step. Reused; never freed.
    hazard_grown: Vec<crate::enemy::hazard::EnemyHazard>,
    /// The enemies' own random generator: target picks, scatter, the boss's adds.
    ///
    /// Seeded from [`SimConfig::rng_seed`] and advanced only by enemy decisions, so the
    /// player's own critical strikes and an enemy's mark cannot disturb each other's
    /// sequence, and a replay of one seed is a replay of the whole fight.
    enemy_rng: u64,
    /// The dungeon run, if this simulation is a dungeon with a completion requirement.
    dungeon: Option<crate::dungeon_run::DungeonRun>,
    /// Kills waiting to be credited towards completion. Reused; never freed.
    pending_credit: Vec<EntityId>,
    /// The boss gate's collision, while it stands.
    gate_static: Option<engine_collision::world::StaticId>,
    /// The chest the finished dungeon left behind, until a reset or a new level clears it.
    chest: Option<loot::LootChest>,
    /// The last chest identity handed out, so a second dungeon's chest is a new chest.
    next_chest: u32,
    /// Added to simulation time to stamp a letter. See `Simulation::set_mail_epoch`.
    mail_epoch: f64,
    /// The data-driven loot rules, once a host has set them. `None` keeps the legacy chest:
    /// one uniform pick from the built-in pool per player, and no monster drops.
    loot_rules: Option<crate::loot::LootRules>,
    /// The dungeon key level the loot is rolled at.
    loot_key_level: u32,
    /// The run's reward bonus, per cent. See `Simulation::set_loot_rules`.
    loot_bonus_pct: u32,
    /// The loot's own generator. Separate from the enemies' so that a monster dropping an item
    /// (or not) can never change which player a mark finds or which adds the boss calls.
    loot_rng: crate::firemage::Rng,
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
    /// Who the player is grouped with. The local player is always its first member.
    party: Party,
    /// Every player but the active one. See [`players`].
    others: Vec<Option<players::PlayerSlot>>,
    /// Players whose bodies are moved from outside the tick (a server applying input frames).
    externally_moved: Vec<EntityId>,
    /// A server runs the fight; see [`remote`].
    remote_authority: bool,
    /// The dungeon modifiers in force and the ground they have put down. See [`crate::modifier`].
    modifiers: crate::modifier::ModifierRuntime,
    /// Set for the length of a [`Simulation::slay`] or a dungeon modifier's hit: the hit skips
    /// every multiplier, so "kill" is not turned into "nearly kill" by the target's Versatility
    /// or a damage reduction, and "5 % of your health" is 5 %.
    slaying: bool,
    /// Absorbs the authority reported, per entity, while it is the authority.
    ///
    /// A client under [`Simulation::set_remote_authority`] runs no Puncture Blood of its own;
    /// the server's number is laid in here and [`Simulation::absorb_of`] reads it.
    presented_absorbs: Vec<(EntityId, f32)>,
    /// Ability presses for the server, while it runs the fight.
    remote_requests: Vec<PlayerCommand>,
    /// Seconds of out-of-combat time since the last regeneration tick.
    ///
    /// Regeneration happens once a second rather than every simulation tick: one heal of a
    /// readable size, announced once, beats sixty of a sixtieth each — which would be sixty
    /// events, sixty floating numbers and a line in the damage meter for every second the
    /// player spends walking.
    regen_elapsed: f32,
    /// Set while damage over time is being applied, so the damage it deals is announced as a
    /// tick rather than a hit.
    ///
    /// Scoped rather than threaded through as an argument: a burn reaches `deal_damage`
    /// through the same `strike` every hit does, and a flag on the seventeen callers of that
    /// would be seventeen places to pass `false`. Only ever true inside the two loops that
    /// tick statuses and burns, and false the rest of the time.
    periodic_damage: bool,
    /// A maximum health that replaces the character sheet's, for a test scene.
    ///
    /// The monster test bed gives the player a million so a boss can be watched through every
    /// phase without the watcher dying; the stat path and the Warrior's multipliers would
    /// otherwise put the number back on the next refresh.
    health_override: Option<f32>,
    /// The character-sheet stats the player's effects gave them when last looked at; a
    /// change is what makes the derived numbers be recomputed. See `sync_effect_stats`.
    effect_stat_bonus: crate::character::StatBlock,
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
            nav: crate::nav::Navigator::new(crate::nav::NavConfig::for_character(
                &config.character,
            )),
            // Sized for the worst corner in a level: a character standing where a floor,
            // two walls and a ramp meet. Past the first tick this never grows again.
            collision_buffers: QueryBuffers::with_capacity(16),
            last_move: MoveResult::default(),
            last_los: None,
            // No level has been loaded yet, so a respawn puts the player back where they are.
            // Whatever builds a level calls `set_player_spawn` and this becomes the level's.
            player_spawn: None,
            respawn: respawn::Respawn::new(),
            void_height: DEFAULT_VOID_HEIGHT,
            void_seconds: 0.0,
            player: Player {
                entity: EntityId::INVALID,
                target: None,
                cast: None,
                queued: None,
                last_error_at: 0.0,
                last_error: None,
                last_ability: None,
            },
            config,
            timestep,
            intent: MoveIntent::NONE,
            pending: CommandBuffer::with_capacity(16),
            hero: Hero::default(),
            roster: Roster::starting(),
            spirit: SpiritSurge::new(),
            mage: FireMage::new(seed),
            warrior: Warrior::new(),
            druid: Druid::new(),
            converting: false,
            stat_bonus: crate::character::StatBlock::zero(),
            last_defense: None,
            fire: FireCounters::default(),
            ignite_ticks: Vec::with_capacity(32),
            zone_ticks: Vec::with_capacity(16),
            impacts: Vec::with_capacity(8),
            expired_zones: Vec::with_capacity(8),
            enemies: Vec::new(),
            encounters: Vec::new(),
            hazards: crate::enemy::hazard::EnemyHazardField::new(),
            hazard_ticks: Vec::with_capacity(16),
            hazard_expired: Vec::with_capacity(16),
            next_telegraph: 0,
            strikes: crate::enemy::strike::StrikeField::new(),
            strike_scratch: Vec::with_capacity(16),
            chain_scratch: Vec::with_capacity(8),
            hits_last: Vec::with_capacity(16),
            hazard_grown: Vec::with_capacity(8),
            // Never zero: xorshift stays at zero forever from a zero state.
            enemy_rng: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            dungeon: None,
            pending_credit: Vec::with_capacity(8),
            gate_static: None,
            chest: None,
            next_chest: 0,
            mail_epoch: 0.0,
            loot_rules: None,
            loot_key_level: 1,
            loot_bonus_pct: 0,
            loot_rng: crate::firemage::Rng::new(seed ^ 0x4C4F_4F54_5F52_4E47),
            events: EventStream::with_capacity(256),
            meter: CombatMeter::new(),
            targets: TargetQuery::with_capacity(64),
            hits: Vec::with_capacity(64),
            outcomes: Vec::with_capacity(16),
            observed: 0,
            regen_elapsed: 0.0,
            periodic_damage: false,
            health_override: None,
            effect_stat_bonus: crate::character::StatBlock::default(),
            tick: 0,
            time: 0.0,
            stats: SimStats::default(),
            last_active_stats: SimStats::default(),
            party: Party::new(),
            others: Vec::new(),
            externally_moved: Vec::new(),
            remote_authority: false,
            modifiers: crate::modifier::ModifierRuntime::new(),
            slaying: false,
            presented_absorbs: Vec::new(),
            remote_requests: Vec::new(),
        }
    }

    /// Which hero the player is.
    #[must_use]
    pub fn hero(&self) -> Hero {
        self.hero
    }

    /// Keeps only this frame's events that `keep` accepts. For a networked client: outcomes
    /// it predicted are dropped, and the authoritative ones are added with
    /// [`Simulation::inject_event`].
    pub fn retain_events(&mut self, keep: impl FnMut(&CombatEvent) -> bool) {
        self.events.retain(keep);
        self.observed = self.observed.min(self.events.len());
    }

    /// Adds an event that happened elsewhere — on the authoritative server — to this frame's
    /// stream, so presentation (effects, sound, numbers, the meter) observes it like a local
    /// one.
    pub fn inject_event(&mut self, event: CombatEvent) {
        self.observe_modifier_event(&event);
        self.events.push(event);
    }

    /// Becomes `hero`, and returns whether anything changed.
    ///
    /// A swap is a *reset of what the player is holding*, not of the world: the fight, the
    /// enemies and the player's position and health are all untouched, and every cooldown,
    /// the cast in progress, the burn the old hero banked and the fire it left on the ground
    /// are not. Keeping a half-finished Pyroblast running while its caster became a Warrior
    /// is the one outcome that is certainly wrong, and the cheapest way to be sure of the
    /// rest is to hand the new hero a clean class state.
    pub fn set_hero(&mut self, hero: Hero) -> bool {
        if self.hero == hero {
            return false;
        }
        self.hero = hero;
        self.player.cast = None;
        self.player.queued = None;
        self.player.last_ability = None;
        self.player.last_error = None;
        self.mage.reset();
        self.warrior.reset();
        self.druid.reset();
        // The old hero's own effects go with them: a Wild Rush on a character who has just
        // become a Warrior is a buff nobody can explain.
        if let Some(statuses) = self.world.statuses_mut(self.player.entity) {
            statuses.clear();
        }
        // The new hero is a different character with different gear, so their health and
        // their stats are the new sheet's. Cleared with the class state for the same reason
        // a half-finished cast is: the buff was the old character's.
        self.spirit.clear();
        self.refresh_character();
        true
    }

    /// Every character the player owns.
    #[must_use]
    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    /// The character sheet of the hero the player currently is.
    #[must_use]
    pub fn sheet(&self) -> &CharacterSheet {
        self.roster.sheet(self.hero)
    }

    /// The whole roster, for a save being loaded.
    ///
    /// Every caller that changes anything through this has to call
    /// [`Simulation::refresh_character`] afterwards, which is why gameplay goes through
    /// [`PlayerCommand::MoveItem`] instead.
    pub fn roster_mut(&mut self) -> &mut Roster {
        &mut self.roster
    }

    /// The Spirit buff's current stacks and clocks.
    #[must_use]
    pub fn spirit(&self) -> &SpiritSurge {
        &self.spirit
    }

    /// **The player's stats, right now**: base, plus gear, plus the Spirit buff.
    ///
    /// What the character panel prints and what [`Simulation::refresh_character`] hands the
    /// class. Derived on every call; see [`crate::character`] for why nothing stores it.
    #[must_use]
    pub fn character_stats(&self) -> crate::character::StatBlock {
        let mut stats = self.sheet().stats_with(&self.spirit);
        stats.add_block(&self.stat_bonus);
        // And what the effects on the player add or take away: Fortitude's Stamina, a
        // plague's loss of it. Read live, so an effect's stats are there exactly while it is.
        if let Some(statuses) = self.world.statuses(self.player.entity) {
            stats.add_block(&statuses.stat_bonus());
        }
        // A stat cannot go below nothing, however many debuffs are on it.
        for stat in crate::character::Stat::ALL {
            if stats.get(stat) < 0.0 {
                stats.set(stat, 0.0);
            }
        }
        stats
    }

    /// The stats granted on top of the sheet.
    #[must_use]
    pub fn stat_bonus(&self) -> crate::character::StatBlock {
        self.stat_bonus
    }

    /// Grants stats on top of the sheet, and recomputes everything derived from them.
    ///
    /// Replaces rather than adds, so the caller holds the whole of what is granted and there
    /// is no way to accumulate a buff that was never taken off.
    pub fn set_stat_bonus(&mut self, bonus: crate::character::StatBlock) {
        self.stat_bonus = bonus;
        self.refresh_character();
    }

    /// The outcome of the last attack on the player: hit, blocked, parried or dodged.
    #[must_use]
    pub fn last_defense(&self) -> Option<crate::character::DefenseOutcome> {
        self.last_defense
    }

    /// **Delivers one attack to the player through the ordinary incoming pipeline.**
    ///
    /// The scripted-encounter and test entry point, and deliberately not a second pipeline:
    /// it is the same call an enemy's swing makes, so what it reflects, avoids, blocks,
    /// mitigates, absorbs and banks is what a real attack does. Returns the outcome the one
    /// roll produced.
    pub fn deliver_attack(
        &mut self,
        attacker: EntityId,
        amount: f32,
        ability: AbilityId,
        damage_type: DamageType,
    ) -> Option<crate::character::DefenseOutcome> {
        self.last_defense = None;
        let target = self.player.entity;
        self.deal_damage(attacker, target, amount, ability, damage_type, false);
        self.last_defense
    }

    /// **Deals one instance of damage through the ordinary pipeline.**
    ///
    /// The counterpart of [`Simulation::deliver_attack`] for damage that is *not* aimed at
    /// the player: a scripted encounter's hazard, a developer control, or a test arranging a
    /// hit it did not want to play out a rotation for. It is the same `deal_damage` every
    /// ability calls, so everything that reads a landed hit — Nature's Exposure, Astral
    /// Sunfire's conversion, threat, the meter, the event stream — sees it exactly as it sees
    /// a real one.
    pub fn apply_damage(
        &mut self,
        source: EntityId,
        target: EntityId,
        amount: f32,
        ability: AbilityId,
        damage_type: DamageType,
    ) {
        self.deal_damage(source, target, amount, ability, damage_type, false);
    }

    /// A player's Versatility as a fraction (`x / (x + 100)`), from their own sheet, gear and
    /// effects. Zero for anything that is not a player.
    ///
    /// Reads the player through [`Simulation::with_player`], so in a party it is the named
    /// player's rating and not whoever happens to be active.
    pub(crate) fn versatility_of(&mut self, entity: EntityId) -> f32 {
        if !self.is_player_entity(entity) {
            return 0.0;
        }
        self.with_player(entity, |sim| {
            sim.character_stats().percent(Stat::Versatility)
        })
        .unwrap_or(0.0)
    }

    /// Moves an item between two slots on the current character, and recomputes everything
    /// that depended on what was worn.
    ///
    /// # Errors
    ///
    /// Returns a [`MoveError`] if the move is not allowed; nothing is changed in that case.
    pub fn move_item(&mut self, from: ItemSlot, to: ItemSlot) -> Result<(), MoveError> {
        self.roster.sheet_mut(self.hero).move_item(from, to)?;
        self.refresh_character();
        Ok(())
    }

    /// **Pushes the character sheet's stats into everything derived from them.**
    ///
    /// Two consumers today: the player entity's maximum health, which is Stamina, and the
    /// class's [`crate::firemage::Gear`], which is how the main stat becomes spell power and
    /// how a haste or crit rating becomes the fraction the class already multiplies by.
    ///
    /// Called whenever the inputs change — an item moved, a hero swapped, a save loaded, the
    /// Spirit buff gaining or losing a stack — rather than every tick, because the sheet is
    /// unchanged on the overwhelming majority of them.
    pub fn refresh_character(&mut self) {
        let stats = self.character_stats();
        let main = self.hero.main_stat();

        // Stamina, and then whatever the class is currently multiplying it by: the
        // Warrior's Last Stand and Battle Fervor. The *fraction* is preserved here
        // because this is the gear path — see `refresh_warrior_health` for the buff path,
        // which keeps the absolute health instead.
        let max = health_from_stamina(stats.get(Stat::Stamina)) * self.warrior_health_multiplier();
        self.world
            .set_max_health(self.player.entity, self.health_override.unwrap_or(max));

        // The bridge between the character sheet and the class. Gear is stated as what is
        // *added* to the class's base, and the base main stat is the base spell power, so
        // the difference is exactly what the character is wearing plus what Spirit is
        // multiplying.
        self.mage.gear = crate::firemage::Gear {
            spell_power: stats.get(main) - crate::character::BASE_MAIN_STAT,
            haste: stats.percent(Stat::Haste),
            crit_chance: stats.percent(Stat::CriticalStrike),
            crit_damage: 0.0,
        };
    }

    /// Replaces the player's maximum health with `max`, or gives it back to the character
    /// sheet with `None`. The player is healed to full either way.
    ///
    /// For a test scene: nothing in the game calls it.
    pub fn set_player_health_override(&mut self, max: Option<f32>) {
        self.health_override = max.filter(|max| max.is_finite() && *max > 0.0);
        self.refresh_character();
        let entity = self.player.entity;
        if let Some(health) = self.world.health(entity) {
            let missing = health.max() - health.current();
            if missing > 0.0 {
                self.world.heal(entity, missing);
            }
        }
    }

    /// The maximum health a test scene has fixed the player at, if it has.
    #[must_use]
    pub fn player_health_override(&self) -> Option<f32> {
        self.health_override
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

    /// The last line-of-sight check a targeted ability ran, if any.
    ///
    /// Developer tooling: the debug view draws it and the overlay prints it. Gameplay never
    /// reads it — the rule was already applied where the check was made.
    #[must_use]
    pub fn last_line_of_sight(&self) -> Option<LineOfSightDebug> {
        self.last_los
    }

    /// Runs a line-of-sight check for `ability` against `target` and records it.
    ///
    /// The only place a targeted ability's ray is cast, so the recorded query and the one
    /// gameplay obeyed cannot come apart. A target with no position is not visible: the
    /// caller has already refused such a target for being invalid, and answering "yes"
    /// here would make this the one path that disagreed.
    pub(crate) fn check_line_of_sight(
        &mut self,
        ability: FireAbility,
        target: EntityId,
        phase: LineOfSightPhase,
    ) -> bool {
        let definition = ability.definition();
        let check =
            crate::los::line_of_sight(&self.collision, &self.world, self.player.entity, target);
        let in_range = self
            .world
            .distance_between(self.player.entity, target)
            .is_some_and(|distance| distance <= definition.range);
        self.last_los = Some(LineOfSightDebug {
            ability,
            target,
            required: definition.checks_line_of_sight(),
            in_range,
            phase,
            check,
            tick: self.tick,
        });
        check.is_some_and(|check| check.visible)
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
        // Whatever is about to change, the ground monsters remember may be part of it.
        self.nav.invalidate();
        &mut self.collision
    }

    /// How monsters find their way, for a readout.
    #[must_use]
    pub fn navigator(&self) -> &crate::nav::Navigator {
        &self.nav
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
        // The party is the player and whoever is with them, so naming the player enrolls
        // them: a roster that had to be populated separately would be one the interface
        // could find empty while a player was plainly standing there.
        self.party.set_leader(entity);
        // The meter counts *this* actor's damage. Before anything could fight back every
        // damage event in the world was the player's own and the owner did not matter; with
        // enemies, a hit taken would otherwise be reported as damage dealt.
        self.meter.set_owner(entity);
        // The entity spawned with the archetype's health; the character's Stamina is what
        // decides it. Doing this here means every scene builder gets it without asking.
        self.refresh_character();
    }

    /// Who the player is grouped with, local player first.
    #[must_use]
    pub fn party(&self) -> &Party {
        &self.party
    }

    /// The party, mutably: how a member joins or leaves.
    ///
    /// Mutable access rather than `join`/`leave` wrappers, for the reason
    /// [`Simulation::world_mut`] gives: the roster is plain data with its own rules, and
    /// restating them here would be a second place they could be got wrong.
    pub fn party_mut(&mut self) -> &mut Party {
        &mut self.party
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
        if self.is_remote_request(&command) {
            self.remote_requests.push(command);
            return;
        }
        match command {
            PlayerCommand::Move { .. } => {
                // Held, not queued: the newest movement intent replaces the previous one,
                // so a frame that produced three of them does not move three times.
                self.intent.direction = command.move_direction();
                self.intent.facing = command.facing_lock();
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
        let started = Stopwatch::start();
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
        let started = Stopwatch::start();
        self.tick_once(&mut stats);
        stats.total = started.elapsed();
        self.record_stats(stats);
    }

    /// **How much damage `entity` will absorb before its health moves.** Zero for none.
    ///
    /// The one question every health bar asks — player frame, target, party, raid and
    /// nameplate alike — so a shield is drawn the same way whoever carries it and whatever
    /// granted it. Every source of absorb answers here: today the Warrior's Puncture Blood
    /// on any player, and whatever the authority reported for an entity while a server runs
    /// the fight. A future shield adds its source to this function and every bar draws it.
    #[must_use]
    pub fn absorb_of(&self, entity: EntityId) -> f32 {
        if self.remote_authority {
            return self
                .presented_absorbs
                .iter()
                .find(|(id, _)| *id == entity)
                .map_or(0.0, |(_, amount)| *amount);
        }
        if entity == self.player.entity {
            return self.warrior.absorb.remaining();
        }
        self.inactive_warrior(entity)
            .map_or(0.0, |warrior| warrior.absorb.remaining())
    }

    /// Steps until the player could start another ability: no cast running and the global
    /// cooldown over. Gives up after ten simulated seconds.
    ///
    /// What a headless driver or a test means by "press the next button": the global
    /// cooldown is a rule of the game, and a script that waited a fixed number of ticks would
    /// encode its length a second time.
    pub fn step_until_ready(&mut self) {
        for _ in 0..600 {
            if self.player.cast.is_none() && self.mage.off_global_cooldown() {
                return;
            }
            self.step_once();
        }
    }

    /// Runs `count` fixed ticks. The events of the whole run are kept.
    pub fn step_many(&mut self, count: u32) {
        self.begin_batch();
        let mut stats = SimStats {
            ticks: count,
            ..SimStats::default()
        };
        let started = Stopwatch::start();
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
        self.for_each_player(|sim| sim.meter.observe(sim.events.events_after(cursor), time));
        self.observed = self.events.len();
    }

    /// One fixed tick.
    fn tick_once(&mut self, stats: &mut SimStats) {
        let step = self.timestep.step_seconds();
        self.tick += 1;
        self.time += f64::from(step);
        if self.remote_authority {
            self.tick_presented(stats);
            return;
        }

        // Before anything moves: record where every body is, so presentation can draw
        // between this tick and the next instead of snapping between them. Nothing in
        // gameplay reads the result.
        self.world.begin_tick();

        self.for_each_player(Self::apply_pending_commands);

        // Before movement, so an intent decided this tick is walked this tick. AI is its own
        // stage in the timings because "what does thinking cost" is the question the stress
        // test asks, and burying it in `abilities` would answer a different one.
        let ai_started = Stopwatch::start();
        // Resets first: a pack asked to reset last tick is put back before anybody thinks,
        // so nothing spends a tick acting on a fight that is being taken down.
        self.apply_encounter_resets();
        self.tick_ai(step);
        // After the AI, so ground lit this tick does not also burn this tick — a pool the
        // player has had no frame to see is a pool they could not have avoided.
        self.tick_enemy_hazards(step);
        // Likewise a zone laid this tick does not land this tick.
        self.tick_enemy_strikes(step);
        // After every encounter has been brought up to date: a boss pulled this tick calls
        // its adds this tick, and a kill this tick is credited this tick.
        self.tick_dungeon();
        // After the dungeon, so a boss pulled this tick is already an engaged fight: the
        // modifiers read who is fighting and who just died, and decide nothing of either.
        self.tick_modifiers(step);
        stats.ai += ai_started.elapsed();

        let movement_started = Stopwatch::start();
        self.integrate_bodies(step);
        stats.movement += movement_started.elapsed();

        let ability_started = Stopwatch::start();
        self.for_each_player(|sim| {
            // Before the class tick and before the cast, because a Shield Charge in flight is
            // a movement that this tick's abilities are aimed from: the Warrior has to have
            // arrived before anything decides what is in range.
            sim.tick_warrior(step);
            sim.tick_firemage(step);
            // Before the cast, so a Tranquility that finishes this tick has already paid out
            // the pulse that fell on the same step.
            sim.tick_druid(step);
            // After the frame's commands have been applied — an instant ability used this
            // tick has already interrupted any cast — and before statuses, so a cast that
            // lands this tick applies its damage before this tick's burns.
            sim.tick_cast(step);
            sim.drop_invalid_target();
        });
        stats.abilities += ability_started.elapsed();

        let status_started = Stopwatch::start();
        self.tick_statuses(step);
        // An effect that changes the player's stats may have expired this tick; one that
        // changes a monster's attack speed is pushed into its mind, whoever applied it.
        self.for_each_player(|sim| {
            sim.sync_effect_stats();
            sim.sync_war_stomp();
        });
        stats.statuses += status_started.elapsed();

        self.for_each_player(|sim| {
            // After combat, so a tick that took damage cannot also regenerate from it.
            sim.tick_regeneration(step);
            sim.tick_spirit(step);
            // Before the respawn clock, because falling out of the world is one of the
            // things that could have killed the player this tick.
            sim.tick_void(step);
            // Last of all, because it asks one question — is the player alive — and the
            // answer has to be this tick's, after everything that could have killed them.
            sim.tick_respawn(step);
        });

        let telemetry_started = Stopwatch::start();
        self.flush_observations();
        self.for_each_player(|sim| sim.sample_fire_counters(step));
        stats.telemetry += telemetry_started.elapsed();
    }

    /// Sets the y under which the player has fallen out of the world.
    ///
    /// The level's, because only the level knows how deep its ground goes: a pit dug to
    /// −40 m is ground, and a character standing in it is standing, not falling. See
    /// [`crate::arena::void_height_of`], which is where the number comes from.
    pub fn set_void_height(&mut self, y: f32) {
        self.void_height = y;
        self.void_seconds = 0.0;
    }

    /// The y under which the player has fallen out of the world.
    #[must_use]
    pub fn void_height(&self) -> f32 {
        self.void_height
    }

    /// How long the player has been below the world, in seconds. Zero whenever they are not.
    #[must_use]
    pub fn void_seconds(&self) -> f32 {
        self.void_seconds
    }

    /// One tick of "has the player fallen out of the level".
    ///
    /// # Why a clock and not a plane
    ///
    /// A level's ground is its terrain, and a terrain has holes in it the moment an author
    /// digs one — so there is no floor to catch a fall without also filling in every pit. The
    /// answer is not to catch the fall but to notice it: below the level's own lowest ground
    /// by a wide margin, for [`VOID_KILL_SECONDS`], is not somewhere a player can be.
    ///
    /// They are then killed rather than teleported, so the way back is the way back from any
    /// other death — the respawn clock, the spawn point, the protection — instead of a second
    /// mechanism that puts a living player somewhere they did not walk to.
    ///
    /// The clock resets the moment they are above the line again, so a jump down a shaft that
    /// lands on something is a fall they survived.
    fn tick_void(&mut self, step: f32) {
        let player = self.player.entity;
        if !self.world.is_alive(player) {
            self.void_seconds = 0.0;
            return;
        }
        let Some(body) = self.world.body(player) else {
            return;
        };
        if body.position.y >= self.void_height {
            self.void_seconds = 0.0;
            return;
        }
        self.void_seconds += step;
        if self.void_seconds < VOID_KILL_SECONDS {
            return;
        }
        log::info!(
            "player fell out of the world: {:.1} s below y = {:.1}",
            self.void_seconds,
            self.void_height
        );
        self.void_seconds = 0.0;
        self.slay(player);
    }

    /// One tick of the player's death-and-return clock.
    ///
    /// The clock decides *when*; this decides *how*, and the level decided *where*. See
    /// [`crate::sim::respawn`] for why those three are kept apart.
    fn tick_respawn(&mut self, step: f32) {
        let alive = self.player_alive();
        match self.respawn.step(step, alive) {
            respawn::RespawnAction::Died => {
                log::info!("player died; respawning in {} s", respawn::RESPAWN_DELAY);
            }
            respawn::RespawnAction::Respawn => self.respawn_player(),
            respawn::RespawnAction::None => {}
        }
        self.publish_respawn_protection();
    }

    /// Writes the respawn clock's protection onto the player as an ordinary status.
    ///
    /// The status is the *published view* of a number this simulation owns — see
    /// [`crate::sim::respawn`]. Everything else in the game and the whole of the interface
    /// read the status and never the clock, which is what keeps "the player cannot be
    /// respawned" a timed effect like every other rather than a special case threaded through
    /// the gameplay code.
    fn publish_respawn_protection(&mut self) {
        let remaining = self.respawn.protection_remaining();
        let player = self.player.entity;
        let Some(statuses) = self.world.statuses_mut(player) else {
            return;
        };
        let kind = respawn::Respawn::status_kind();
        if remaining <= 0.0 {
            statuses.consume(kind);
            return;
        }
        let mut status = crate::status::Status::new(
            kind,
            player,
            AbilityId::NONE,
            respawn::RESPAWN_PROTECTION,
            0.0,
        );
        // The remaining time comes from the clock rather than from the status's own countdown,
        // because the status stops ticking on a corpse and this effect must not.
        status.remaining = remaining;
        statuses.apply(status);
    }

    /// Puts the player back at the level's spawn point, whole.
    ///
    /// [`Simulation::revive_player`] is what "whole" means and has been since long before
    /// respawning existed: full health, no cast, no target, no statuses. This adds the one
    /// thing a respawn is and a revive is not — being somewhere else.
    fn respawn_player(&mut self) {
        let player = self.player.entity;
        self.revive_player();
        // The party wiped: the fight it lost is over. What is still standing resets; what the
        // party killed stays dead, and the dungeon keeps its progress.
        self.reset_after_wipe();
        // After the revive, because that clears every status the corpse was carrying and the
        // protection is applied by `publish_respawn_protection` on this same tick.
        if let Some((position, yaw)) = self.player_spawn
            && let Some(body) = self.world.body_mut(player)
        {
            *body = crate::movement::Body::at(position);
            body.facing_yaw = yaw;
        }
        log::info!(
            "player respawned ({} this session); {} for {} s",
            self.respawn.count(),
            crate::status::StatusKind::RespawnProtection.display_name(),
            respawn::RESPAWN_PROTECTION
        );
    }

    /// Where the player comes back, and which way they face.
    ///
    /// Set by whatever built the level. A simulation nobody told respawns the player where
    /// they are, which is what every test and benchmark that predates spawn points expects.
    pub fn set_player_spawn(&mut self, position: Vec3, yaw: f32) {
        self.player_spawn = Some((position, yaw));
    }

    /// The player's death-and-return clock, for the interface and for a test.
    #[must_use]
    pub fn respawn(&self) -> &respawn::Respawn {
        &self.respawn
    }

    /// Kills the player through the ordinary damage path.
    ///
    /// **A development control**, and the only thing behind the F7 key: it exists so the death
    /// and respawn path can be exercised without finding something to be killed by. It goes
    /// through [`Simulation::slay`] rather than writing health to zero, so everything a real
    /// death produces is produced — the death event, the cast coming down, the hazards going
    /// out — and the respawn that follows is the shipping one and not a second path beside it.
    ///
    /// Pressing it while the player is already dead does nothing at all: `slay` refuses an
    /// entity with no health left, so a key held down cannot queue up deaths.
    pub fn kill_player(&mut self) {
        if !self.player_alive() {
            return;
        }
        self.slay(self.player.entity);
    }

    /// Records what Heat and Ignite looked like this tick.
    ///
    /// Sampled once a tick rather than derived from events afterwards: "average Heat" and
    /// "seconds spent above seventy" are properties of the timeline, and an event stream
    /// does not have one.
    fn sample_fire_counters(&mut self, step: f32) {
        let step = f64::from(step);
        let heat = self.mage.heat.value();
        self.fire.heat_sum += f64::from(heat);
        self.fire.heat_samples += 1;
        if self.mage.heat.is_inferno() {
            self.fire.seconds_inferno += step;
            self.fire.seconds_hot += step;
            self.fire.seconds_warm += step;
        } else {
            if heat >= crate::firemage::HEAT_WARM_THRESHOLD {
                self.fire.seconds_warm += step;
            }
            if heat >= crate::firemage::HEAT_HOT_THRESHOLD {
                self.fire.seconds_hot += step;
            }
        }

        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            let stacks = self.mage.ignite.stacks(id);
            if stacks > 0 {
                self.fire.ignite_stack_sum += f64::from(stacks);
                self.fire.ignite_stack_samples += 1;
            }
        }
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
                    if let Some(ability) = self.hero.ability_at(slot) {
                        let _ = self.cast_at(ability, position);
                    }
                }
                PlayerCommand::UseUltimate { position } => {
                    // Whichever ability this hero keeps beside the bar. The FireMage's is
                    // switched off and refuses; the Warrior's is Last Stand.
                    if let Some(ultimate) = self.hero.ultimate() {
                        let _ = self.cast_at(ultimate, position);
                    }
                }
                PlayerCommand::UseExtra(index) => {
                    if let Some(ability) = self.hero.extras().nth(index) {
                        let _ = self.cast(ability);
                    }
                }
                PlayerCommand::PurchaseTalent(talent) => {
                    let _ = self.mage.purchase_talent(talent);
                }
                PlayerCommand::MoveItem { from, to } => {
                    // A refused move is not an error the tick should fail on, for the same
                    // reason an ability that cannot be cast is not: a command is a request.
                    let _ = self.move_item(from, to);
                }
                PlayerCommand::SelectNearestTarget => {
                    self.player.target = self
                        .targets
                        .nearest_hostile(&self.world, self.player.entity);
                }
                PlayerCommand::CycleTarget => {
                    let current = self.player.target;
                    // Only what the class can actually reach. Nothing is selected when
                    // nothing is in range: the key does nothing rather than handing the
                    // player a target every ability would refuse.
                    if let Some(next) = self.targets.next_hostile_within(
                        &self.world,
                        self.player.entity,
                        current,
                        self.targeting_range(),
                    ) {
                        self.player.target = Some(next);
                    }
                }
                PlayerCommand::ClearTarget => {
                    self.player.target = None;
                }
                PlayerCommand::SelectSelf => {
                    self.player.target = Some(self.player.entity);
                }
                PlayerCommand::SelectTarget { entity } => {
                    // The caller named an entity; the rules of selection are still this
                    // crate's. A nameplate that outlived its enemy by a frame cannot select a
                    // corpse, and nothing can be asked to select an ally.
                    if TargetQuery::is_selectable(&self.world, self.player.entity, entity) {
                        self.player.target = Some(entity);
                    }
                }
                PlayerCommand::PickTarget { origin, direction } => {
                    // A miss changes nothing: clicking the floor is not a request to have no
                    // target. Escape is how a player asks for that, and it says so.
                    if let Some(picked) = self.targets.pick_selectable(
                        &self.world,
                        self.player.entity,
                        origin,
                        direction,
                    ) {
                        self.player.target = Some(picked);
                    }
                }
                PlayerCommand::ResetDummies => self.reset_dummies(),
                PlayerCommand::Teleport { position } => self.teleport_player(position),
                PlayerCommand::OpenChest => self.open_chest(),
                PlayerCommand::TakeMail { id } => self.take_mail(id),
                PlayerCommand::MarkMailRead { id } => self.mark_mail_read(id),
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
        let config = self.config.movement;
        let character = self.config.character;

        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            if self.is_player_entity(id) {
                continue;
            }
            // An enemy walks with the intent its brain decided; everything else stands
            // still and is only integrated so gravity and friction still apply to it.
            let runtime = self
                .enemies
                .get(slot)
                .and_then(|runtime| runtime.as_ref())
                .filter(|_| self.world.is_alive(id));
            let intent = runtime.map_or(MoveIntent::NONE, |runtime| runtime.intent);
            // Its own speed, and its current phase's scaling of it. Without this every enemy
            // walked at the player's top speed, which made "slower than the player" a number
            // in a table that nothing read — and a boss phase's `speed_scale` meaningless.
            // And no backpedal penalty: that is a player convention, and an enemy that backs
            // away to keep its target in front walks at the speed its brain was tuned for.
            let config = match runtime {
                Some(runtime) => MovementConfig {
                    // And every effect on it that names movement speed: a crippling shot
                    // slows a skeleton exactly as it slows the player.
                    max_speed: runtime.brain.speed()
                        * self
                            .world
                            .statuses(id)
                            .map_or(1.0, crate::status::StatusSet::move_speed_multiplier),
                    backpedal_speed: 1.0,
                    ..config
                },
                None => MovementConfig {
                    backpedal_speed: 1.0,
                    ..config
                },
            };
            if intent.direction.length() > 0.0 || intent.facing.is_some() {
                // The same collided step the player takes: an enemy that walked through a
                // wall would be an enemy the arena could not contain.
                if let Some(body) = self.world.body(id).copied() {
                    let mut moved = body;
                    movement::step_collided(
                        &mut moved,
                        intent,
                        &config,
                        &character,
                        &self.collision,
                        &mut self.collision_buffers,
                        step,
                    );
                    if let Some(body) = self.world.body_mut(id) {
                        *body = moved;
                    }
                }
            } else if let Some(body) = self.world.body_mut(id) {
                movement::step(body, MoveIntent::NONE, &config, step);
            }
        }

        self.for_each_player(|sim| sim.integrate_player(step));
    }

    /// Moves the active player one tick with their own intent, through the collided step.
    fn integrate_player(&mut self, step: f32) {
        let player = self.player.entity;
        if self.externally_moved.contains(&player) {
            // Moved by whoever owns it — a server applying the client's input frames one by
            // one — and only read here.
            self.intent.jump = false;
            return;
        }
        let intent = self.intent;
        self.move_active_player(intent, step);
        // Consumed, not held: a jump lasts one tick, and the next tick's intent comes from
        // the next frame's input.
        self.intent.jump = false;
    }

    /// Moves the player controlling `entity` one fixed step with `intent`, right now, through
    /// exactly the path the tick moves a player by: their speed effects, death, collision
    /// with whatever gates and barriers are standing. Returns false if no player controls
    /// `entity`.
    ///
    /// For a server applying a networked player's input frames as they arrive; see
    /// [`Simulation::set_external_movement`].
    pub fn drive_player(&mut self, entity: EntityId, intent: MoveIntent) -> bool {
        let step = self.timestep.step().as_secs_f32();
        self.with_player(entity, |sim| sim.move_active_player(intent, step))
            .is_some()
    }

    /// One collided step of the active player's body with `intent`.
    fn move_active_player(&mut self, intent: MoveIntent, step: f32) {
        let player = self.player.entity;
        let config = self.config.movement;
        let character = self.config.character;

        // A dead player is a body, not a character: it keeps falling and sliding to a stop
        // under the same controller, and it stops steering. Refusing abilities is
        // `try_cast`'s job and was already true before anything could kill anybody.
        let intent = if self.world.is_alive(player) {
            intent
        } else {
            MoveIntent::NONE
        };

        // Whatever the player's own class is currently multiplying their speed by: Wild
        // Rush, and nothing else today. Applied to the configuration the collided step
        // reads, so a faster character is faster through the same controller rather than
        // through a second movement path.
        let config = MovementConfig {
            max_speed: config.max_speed * self.move_speed_multiplier(),
            ..config
        };

        // The player last, and out of the loop, because it is the one body that needs the
        // collision world and the query buffers — both of which are borrowed from `self`.
        if let Some(body) = self.world.body_mut(player) {
            let mut moved = *body;
            let was_grounded = moved.grounded;
            let falling = moved.velocity.y;
            self.last_move = movement::step_collided(
                &mut moved,
                intent,
                &config,
                &character,
                &self.collision,
                &mut self.collision_buffers,
                step,
            );
            // Leaving and touching the ground, as events rather than as a state presentation
            // has to diff for itself. A landing carries the speed it arrived at, because how
            // hard a landing was is the whole difference between a step down and a fall.
            let tick = self.tick;
            if was_grounded && !moved.grounded && moved.velocity.y > 0.0 {
                self.events.push(CombatEvent::Jumped {
                    entity: player,
                    position: moved.position,
                    tick,
                });
            } else if !was_grounded && moved.grounded {
                self.events.push(CombatEvent::Landed {
                    entity: player,
                    position: moved.position,
                    speed: (-falling).max(0.0),
                    tick,
                });
            }
            if let Some(body) = self.world.body_mut(player) {
                *body = moved;
            }
        }
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

    /// Drops a target that has died or gone, and selects nobody in its place.
    ///
    /// **Nothing is chosen automatically.** A target is something the player picked, and
    /// picking the next one for them is picking wrong: a pack whose members die in turn
    /// would walk the selection around the room on its own, and every spell the player
    /// pressed afterwards would land somewhere they did not aim it.
    ///
    /// This used to select the nearest hostile instead, on the grounds that a prototype
    /// whose target quietly becomes nothing looks broken. With a dungeon rather than a
    /// training arena, the opposite is true.
    ///
    /// A living target that walked out of range keeps its place: only a dead or missing one
    /// is dropped, so this cannot take a selection away from a player chasing something.
    fn drop_invalid_target(&mut self) {
        let valid = self
            .player
            .target
            .is_some_and(|target| self.world.is_alive(target));
        if !valid {
            self.player.target = None;
        }
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
            // Haste, from the stats of *this* tick rather than from the moment the effect was
            // applied: a Druid who gains haste mid-fight ticks their existing Moonfire faster,
            // which is the same rule Ignite already follows. Only a status that asked to be
            // hasted uses it, and every one that does is the player's own.
            let haste_scale = self.mage.stats().interval(1.0);
            if let Some(statuses) = self.world.statuses_mut(id) {
                statuses.tick(step, haste_scale, &mut self.outcomes);
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
                        source_health_fraction,
                        heals,
                        ..
                    } => {
                        // **The unsnapshotted half.** The status handed out a coefficient;
                        // the source's maximum health is read here, on the tick the effect
                        // actually happens, so a bleed applied at ten thousand maximum health
                        // ticks for more after a Last Stand and for less after it ends.
                        let scaled = if source_health_fraction > 0.0 {
                            let max = self
                                .world
                                .health(source)
                                .map_or(0.0, crate::combat::Health::max);
                            source_health_fraction * max
                        } else {
                            0.0
                        };
                        let amount = damage + scaled;
                        if amount <= 0.0 {
                            continue;
                        }
                        if heals {
                            let amount = amount * (1.0 + self.versatility_of(source));
                            let restored = self.world.heal(id, amount).applied;
                            if restored > 0.0 {
                                let tick = self.tick;
                                self.events.push(CombatEvent::Healing {
                                    source,
                                    target: id,
                                    amount: restored,
                                    ability,
                                    tick,
                                });
                                self.add_healing_threat(source, id, restored, ability);
                            }
                            continue;
                        }
                        // A periodic tick does not roll: the roll belongs to the cast that
                        // applied the status, and rerolling per tick would make a damage
                        // over time effect a second source of critical strikes.
                        self.periodic_damage = true;
                        self.deal_damage(source, id, amount, ability, kind.damage_type(), false);
                        self.periodic_damage = false;
                    }
                    StatusOutcome::Ended {
                        kind,
                        reason,
                        ability,
                        expiry_heal,
                        ..
                    } => {
                        let tick = self.tick;
                        self.events.push(CombatEvent::StatusRemoved {
                            target: id,
                            status: kind,
                            reason,
                            tick,
                        });
                        // Lifebloom's bloom, and the only thing an ending pays out. The status
                        // system already decided it is owed nothing unless it expired.
                        if expiry_heal > 0.0 {
                            self.bloom(id, ability, expiry_heal);
                        }
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

        // Finished — but not landed yet. A targeted ability that needs line of sight is
        // asked one last time, against the target the cast was started on, so a player who
        // walked their target behind a pillar during a three-second Pyroblast does not hit
        // through it. Two checks, at the start and here: no ray is cast on the frames in
        // between.
        if let Some(target) = cast.target
            && cast.ability.definition().checks_line_of_sight()
            && !self.check_line_of_sight(cast.ability, target, LineOfSightPhase::CastCompletion)
        {
            self.player.last_error = Some(AbilityError::NoLineOfSight);
            self.player.last_error_at = self.time;
            // `end_cast` takes the cast, announces it and gives back the global cooldown:
            // the cast produced nothing, so it is refunded exactly like one that was
            // cancelled.
            self.end_cast(CastEnd::LineOfSightLost);
            return;
        }

        // The effect lands now, against the target the cast was started on.
        self.player.cast = None;
        self.fire.casts_completed += 1;
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
        //
        // It also gives back the global cooldown. That timer is started when a cast *begins*,
        // so without this a cast cancelled in its first frame would lock the whole bar for
        // the rest of half a second while nothing had happened — the same rule that refunds
        // the charge and the Ultimate's energy, applied to the one resource every ability
        // shares. A completed cast keeps it, because something did happen.
        if !matches!(end, CastEnd::Completed) {
            self.player.queued = None;
            self.mage.clear_global_cooldown();
        }
        let Some(cast) = self.player.cast.take() else {
            return;
        };
        if !matches!(end, CastEnd::Completed) {
            self.fire.casts_interrupted += 1;
        }
        self.events.push(CombatEvent::CastEnded {
            source: self.player.entity,
            ability: AbilityId::from_fire(cast.ability),
            end,
            tick: self.tick,
        });
    }

    /// The last refusal, if it is recent enough to still be worth showing.
    ///
    /// The simulation owns how long a message lives, so the interface cannot decide to keep
    /// one for longer than the rule that produced it.
    #[must_use]
    pub fn recent_error(&self) -> Option<AbilityError> {
        let error = self.player.last_error?;
        if self.time - self.player.last_error_at <= NOTIFICATION_SECONDS {
            Some(error)
        } else {
            None
        }
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
        let ability = self.hero.ability_at(slot).ok_or(AbilityError::CasterDead)?;
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

        // Fire that burned out. Announced so an observer holding a picture of it knows to
        // take the picture down; nothing in gameplay reads this.
        let mut expired = std::mem::take(&mut self.expired_zones);
        expired.clear();
        self.mage.zones.drain_expired(&mut expired);
        let tick = self.tick;
        for (id, zone) in &expired {
            self.events.push(CombatEvent::AreaEffectEnded {
                id: *id,
                position: zone.position,
                tick,
            });
        }
        expired.clear();
        self.expired_zones = expired;

        self.ignite_ticks.clear();
        let mut ignite_ticks = std::mem::take(&mut self.ignite_ticks);
        // The interval is handed in from the stats of *this* tick, which is what lets a
        // burn applied without haste speed up the moment the mage gains some.
        let interval = self.mage.ignite_interval();
        self.mage.ignite.tick(step, interval, &mut ignite_ticks);
        for tick in &ignite_ticks {
            self.periodic_damage = true;
            self.resolve_ignite_tick(*tick);
            self.periodic_damage = false;
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
        if let Some(result) = self.slide_body(entity, offset) {
            self.last_move = result;
        }
    }

    /// Moves any body by `offset`, sliding along whatever is in the way.
    ///
    /// The one collided displacement, shared by the player's Blink and Shield Charge and an
    /// enemy's charge, so a monster that charges into a pillar stops at the pillar exactly
    /// as a Warrior does.
    pub(super) fn slide_body(&mut self, entity: EntityId, offset: Vec3) -> Option<MoveResult> {
        let body = self.world.body(entity).copied()?;
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
        Some(result)
    }

    /// Spawns an enemy of `kind` at `position`, facing `yaw`.
    ///
    /// The one way an enemy comes into the world. It is an ordinary entity — the same
    /// spawn, the same body, the same health — that additionally gets a brain, which is
    /// what "an enemy is an entity that happens to have a mind" means in practice.
    pub fn spawn_enemy(&mut self, kind: EnemyKind, position: Vec3, yaw: f32) -> EntityId {
        self.spawn_enemy_with(
            kind,
            position,
            yaw,
            &dungeon_exec::EnemySpawnOptions::default(),
        )
    }

    /// One enemy's brain, if that entity is an enemy.
    #[must_use]
    pub fn brain(&self, entity: EntityId) -> Option<&EnemyBrain> {
        self.enemies
            .get(entity.index())
            .and_then(|slot| slot.as_ref())
            .filter(|_| self.world.contains(entity))
            .map(|runtime| &runtime.brain)
    }

    /// Every enemy in the world, with its brain.
    pub fn enemies(&self) -> impl Iterator<Item = (EntityId, &EnemyBrain)> + '_ {
        self.enemies
            .iter()
            .enumerate()
            .filter_map(|(slot, runtime)| {
                let runtime = runtime.as_ref()?;
                let entity = self.world.id_at(slot)?;
                Some((entity, &runtime.brain))
            })
    }

    /// How many enemies exist, alive or dead.
    #[must_use]
    pub fn enemy_count(&self) -> usize {
        self.enemies().count()
    }

    /// The first fight the scene built, if it built one.
    ///
    /// The single-encounter question, unchanged: a scene with one fight in it has one
    /// answer, and a dungeon's callers ask [`Simulation::encounters`] instead.
    #[must_use]
    pub fn encounter(&self) -> Option<&crate::encounter::Encounter> {
        self.encounters.first()
    }

    /// The ground enemies have left burning.
    ///
    /// Read by the developer overlay and by presentation, which draws one effect per hazard.
    #[must_use]
    pub fn enemy_hazards(&self) -> &crate::enemy::hazard::EnemyHazardField {
        &self.hazards
    }

    /// Every fight the scene built, in the order it built them.
    #[must_use]
    pub fn encounters(&self) -> &[crate::encounter::Encounter] {
        &self.encounters
    }

    /// One fight by index.
    #[must_use]
    pub fn encounter_at(&self, index: usize) -> Option<&crate::encounter::Encounter> {
        self.encounters.get(index)
    }

    /// Asks one fight to reset itself on the next tick.
    pub fn request_encounter_reset(&mut self, index: usize) {
        if let Some(encounter) = self.encounters.get_mut(index) {
            encounter.request_reset();
        }
    }

    /// Carries out every reset that was asked for, one encounter at a time.
    ///
    /// Run once a tick, before the AI: a reset is a *request* precisely so it happens between
    /// ticks rather than in the middle of one, where it would despawn entities this tick's
    /// events still name.
    ///
    /// Resetting one pack does not touch another. That is the difference between a wipe on
    /// the boss and a wipe on the dungeon, and it is why this is per encounter rather than
    /// the whole-world [`Simulation::reset_encounter`].
    pub fn apply_encounter_resets(&mut self) {
        let mut encounters = std::mem::take(&mut self.encounters);
        for (index, encounter) in encounters.iter_mut().enumerate() {
            if !encounter.is_resetting() {
                continue;
            }
            // An add the boss called was never part of the level: it is taken away, not put
            // back.
            for summon in encounter.take_summons() {
                if self.world.contains(summon) {
                    self.despawn_enemy(summon);
                }
            }
            if let Some(run) = self.dungeon.as_mut()
                && run.rules().boss_encounter == Some(index)
            {
                run.disengage();
            }
            for member in encounter.members().to_vec() {
                self.restore_enemy(member);
            }
            encounter.finish_reset();
        }
        self.encounters = encounters;
    }

    /// Puts one enemy back exactly as it spawned: home, whole, idle and forgetful.
    ///
    /// Restores rather than respawns, so entity ids stay valid and nothing holding one — a
    /// nameplate, a floating number, an effect — is left pointing at a slot that has moved on.
    pub fn restore_enemy(&mut self, entity: EntityId) {
        let slot = entity.index();
        let Some(Some(runtime)) = self.enemies.get_mut(slot) else {
            return;
        };
        runtime.brain.reset();
        runtime.intent = MoveIntent::NONE;
        runtime.dash = None;
        let telegraphs = runtime.telegraphs.take();
        let home = runtime.brain.home();
        if let Some((first, count)) = telegraphs {
            let anchor = self.world.position(entity).unwrap_or(home);
            self.end_telegraphs(first, count, anchor, false);
        }
        self.clear_hazards_of(entity);
        self.clear_strikes_of(entity);
        if !self.world.contains(entity) {
            return;
        }
        self.world.restore(entity);
        if let Some(body) = self.world.body_mut(entity) {
            *body = crate::movement::Body::at(home);
        }
    }

    /// Installs a single encounter, replacing any that were there, and spawns its enemies.
    ///
    /// The encounter owns the *membership*; the simulation owns the entities. Nothing else
    /// spawns enemies, so "who is in this fight" has one answer.
    pub fn set_encounter(&mut self, encounter: crate::encounter::Encounter) {
        self.encounters.clear();
        self.add_encounter(encounter);
    }

    /// Adds an encounter and spawns every enemy it names.
    ///
    /// What a dungeon calls, once per pack. The enemies of every pack exist from the moment
    /// the dungeon is built — they stand at their spawn points being idle, which is what
    /// makes walking into a room a pull rather than a spawn.
    pub fn add_encounter(&mut self, mut encounter: crate::encounter::Encounter) {
        let mut members = Vec::with_capacity(encounter.spawns().len());
        for spawn in encounter.spawns().to_vec() {
            let options = spawn
                .detail
                .map(|detail| detail.options())
                .unwrap_or_default();
            members.push(self.spawn_enemy_with(
                spawn.kind,
                spawn.position,
                spawn.facing_yaw,
                &options,
            ));
        }
        encounter.set_members(members);
        self.encounters.push(encounter);
    }

    /// Puts the fight back: every enemy alive, home, idle and forgetful, and the player
    /// whole.
    ///
    /// The development reset, and the same call the encounter's own `Resetting` state ends
    /// in. It restores rather than respawns, so entity ids stay valid and nothing that is
    /// holding one — a nameplate, a floating number, an effect — is left pointing at a
    /// slot that has moved on.
    pub fn reset_encounter(&mut self) {
        for slot in 0..self.enemies.len() {
            let Some(entity) = self.world.id_at(slot) else {
                continue;
            };
            // The same per-enemy restore a single pack's reset runs, so "reset everything"
            // and "reset this pack" cannot drift apart.
            self.restore_enemy(entity);
        }
        self.revive_player();
        // And the death clock, so a reset does not leave a respawn owed or twenty seconds of
        // protection carried over from the attempt that was just taken down.
        self.respawn.clear();
        // Ground left burning by anybody. A pool that survived a reset would be the one piece
        // of the last attempt still on the floor during the next one.
        self.clear_all_hazards();
        // And every mark, rain and quake still counting down.
        self.clear_all_strikes();
        // The run itself: nothing credited, the gate shut, the boss sealed, the adds gone.
        self.reset_dungeon_run();
        for encounter in &mut self.encounters {
            encounter.finish_reset();
        }
        self.meter.reset(self.time);
        log::info!("encounter reset");
    }

    /// Recovers from a wipe: every monster still standing goes home whole and forgets the
    /// fight, and every monster that was killed **stays dead**.
    ///
    /// What happens when the player comes back from dying. Unlike
    /// [`Simulation::reset_encounter`] — the development reset, which rebuilds the whole
    /// fight — this keeps the party's progress: packs they cleared stay cleared, completion
    /// keeps what was credited, a boss gate that opened stays open. A boss fight that was
    /// running is abandoned: its adds go, and the boss is made whole to be pulled again.
    pub fn reset_after_wipe(&mut self) {
        self.dismiss_summons();
        for slot in 0..self.enemies.len() {
            let Some(entity) = self.world.id_at(slot) else {
                continue;
            };
            if self.enemies[slot].is_some() && self.world.is_alive(entity) {
                self.restore_enemy(entity);
            }
        }
        self.clear_all_hazards();
        self.clear_all_strikes();
        for encounter in &mut self.encounters {
            // A cleared pack stays cleared; one with anybody left in it can be pulled again.
            if encounter.state() != crate::encounter::EncounterState::Completed {
                encounter.finish_reset();
            }
        }
        log::info!("wipe: the living reset, the dead stay dead");
    }

    /// Kills an entity outright, through the ordinary damage path.
    ///
    /// A development control — the counterpart of the reset key — and what a headless test
    /// uses to arrange "this pack is dead" without playing out a rotation. It goes through
    /// [`Simulation::deal_damage`] rather than writing health directly, so everything a real
    /// kill produces is produced: the death event, the cast bar coming down, the telegraph
    /// ending, the hazards going out and the encounter noticing.
    pub fn slay(&mut self, entity: EntityId) {
        let Some(health) = self.world.health(entity) else {
            return;
        };
        let remaining = health.current();
        if remaining <= 0.0 {
            return;
        }
        self.slaying = true;
        self.deal_damage(
            EntityId::INVALID,
            entity,
            remaining,
            AbilityId::NONE,
            DamageType::Physical,
            false,
        );
        self.slaying = false;
    }

    /// Whether `entity` is something that may attack the player.
    ///
    /// Asked before the incoming pipeline runs, so a Warrior's own Puncture Blood and a
    /// friendly heal never roll a dodge against themselves.
    #[must_use]
    pub fn is_hostile_to_player(&self, entity: EntityId) -> bool {
        let Some(theirs) = self.world.faction(entity) else {
            return false;
        };
        self.world
            .faction(self.player.entity)
            .is_some_and(|ours| ours.is_hostile_to(theirs))
    }

    /// Whether the player is alive.
    ///
    /// The one question every "can I act" check asks. It is on the simulation rather than
    /// read off the world at each call site so that a future ghost or downed state changes
    /// one function.
    #[must_use]
    pub fn player_alive(&self) -> bool {
        self.world.is_alive(self.player.entity)
    }

    /// Brings the player back to full health where they stand, and clears their state.
    pub fn revive_player(&mut self) {
        let player = self.player.entity;
        if !self.world.contains(player) {
            return;
        }
        self.world.restore(player);
        self.player.cast = None;
        self.player.queued = None;
        self.player.target = None;
        self.intent = MoveIntent::NONE;
        if let Some(statuses) = self.world.statuses_mut(player) {
            statuses.clear();
        }
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
        critical: bool,
    ) {
        // **The incoming pipeline.** An attack on the player is answered before it is
        // applied: reflected, avoided, blocked, mitigated by armour, absorbed, and banked
        // by Adrenaline — in that order, once, in one place. Everything a defensive event
        // pays out is fed from the single outcome that produces, which is what makes
        // duplicate payment structurally impossible rather than a rule to remember.
        // **Nature's Exposure**, read where damage is applied rather than where a Druid spell
        // is cast: it amplifies every point of Nature and Arcane damage the target takes,
        // whoever dealt it, which is what the design asks for and what a per-ability
        // multiplier could never be.
        // A boss behind a sealed gate cannot be hurt: it is not in the fight yet, and a bolt
        // through the bars would start one the dungeon has not allowed.
        if self.is_sealed(target) || self.remote_authority {
            return;
        }
        // With several players, the one being hit (their defences) or the one hitting (their
        // procs and resources) is made active first. A single-player simulation never swaps.
        if let Some(player) = self.context_player(source, target) {
            let _ = self.with_player(player, |sim| {
                sim.deal_damage(source, target, amount, ability, damage_type, critical);
            });
            return;
        }
        // Every effect that changes damage, from the registry: what the target's effects make
        // it take, what the source's make it deal. The player's flat reductions are applied
        // with the rest of their defences below; anybody else's are applied here.
        let mut amount = amount;
        if !self.slaying {
            amount *= self.damage_taken_multiplier(target, damage_type)
                * self.damage_dealt_multiplier(source);
            // **Versatility**, from each player's own sheet: a player deals more, a player
            // takes less. Applied here, the one place health goes down, so no ability can
            // bypass it.
            amount *= 1.0 + self.versatility_of(source);
            amount *= 1.0 - self.versatility_of(target) * VERSATILITY_MITIGATION_SHARE;
            if target != self.player.entity {
                amount *= 1.0 - self.status_damage_reduction(target);
            }
        }
        if target == self.player.entity && source != target && self.is_hostile_to_player(source) {
            match self.defend_player(source, amount, ability, damage_type) {
                Some(landing) => amount = landing,
                None => return,
            }
        }
        // Shield Block's promise, kept where damage is applied: a defender who cannot be
        // critically struck is never handed a critical hit. Nothing rolls a critical against
        // the player today, so this is a guard on a future one rather than a live branch.
        let critical = critical && !self.defense_profile().immune_to_critical;

        // Read before the damage: a killing blow frees nothing this tick, but a hit is a
        // thing that happened where the target was standing when it landed, and reading the
        // position afterwards would be reading it after a death has been processed.
        let position = self.world.position(target).unwrap_or(Vec3::ZERO);
        let result = self.world.damage(target, amount);
        if result.applied <= 0.0 && !result.killed {
            return;
        }
        // Dealing damage is what "in combat" means for Heat: the bar holds while a fight is
        // running and only drains once one is over.
        self.mage.note_combat();

        let tick = self.tick;
        self.events.push(CombatEvent::Damage(DamageEvent {
            source,
            target,
            amount: result.applied,
            overkill: result.overkill,
            ability,
            damage_type,
            killed: result.killed,
            critical,
            periodic: self.periodic_damage,
            position,
            tick,
        }));

        // **Astral Sunfire.** A share of what the party dealt to a marked enemy becomes
        // healing, from the one place the damage that actually landed is known.
        self.convert_astral_sunfire(source, target, result.applied);

        // Being hit is what makes an enemy angry. Threat is fed here, in the one place damage
        // lands, so no ability can deal damage without generating threat. The rate is the
        // ability's own, off its row: one for a damage dealer, two for the tank, more for
        // the tank's threat tools. Anything that is not a player ability counts one for one.
        let threat = result.applied
            * ability
                .fire()
                .map_or(1.0, |fire| fire.definition().threat.damage);
        if let Some(Some(runtime)) = self.enemies.get_mut(target.index())
            && self.world.contains(target)
        {
            runtime.brain.add_threat(source, threat);
        }

        if result.killed {
            // A corpse does not burn: the burn, its ticks and the fire vulnerability go
            // with the target, which is what makes detonating a nearly dead pack a real
            // decision rather than a free one.
            self.mage.ignite.clear(target);
            self.mage.vulnerability.clear(target);

            // Reported rather than silently dropped, so a status bar that is watching sees
            // it end.
            self.outcomes.clear();
            let mut ended = std::mem::take(&mut self.outcomes);
            if let Some(statuses) = self.world.statuses_mut(target) {
                statuses.clear_into(&mut ended);
            }
            for outcome in &ended {
                if let StatusOutcome::Ended { kind, reason, .. } = *outcome {
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

            // A dead enemy stops thinking, stops casting and stops being angry. Its cast
            // bar comes down through the event, exactly as a living caster's would.
            if let Some(Some(runtime)) = self.enemies.get_mut(target.index()) {
                let cast = runtime.brain.die();
                let telegraphs = runtime.telegraphs.take();
                runtime.intent = MoveIntent::NONE;
                runtime.dash = None;
                if let Some(cast) = cast {
                    if let Some((first, count)) = telegraphs {
                        self.end_telegraphs(first, count, cast.anchor, false);
                    }
                    self.events.push(CombatEvent::CastEnded {
                        source: target,
                        ability: AbilityId::from_enemy(cast.ability),
                        end: CastEnd::CasterDied,
                        tick,
                    });
                }
            }

            // Ground it left burning goes out with it. A pool whose caster is gone is a pool
            // nothing will ever explain, and it would keep dealing damage credited to a
            // corpse.
            self.clear_hazards_of(target);
            // And the marks it laid: killing the Bonecaster in time saves whoever it marked.
            self.clear_strikes_of(target);
            // What it was worth towards the dungeon.
            // Queued whoever died: during the AI's own step the enemy table is out, so
            // whether this was an enemy is answered when the queue is paid, not here.
            self.queue_credit(target);
            // And a dungeon modifier's say in what a death leaves behind.
            self.modifier_on_death(target, position);

            // The pack it belonged to hears it die. A member killed outright never leaves
            // Idle and its threat table is cleared by the death, so this is the only moment
            // the killer's identity and the victim's membership are both in hand — without
            // it a pack can be taken apart one silent one-shot at a time.
            if source != target {
                for encounter in &mut self.encounters {
                    if encounter.contains(target) {
                        encounter.raise_alarm(source);
                    }
                }
            }

            self.events.push(CombatEvent::Death {
                entity: target,
                killer: source,
                tick,
            });
            self.flush_credits();
        }
    }

    /// Credits the threat a heal generates.
    ///
    /// Every living enemy that is already fighting whoever was healed — has them in its
    /// threat table — gets `restored` times the ability's healing rate for the healer.
    /// Healing somebody no enemy has noticed pulls nothing, and neither does a heal that is
    /// not a player ability: regeneration and an enemy mending its own pack are not the
    /// party's doing.
    ///
    /// Called with what the heal actually restored, so overhealing generates no threat.
    pub(super) fn add_healing_threat(
        &mut self,
        healer: EntityId,
        healed: EntityId,
        restored: f32,
        ability: AbilityId,
    ) {
        let Some(fire) = ability.fire() else {
            return;
        };
        let threat = restored * fire.definition().threat.healing;
        if !threat.is_finite() || threat <= 0.0 {
            return;
        }
        let Some(ours) = self.world.faction(healer) else {
            return;
        };
        for slot in 0..self.enemies.len() {
            let Some(runtime) = self.enemies[slot].as_mut() else {
                continue;
            };
            let Some(entity) = self.world.id_at(slot) else {
                continue;
            };
            if !self.world.is_alive(entity)
                || !runtime.brain.threat().contains(healed)
                || !self
                    .world
                    .faction(entity)
                    .is_some_and(|theirs| theirs.is_hostile_to(ours))
            {
                continue;
            }
            runtime.brain.add_threat(healer, threat);
        }
    }

    /// Whether the player is in a fight right now.
    ///
    /// True while any enemy is in one of the fighting states — chasing, attacking, casting or
    /// recovering. Asked of the enemies rather than kept as a flag on the player: the pack
    /// *is* the fight, and a timer since the last hit would say a player is in combat while
    /// they stand alone in an empty room, and out of it while a Brute walks the last two
    /// metres towards them.
    ///
    /// An enemy walking home has already given up, and Idle, Alert, Returning and Dead are
    /// therefore all "not fighting": the fight ends when the last enemy stops chasing.
    #[must_use]
    pub fn player_in_combat(&self) -> bool {
        self.enemies().any(|(_, brain)| brain.state().is_combat())
    }

    /// Health the player regains per second out of combat, as a fraction of their maximum.
    #[must_use]
    pub fn health_regen_per_second(&self) -> f32 {
        self.config.health_regen_per_second
    }

    /// Sets the out-of-combat regeneration rate, as a fraction of maximum health per second.
    ///
    /// What the player's `settings.cfg` reaches the simulation through. A nonsense value is
    /// ignored rather than stored: a NaN here would put NaN health on the player, and a
    /// character with no health at all is a worse outcome than an ignored setting.
    pub fn set_health_regen_per_second(&mut self, fraction: f32) {
        if fraction.is_finite() && fraction >= 0.0 {
            self.config.health_regen_per_second = fraction;
        }
    }

    /// Gives the player health back while nothing is fighting them.
    ///
    /// Once a second, not once a tick. The amount is the same either way, but a tick's worth
    /// of it is a hundredth of a point: too small to announce, too small to draw, and sixty
    /// times a second it would fill the event stream and the damage meter with noise. One
    /// heal a second is one event, one floating number and one line a player can read.
    ///
    /// The clock resets the moment anything engages, so leaving a fight does not immediately
    /// pay out the second that was accumulating when it started.
    /// Runs the Spirit buff's clocks and recomputes the stats when a stack falls off.
    ///
    /// Does nothing at all when no stack is running, which on the current roster is always:
    /// no hero and no item grants Spirit, so nothing can ever have procced.
    fn tick_spirit(&mut self, step: f32) {
        if !self.spirit.is_active() {
            return;
        }
        self.spirit.tick(step);
        self.refresh_character();
    }

    fn tick_regeneration(&mut self, step: f32) {
        let player = self.player.entity;
        let rate = self.config.health_regen_per_second;
        if rate <= 0.0 || !self.world.is_alive(player) || self.player_in_combat() {
            self.regen_elapsed = 0.0;
            return;
        }
        self.regen_elapsed += step;
        // Against an epsilon, because sixty steps of a sixtieth do not add up to exactly one
        // in binary floating point — without it every regeneration tick would land a frame
        // late and the rate would be quietly wrong.
        if self.regen_elapsed < 1.0 - crate::TIME_EPSILON {
            return;
        }
        self.regen_elapsed = (self.regen_elapsed - 1.0).max(0.0);

        let Some(health) = self.world.health(player) else {
            return;
        };
        if health.current() >= health.max() {
            return;
        }
        let restored = self.world.heal(player, health.max() * rate).applied;
        if restored <= 0.0 {
            return;
        }
        // Credited to the player and to no ability: regeneration is something the character
        // does, not something they cast, and the meter must not report it as healing done.
        self.events.push(CombatEvent::Healing {
            source: player,
            target: player,
            amount: restored,
            ability: AbilityId::NONE,
            tick: self.tick,
        });
    }

    /// Restores every dummy and starts a new combat session.
    ///
    /// Health, position and statuses come back, the meter is cleared, and cooldowns and the
    /// resource are refilled — the point of the control is to try a rotation again from a
    /// known state, and waiting eight seconds for Detonate is not part of that state.
    pub fn reset_dummies(&mut self) {
        self.mage.reset();
        self.warrior.reset();
        self.druid.reset();
        // The player's own buffs and burns are part of the state the control exists to clear.
        if let Some(statuses) = self.world.statuses_mut(self.player.entity) {
            statuses.clear();
        }
        self.fire = FireCounters::default();
        for slot in 0..self.world.slot_count() {
            let Some(id) = self.world.id_at(slot) else {
                continue;
            };
            // Both kinds of dummy: the hostile ones the rotation is practised on, and the
            // friendly one the healing is. Restoring one and not the other would leave the
            // heals of the last attempt running on a target the key was pressed to clear.
            if matches!(
                self.world.kind(id),
                Some(EntityKind::Dummy | EntityKind::HealingDummy)
            ) {
                self.world.restore(id);
            }
        }
        // A cast in progress is part of the state R exists to clear: the point of the
        // control is to try a rotation again from a known start, and "mid-cast" is not it.
        self.player.cast = None;
        self.player.last_error = None;
        // The same key puts the *fight* back where there is one: an encounter is what the
        // dummies are in a scene that has enemies, and a player who has just died pressing
        // the reset key means "again", not "restore the dummies I was not fighting".
        //
        // Pressed by a *dead* player it is the wipe recovery rather than a rebuild: back at the
        // start, the monsters still standing home and whole, and every monster the party
        // killed still dead. Only a living player's press puts the whole fight back — the
        // development reset.
        if !self.encounters.is_empty() || !self.enemies.is_empty() {
            if self.player_alive() {
                self.reset_encounter();
            } else {
                self.respawn.clear();
                self.respawn_player();
            }
            return;
        }
        self.meter.reset(self.time);
        self.drop_invalid_target();
        log::debug!("dummies reset at simulation time {:.2} s", self.time);
    }

    /// How far away something may be and still be worth selecting, in metres.
    ///
    /// The longest reach the current hero has. Selection is not a spell, so it does not
    /// belong to any one ability's range — but a target nothing on the bar can hit is a
    /// target the player would have to cycle past, which is what makes an eight-room
    /// dungeon's cycle key useless.
    #[must_use]
    pub fn targeting_range(&self) -> f32 {
        self.hero.longest_range()
    }

    /// Selects the nearest living hostile the class could reach.
    pub fn select_nearest_target(&mut self) {
        if let Some(nearest) = self.targets.nearest_hostile_within(
            &self.world,
            self.player.entity,
            self.targeting_range(),
        ) {
            self.player.target = Some(nearest);
        }
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

    #[test]
    fn a_health_override_survives_the_character_sheet_and_can_be_given_back() {
        let mut simulation = arena();
        let player = simulation.player().entity;
        let natural = simulation
            .world()
            .health(player)
            .map_or(0.0, |health| health.max());

        simulation.set_player_health_override(Some(1_000_000.0));
        let health = |simulation: &Simulation| {
            simulation
                .world()
                .health(player)
                .map_or((0.0, 0.0), |health| (health.current(), health.max()))
        };
        assert_eq!(
            health(&simulation),
            (1_000_000.0, 1_000_000.0),
            "set, and full"
        );

        // Everything that recomputes maximum health from the sheet: a refresh, a hero swap —
        // the Warrior's multipliers included.
        simulation.refresh_character();
        assert_eq!(health(&simulation).1, 1_000_000.0);
        assert!(simulation.set_hero(Hero::Warrior));
        for _ in 0..30 {
            simulation.step_once();
        }
        assert_eq!(
            health(&simulation).1,
            1_000_000.0,
            "the Warrior keeps the override"
        );

        simulation.set_player_health_override(None);
        assert!(simulation.set_hero(Hero::FireMage));
        assert!((health(&simulation).1 - natural).abs() < 1e-3, "given back");
        assert_eq!(simulation.player_health_override(), None);
    }

    fn dummy_health(simulation: &Simulation, id: EntityId) -> f32 {
        simulation
            .world()
            .health(id)
            .map_or(0.0, |health| health.current())
    }

    /// The player's health right now.
    fn player_health(simulation: &Simulation) -> f32 {
        simulation
            .world()
            .health(simulation.player().entity)
            .map_or(0.0, crate::combat::Health::current)
    }

    #[test]
    fn the_player_regenerates_one_percent_a_second_out_of_combat() {
        // The training arena has dummies, and a dummy has no brain: nothing is fighting.
        let mut simulation = arena();
        let player = simulation.player().entity;
        let max = simulation
            .world()
            .health(player)
            .expect("the player exists")
            .max();
        simulation.world_mut().damage(player, max * 0.5);
        let wounded = player_health(&simulation);

        assert!(!simulation.player_in_combat());
        // Ten seconds is ten ticks of regeneration at one percent of maximum each.
        simulation.step_many(600);
        let regained = player_health(&simulation) - wounded;
        assert!(
            (regained - max * 0.10).abs() < max * 0.005,
            "regained {regained} in ten seconds, expected {}",
            max * 0.10
        );
    }

    #[test]
    fn regeneration_ticks_once_a_second_rather_than_once_a_frame() {
        // Sixty events a second of a sixtieth of a point each would be noise in the stream,
        // in the meter and on the screen.
        let mut simulation = arena();
        let player = simulation.player().entity;
        simulation.world_mut().damage(player, 200.0);

        simulation.step_many(300);
        let heals = simulation
            .events()
            .iter()
            .filter(
                |event| matches!(event, CombatEvent::Healing { target, .. } if *target == player),
            )
            .count();
        assert_eq!(heals, 5, "five seconds should be five heals");
    }

    #[test]
    fn regeneration_stops_while_anything_is_fighting_the_player() {
        let config = ArenaConfig::with_group(0);
        let mut simulation = arena::encounter_arena(&config, arena::pack_encounter(&config, 2));
        let player = simulation.player().entity;
        simulation.world_mut().damage(player, 200.0);
        simulation.step_once();
        let wounded = player_health(&simulation);

        // Walk into the pack. Once they are chasing, the player heals nothing.
        simulation.push_command(PlayerCommand::Teleport {
            position: simulation.player_position() + Vec3::new(0.0, 0.0, -14.0),
        });
        simulation.step_many(120);
        assert!(simulation.player_in_combat(), "the pack never engaged");

        let before = player_health(&simulation);
        simulation.step_many(300);
        assert!(
            player_health(&simulation) <= before,
            "the player regenerated during a fight"
        );
        let _ = wounded;
    }

    #[test]
    fn regeneration_stops_at_full_health_and_never_revives_a_corpse() {
        let mut simulation = arena();
        let player = simulation.player().entity;
        let max = simulation
            .world()
            .health(player)
            .expect("the player exists")
            .max();
        simulation.step_many(300);
        assert_eq!(player_health(&simulation), max, "healing past full");
        assert!(
            !simulation
                .events()
                .iter()
                .any(|event| matches!(event, CombatEvent::Healing { .. })),
            "a plate at full health was healed anyway"
        );

        simulation.world_mut().damage(player, 1e9);
        // Stopped short of `respawn::RESPAWN_DELAY`, because the player lifecycle brings a
        // corpse back after three seconds now and this test is about regeneration: it asserts
        // that *healing over time* never revives one, which is a different claim.
        simulation.step_many(120);
        assert!(
            !simulation.player_alive(),
            "regeneration revived the player"
        );
        assert_eq!(player_health(&simulation), 0.0);
    }

    #[test]
    fn the_regeneration_rate_is_configurable_and_can_be_turned_off() {
        let mut simulation = arena();
        let player = simulation.player().entity;
        simulation.world_mut().damage(player, 200.0);

        simulation.set_health_regen_per_second(0.0);
        let before = player_health(&simulation);
        simulation.step_many(300);
        assert_eq!(player_health(&simulation), before, "zero still regenerated");

        // And a faster rate heals proportionally more.
        simulation.set_health_regen_per_second(0.05);
        simulation.step_many(60);
        let regained = player_health(&simulation) - before;
        let max = simulation
            .world()
            .health(player)
            .expect("the player exists")
            .max();
        assert!(
            (regained - max * 0.05).abs() < max * 0.005,
            "regained {regained} at five percent a second"
        );

        // Nonsense is ignored rather than stored: NaN health is worse than a lost setting.
        simulation.set_health_regen_per_second(f32::NAN);
        assert_eq!(simulation.health_regen_per_second(), 0.05);
        simulation.set_health_regen_per_second(-1.0);
        assert_eq!(simulation.health_regen_per_second(), 0.05);
    }

    #[test]
    fn regeneration_is_not_counted_as_healing_done() {
        // The meter measures the fight. A player who took the long way round between pulls
        // has not out-healed one who fought well.
        let mut simulation = arena();
        let player = simulation.player().entity;
        simulation.world_mut().damage(player, 200.0);
        simulation.step_many(300);
        assert_eq!(simulation.meter().total_healing(), 0.0);
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

        simulation.cast(FireAbility::Detonate).expect("a full burn");
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

        simulation.step_many(130);
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
            face_camera: false,
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
    fn a_cancelled_cast_costs_nothing_at_all() {
        // Nothing is spent until a cast lands, so backing out of one leaves the ability
        // exactly as it was.
        let mut simulation = arena();
        simulation.cast(FireAbility::Pyroblast).expect("in range");
        assert!(simulation.mage().abilities.is_ready(FireAbility::Pyroblast));

        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(4);
        assert!(simulation.casting().is_none(), "movement did not break it");
        assert!(
            simulation.mage().abilities.is_ready(FireAbility::Pyroblast),
            "a cast that never landed cost a charge"
        );
    }

    #[test]
    fn starting_any_cast_puts_the_whole_bar_on_the_global_cooldown() {
        // What "global" means: it is not a property of the ability that was pressed.
        let mut simulation = arena();
        assert!(simulation.mage().off_global_cooldown());

        simulation.cast(FireAbility::Pyroblast).expect("in range");
        assert!(!simulation.mage().off_global_cooldown());
        assert!(simulation.mage().global_cooldown_fraction() < 1.0);
        assert!(simulation.mage().global_cooldown_remaining() > 0.0);
    }

    #[test]
    fn the_global_cooldown_refuses_every_other_ability_while_it_runs() {
        // Checked after an *instant*, because pressing something during a cast queues it
        // rather than being refused — the queue is tested on its own elsewhere.
        let mut simulation = arena();
        simulation
            .cast(FireAbility::FlameShield)
            .expect("a self-cast");
        assert!(simulation.casting().is_none(), "an instant started a cast");

        // An unrelated, ready, free ability is held by the shared timer and by nothing else.
        assert!(simulation.mage().abilities.is_ready(FireAbility::Firebolt));
        assert_eq!(
            simulation.cast(FireAbility::Firebolt),
            Err(AbilityError::GlobalCooldown)
        );
    }

    #[test]
    fn blink_detonate_and_fire_mastery_ignore_the_global_cooldown_and_do_not_start_it() {
        let mut simulation = arena();
        for ability in [
            FireAbility::Blink,
            FireAbility::Detonate,
            FireAbility::FireMastery,
        ] {
            assert!(!ability.definition().on_global_cooldown, "{ability:?}");
        }

        // Pressed while the global cooldown runs, they go off anyway...
        simulation
            .cast(FireAbility::FlameShield)
            .expect("a self-cast");
        assert!(!simulation.mage().off_global_cooldown());
        let left = simulation.mage().global_cooldown_remaining();
        simulation
            .cast(FireAbility::FireMastery)
            .expect("off the global cooldown");
        let target = simulation.player().target.expect("a dummy is selected");
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(target, maximum, maximum, duration);
        simulation.cast(FireAbility::Detonate).expect("a full burn");
        simulation
            .cast(FireAbility::Blink)
            .expect("off the global cooldown");
        // ...and leave it exactly as it was rather than restarting it.
        assert!((simulation.mage().global_cooldown_remaining() - left).abs() < 1e-6);

        // And from a standing start, none of them locks the bar.
        let mut simulation = arena();
        simulation.cast(FireAbility::Blink).expect("ready");
        assert!(simulation.mage().off_global_cooldown());
        assert!(simulation.cast(FireAbility::Firebolt).is_ok());
    }

    #[test]
    fn a_cancelled_cast_gives_the_global_cooldown_back() {
        // The timer starts when a cast begins, so without releasing it a cast cancelled in
        // its first frame would lock the bar for half a second while nothing happened.
        let mut simulation = arena();
        simulation.cast(FireAbility::Pyroblast).expect("in range");
        assert!(!simulation.mage().off_global_cooldown());

        assert!(simulation.cancel_cast(), "there was a cast to cancel");
        assert!(
            simulation.mage().off_global_cooldown(),
            "the bar is still locked by a cast that never happened"
        );
        assert_eq!(simulation.mage().global_cooldown_fraction(), 1.0);

        // And the bar really is usable again, not merely reporting that it is.
        assert!(simulation.cast(FireAbility::Blink).is_ok());
    }

    #[test]
    fn moving_out_of_a_cast_also_gives_the_global_cooldown_back() {
        // Moving is a cancel the player did not type. It costs the same: nothing.
        let mut simulation = arena();
        simulation.cast(FireAbility::Pyroblast).expect("in range");
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(4);

        assert!(simulation.casting().is_none(), "movement did not break it");
        assert!(
            simulation.mage().off_global_cooldown(),
            "the bar stayed locked after the cast was broken"
        );
    }

    #[test]
    fn a_completed_cast_keeps_its_global_cooldown() {
        // The other half of the rule. An instant lands immediately and still locks the bar,
        // which is the whole reason the timer exists.
        let mut simulation = arena();
        simulation
            .cast(FireAbility::FlameShield)
            .expect("a self-cast");
        assert!(
            !simulation.mage().off_global_cooldown(),
            "an instant that landed did not lock the bar"
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
        simulation.step_many(130);

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
            face_camera: false,
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
        simulation.step_many(21 * simulation.timestep().hz() as u32);
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

    #[test]
    fn naming_the_player_puts_them_at_the_head_of_the_party() {
        // The party frame reads the roster, so a player standing in the world who is not in
        // it would be a frame that is empty while they are plainly there.
        let mut simulation = Simulation::new(SimConfig::default());
        assert!(simulation.party().is_empty());

        let entity = simulation
            .world_mut()
            .spawn(crate::world::EntityDesc::player(Vec3::ZERO));
        simulation.set_player_entity(entity);
        assert_eq!(simulation.party().members(), [entity]);
        assert_eq!(simulation.party().member(0), Some(entity));

        // And a member joining is visible immediately, without a tick.
        let ally = simulation
            .world_mut()
            .spawn(crate::world::EntityDesc::player(Vec3::ZERO));
        assert!(simulation.party_mut().join(ally));
        assert_eq!(simulation.party().len(), 2);
        assert_eq!(
            simulation.party().member(0),
            Some(entity),
            "the player moved"
        );
    }
}
