//! The state machine: what one enemy wants to do this tick.
//!
//! ```text
//!            player in aggro radius            alert_time elapsed
//!   Idle ─────────────────────────▶ Alert ───────────────────────▶ Chase ◀──┐
//!    ▲                                                              │       │
//!    │                                              in range        ▼       │ out of range
//!    │                                        ┌──────────────── Attack      │
//!    │                                        │                   │         │
//!    │                             cast ability│         instant   │        │
//!    │                                        ▼                    ▼        │
//!    │                                      Cast ──────────────▶ Recover ───┘
//!    │                                        │  lands              │
//!    │            arrived home                │  interrupted        │
//!    └──────────── Returning ◀────────────────┴─────────────────────┘
//!                  nobody left to fight (the player is dead), or
//!                  an idle monster pushed past its leash
//!
//!   any state ──────────────▶ Dead   (health reached zero)
//! ```
//!
//! # What this file is allowed to know
//!
//! An [`AiPercept`]: where this enemy is, where its target is, how far away, and whether it
//! is still worth attacking. That is all. It cannot see the world, cannot deal damage,
//! cannot move a body and cannot emit an event — [`EnemyBrain::think`] returns an
//! [`AiDecision`], and the simulation is what carries it out.
//!
//! That split is what makes the AI testable without a world and impossible to use as a back
//! door into gameplay: **AI decides what it wants; combat decides what happens.**
//!
//! # Deterministic by construction
//!
//! There is no randomness here at all. Ability choice is the archetype's priority order, and
//! every timer is driven by the fixed timestep. The same tick from the same state produces
//! the same decision on every machine, which is what the determinism tests assert.

use engine_core::math::Vec3;

use crate::entity::EntityId;
use crate::movement::MoveIntent;

use super::{
    ENEMY_ABILITY_COUNT, EnemyAbility, EnemyArchetype, EnemyKind, EnemyRequirement, EnemyTarget,
    TelegraphShape,
};

/// The most impact zones one cast can lay.
///
/// Deadly Rain's three circles on each of three players. A fixed array rather than a list, so a
/// cast stays `Copy` and a brain allocates nothing however many zones it lays.
pub const MAX_ZONES: usize = 9;

/// The most distinct hostiles one cast can be aimed at.
pub const MAX_TARGETS: usize = 3;

/// How close to home counts as home, in metres.
const HOME_TOLERANCE: f32 = 0.6;

/// What an enemy is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiState {
    /// Standing at home, watching for something to fight.
    Idle,
    /// Has noticed something and is about to act on it.
    Alert,
    /// Moving towards its target.
    Chase,
    /// In range, choosing and starting attacks.
    Attack,
    /// Casting something. The telegraph is up.
    Cast,
    /// The gap after an attack, during which it does nothing.
    Recover,
    /// Walking home because it lost its target or ran out of leash.
    Returning,
    /// Dead. Nothing further happens.
    Dead,
}

impl AiState {
    /// Whether this is one of the fighting states.
    ///
    /// The brief's `Combat` state, as a predicate rather than a nesting: four flat states
    /// are easier to assert on than a state with a sub-state, and this is the one place the
    /// grouping matters.
    #[must_use]
    pub fn is_combat(self) -> bool {
        matches!(
            self,
            Self::Chase | Self::Attack | Self::Cast | Self::Recover
        )
    }

    /// Short uppercase name, for the developer readout.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Alert => "ALERT",
            Self::Chase => "CHASE",
            Self::Attack => "ATTACK",
            Self::Cast => "CAST",
            Self::Recover => "RECOVER",
            Self::Returning => "RETURN",
            Self::Dead => "DEAD",
        }
    }
}

/// What the simulation tells a brain about the world this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiPercept {
    /// Where this enemy is standing.
    pub position: Vec3,
    /// Whether it is still alive.
    pub alive: bool,
    /// Its target, if it has a valid one.
    pub target: Option<AiTarget>,
    /// The nearest hostile that could be noticed, valid target or not.
    ///
    /// What pulls an idle enemy into a fight: it has no threat towards anybody yet, so
    /// "who is nearby" is a separate question from "who am I angriest at".
    pub nearest: Option<AiTarget>,
    /// How much of its health is left, in `[0, 1]`. What phase transitions are read from.
    pub health_fraction: f32,
    /// The most wounded living ally, if anything friendly has lost health.
    ///
    /// The caster itself counts. `None` means nothing is hurt, which is what makes an
    /// untouched pack's healer fall through to its filler instead of casting a heal at
    /// nobody.
    pub wounded_ally: Option<AiTarget>,
    /// The hostile furthest away that could still be reached, for a charge.
    ///
    /// Only asked for by an archetype that has something to aim at it; `None` otherwise.
    pub farthest: Option<AiTarget>,
    /// Whether whoever it is fighting is bleeding, for an ability that needs them to be.
    pub victim_bleeding: bool,
}

impl AiPercept {
    /// A percept of a living enemy at `position` with nobody around it.
    ///
    /// The starting point every caller fills in: a test names the one or two fields it is
    /// about, and a field added to the percept later is a default here rather than an edit to
    /// every literal.
    #[must_use]
    pub fn alone(position: Vec3) -> Self {
        Self {
            position,
            alive: true,
            target: None,
            nearest: None,
            health_fraction: 1.0,
            wounded_ally: None,
            farthest: None,
            victim_bleeding: false,
        }
    }

    /// Who this enemy is fighting: the angriest, or failing that the nearest.
    ///
    /// Threat decides *between* attackers; it does not decide *whether* there is a fight.
    /// An enemy pulled by walking into its aggro radius has no threat towards anybody yet,
    /// and one that answered "nobody" here would walk home the instant it noticed somebody.
    ///
    /// The executor asks the same question with the same function, so what the brain aimed
    /// at and what combat hits cannot be two different things.
    #[must_use]
    pub fn victim(&self) -> Option<AiTarget> {
        self.target.or(self.nearest)
    }
}

/// What the simulation tells a brain about one possible victim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiTarget {
    /// Which entity.
    pub entity: EntityId,
    /// Where it is standing.
    pub position: Vec3,
    /// Horizontal metres between the two bodies' *edges*, never below zero.
    ///
    /// Edge to edge because every range test in this engine is: a wide body is in melee
    /// range when its side is, not when its centre is.
    pub distance: f32,
    /// Whether nothing solid stands between the two: the same chest-to-chest ray the
    /// player's own targeted abilities are refused on.
    ///
    /// An ability aimed at somebody is only used on somebody visible, and an enemy whose
    /// victim is behind a wall closes in — around the wall — until they are not.
    pub visible: bool,
}

/// What the brain decided to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiDecision {
    /// How it wants to move this tick.
    pub intent: MoveIntent,
    /// What it wants combat to do about an ability, if anything.
    pub request: AiRequest,
    /// The phase it entered this tick, if it changed phase.
    ///
    /// Carried on the decision rather than polled off the brain afterwards, for the same
    /// reason the ability request is: everything the simulation has to act on comes out of
    /// one call, and there is no second question to forget to ask.
    pub phase: Option<usize>,
}

impl AiDecision {
    /// Stand still and ask for nothing.
    pub const IDLE: Self = Self {
        intent: MoveIntent::NONE,
        request: AiRequest::None,
        phase: None,
    };

    /// The same decision, reporting that it entered `phase` this tick.
    #[must_use]
    fn entering(mut self, phase: Option<usize>) -> Self {
        self.phase = phase;
        self
    }
}

/// An ability the brain wants resolved.
///
/// The brain never applies damage: it *asks*, and the simulation validates and executes —
/// the same relationship the player's commands have with the ability system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiRequest {
    /// Nothing this tick.
    None,
    /// Use this ability now; it has no cast time.
    Instant(EnemyAbility),
    /// Start casting this ability. Its telegraph goes up now.
    BeginCast(EnemyAbility),
    /// The cast that was running has finished. Resolve it.
    CompleteCast(EnemyAbility),
    /// The cast that was running has ended without landing.
    CancelCast(EnemyAbility),
}

/// Why a cast ended without landing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastStop {
    /// A player ability interrupted it.
    Interrupted,
    /// Its target died, went away, or the enemy lost its leash.
    Lost,
    /// The caster died.
    Died,
}

/// A cast in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyCast {
    /// Which ability.
    pub ability: EnemyAbility,
    /// Simulated seconds spent so far.
    pub elapsed: f32,
    /// Simulated seconds it takes.
    pub duration: f32,
    /// Who it was started against.
    pub target: EntityId,
    /// Where its shape is anchored — the aim point, fixed at the moment the cast started.
    ///
    /// Fixed, deliberately: a telegraph that followed a moving player would be impossible to
    /// walk out of, which is the entire point of a telegraph.
    pub anchor: Vec3,
    /// Whether a player interrupt can stop it.
    pub interruptible: bool,
    /// The ground it will cover, resolved at the moment the cast started.
    ///
    /// Resolved once, here, and then used *twice*: the telegraph event carries this value
    /// and the hit test runs against this value. A cone recomputed when it landed would
    /// point wherever the caster had turned to in the meantime, and the telegraph on the
    /// floor would have been a lie.
    pub shape: TelegraphShape,
    /// How many zones the cast lays. Zone zero is [`EnemyCast::anchor`] and
    /// [`EnemyCast::shape`]; the rest are filled in by the simulation when the cast starts.
    ///
    /// The brain cannot see the world, so it cannot know who else there is to aim at; it
    /// starts the cast on its one recipient and the executor, which can, plans the rest.
    pub zone_count: u8,
    /// Where each zone is anchored.
    pub zones: [Vec3; MAX_ZONES],
    /// Where each zone was *aimed*: the player a line was fired at, the spot a pool is left.
    pub aims: [Vec3; MAX_ZONES],
    /// Which way each zone's shape points, for cones and lines.
    pub yaws: [f32; MAX_ZONES],
    /// Who each zone was laid on, for a mark that follows its victim.
    pub zone_targets: [Option<EntityId>; MAX_ZONES],
    /// Every hostile the cast is aimed at, the recipient first.
    pub targets: [Option<EntityId>; MAX_TARGETS],
    /// How many ticks a channel has already dealt.
    pub ticks: u8,
}

impl EnemyCast {
    /// The shape of zone `index`, turned the way that zone points.
    #[must_use]
    pub fn zone_shape(&self, index: usize) -> TelegraphShape {
        aimed_shape(self.shape, self.yaws.get(index).copied().unwrap_or(0.0))
    }

    /// The zones the cast lays, as `(anchor, shape, aim, laid on)`.
    pub fn zone_list(
        &self,
    ) -> impl Iterator<Item = (Vec3, TelegraphShape, Vec3, Option<EntityId>)> + '_ {
        (0..usize::from(self.zone_count).min(MAX_ZONES)).map(|index| {
            (
                self.zones[index],
                self.zone_shape(index),
                self.aims[index],
                self.zone_targets[index],
            )
        })
    }

    /// Every hostile the cast is aimed at.
    pub fn target_list(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.targets.iter().filter_map(|target| *target)
    }
}

impl EnemyCast {
    /// Progress in `[0, 1]`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Seconds left before it lands.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

/// One enemy's mind: state, timers, cooldowns, threat and the cast it is running.
///
/// Held by the simulation, one per live enemy, and indexed by the entity's slot.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyBrain {
    /// Which archetype it is.
    kind: EnemyKind,
    state: AiState,
    /// Where it was spawned, and where it returns to.
    home: Vec3,
    /// Which way home it was facing when it spawned.
    home_yaw: f32,
    /// Who it is fighting.
    target: Option<EntityId>,
    /// Everyone who has attacked it.
    threat: super::threat::ThreatTable,
    /// Seconds left in the current state, where the state has a timer.
    timer: f32,
    /// Seconds left on each ability, by index.
    cooldowns: [f32; ENEMY_ABILITY_COUNT],
    /// The cast in progress, if any.
    cast: Option<EnemyCast>,
    /// Seconds it has been in combat, for the readout.
    combat_time: f32,
    /// Which phase it is in. Zero is the archetype's own ability list.
    ///
    /// Only ever increases within a life: healing an enemy back over a threshold does not
    /// put it back a phase, which is what stops a Crypt Warden from rewinding a boss.
    phase: usize,
    /// Seconds of stun left. While it is running the enemy does nothing at all.
    ///
    /// A control effect lives on the mind rather than in a status set because what it does
    /// is stop the mind: a stunned enemy is not an enemy with a debuff that something has to
    /// remember to check, it is an enemy whose `think` returns before it decides anything.
    stun: f32,
    /// Seconds of silence left. While it is running the enemy may not start a *cast*.
    ///
    /// Instant attacks are unaffected, which is what makes a silence a different tool from a
    /// stun: a silenced caster walks up and swings.
    silence: f32,
    /// What its attack speed is multiplied by. One is unhindered.
    ///
    /// Pushed in from outside — War Stomp's stacks are a status on the enemy, and the
    /// simulation converts them into this — so the brain has no idea what a Warrior is. It
    /// divides every cooldown it starts, which is what "attack speed" means for an enemy
    /// whose whole rotation is cooldowns.
    attack_speed_scale: f32,
    /// Seconds of frenzy left: the enemy attacks faster while it runs.
    frenzy: f32,
    /// How much faster a frenzy makes it attack, as a fraction.
    frenzy_haste: f32,
    /// How close the player must come before it notices, in metres.
    ///
    /// The archetype's, unless the level placed it with its own - a sentry an author wants
    /// to notice the whole courtyard is a placement setting, not a new monster.
    aggro_radius: f32,
    /// Whether it has been told to drop the fight, to be carried out on its next think.
    ///
    /// A request rather than an immediate change, so the cast it loses is cancelled through
    /// the same decision every other lost cast is — the telegraph comes down and the cast
    /// bar is told why, by the executor, in one place.
    evading: bool,
}

impl EnemyBrain {
    /// A brain for a `kind` spawned at `home`.
    #[must_use]
    pub fn new(kind: EnemyKind, home: Vec3, home_yaw: f32) -> Self {
        Self {
            kind,
            state: AiState::Idle,
            home,
            home_yaw,
            target: None,
            threat: super::threat::ThreatTable::new(),
            timer: 0.0,
            cooldowns: [0.0; ENEMY_ABILITY_COUNT],
            cast: None,
            combat_time: 0.0,
            phase: 0,
            stun: 0.0,
            silence: 0.0,
            attack_speed_scale: 1.0,
            frenzy: 0.0,
            frenzy_haste: 0.0,
            aggro_radius: kind.archetype().aggro_radius,
            evading: false,
        }
    }

    /// Sets how close the player must come before it notices, in metres.
    ///
    /// Survives a reset: it is where the level put it, not something the fight changed.
    pub fn set_aggro_radius(&mut self, radius: f32) {
        if radius.is_finite() && radius >= 0.0 {
            self.aggro_radius = radius;
        }
    }

    /// How close the player must come before it notices, in metres.
    #[must_use]
    pub fn aggro_radius(&self) -> f32 {
        self.aggro_radius
    }

    /// Starts a frenzy: `haste` faster for `seconds`, keeping whichever lasts longer.
    pub fn frenzy(&mut self, seconds: f32, haste: f32) {
        if seconds.is_finite() && seconds > self.frenzy {
            self.frenzy = seconds;
        }
        if haste.is_finite() {
            self.frenzy_haste = haste.max(0.0);
        }
    }

    /// Whether it is frenzied.
    #[must_use]
    pub fn is_frenzied(&self) -> bool {
        self.frenzy > 0.0
    }

    /// Seconds of frenzy left. Zero when it is not frenzied.
    #[must_use]
    pub fn frenzy_remaining(&self) -> f32 {
        self.frenzy.max(0.0)
    }

    /// Everything that makes it attack faster, multiplied together.
    ///
    /// Divides every cooldown and every recovery it starts. The one place the two sources -
    /// a debuff pushed in from outside and its own frenzy - meet.
    #[must_use]
    pub fn haste(&self) -> f32 {
        let frenzy = if self.frenzy > 0.0 {
            1.0 + self.frenzy_haste
        } else {
            1.0
        };
        (self.attack_speed_scale * frenzy).max(0.2)
    }

    /// The cast in progress, to be completed by the simulation.
    ///
    /// The brain starts a cast on its one recipient; the executor, which can see the world,
    /// fills in the other targets and zones the ability asks for. Nothing else writes here.
    pub fn cast_mut(&mut self) -> Option<&mut EnemyCast> {
        self.cast.as_mut()
    }

    /// Advances its timers without deciding anything.
    ///
    /// What a charge runs while the body is carried along its line: the mind waits, its
    /// cooldowns and its frenzy keep counting, and it decides nothing until it lands.
    pub fn hold(&mut self, step: f32) {
        let step = if step.is_finite() { step.max(0.0) } else { 0.0 };
        self.tick_timers(step);
    }

    /// Which archetype it is.
    #[must_use]
    pub fn kind(&self) -> EnemyKind {
        self.kind
    }

    /// Everything about its archetype.
    #[must_use]
    pub fn archetype(&self) -> &'static EnemyArchetype {
        self.kind.archetype()
    }

    /// What it is doing.
    #[must_use]
    pub fn state(&self) -> AiState {
        self.state
    }

    /// Who it is fighting.
    #[must_use]
    pub fn target(&self) -> Option<EntityId> {
        self.target
    }

    /// Where it goes when it gives up.
    #[must_use]
    pub fn home(&self) -> Vec3 {
        self.home
    }

    /// Its threat table.
    #[must_use]
    pub fn threat(&self) -> &super::threat::ThreatTable {
        &self.threat
    }

    /// The cast it is running, if any.
    #[must_use]
    pub fn cast(&self) -> Option<EnemyCast> {
        self.cast
    }

    /// Seconds left on one ability.
    #[must_use]
    pub fn cooldown(&self, ability: EnemyAbility) -> f32 {
        self.cooldowns[ability.index()]
    }

    /// Seconds left in whatever the current state is counting down.
    #[must_use]
    pub fn timer(&self) -> f32 {
        self.timer
    }

    /// Seconds spent fighting since it last left Idle.
    #[must_use]
    pub fn combat_time(&self) -> f32 {
        self.combat_time
    }

    /// Whether it is stunned: doing nothing, and deciding nothing.
    #[must_use]
    pub fn is_stunned(&self) -> bool {
        self.stun > 0.0
    }

    /// Seconds of stun left.
    #[must_use]
    pub fn stun_remaining(&self) -> f32 {
        self.stun.max(0.0)
    }

    /// Whether it is silenced: instants only.
    #[must_use]
    pub fn is_silenced(&self) -> bool {
        self.silence > 0.0
    }

    /// Seconds of silence left.
    #[must_use]
    pub fn silence_remaining(&self) -> f32 {
        self.silence.max(0.0)
    }

    /// Stuns it for `seconds`, keeping whichever stun is longer.
    ///
    /// The longer of the two rather than the sum, for the reason every timer in the project
    /// takes the longer: two half-second stuns landing together are one half-second stun,
    /// not a second of one.
    pub fn stun(&mut self, seconds: f32) {
        if seconds > self.stun {
            self.stun = seconds.max(0.0);
        }
    }

    /// Silences it for `seconds`, keeping whichever silence is longer.
    pub fn silence(&mut self, seconds: f32) {
        if seconds > self.silence {
            self.silence = seconds.max(0.0);
        }
    }

    /// Sets what its attack speed is multiplied by.
    ///
    /// Clamped to a floor rather than allowed to reach zero: an enemy whose cooldowns are
    /// divided by nothing never attacks again, which is a stun with no duration.
    pub fn set_attack_speed_scale(&mut self, scale: f32) {
        self.attack_speed_scale = if scale.is_finite() {
            scale.clamp(0.2, 4.0)
        } else {
            1.0
        };
    }

    /// What its attack speed is multiplied by.
    #[must_use]
    pub fn attack_speed_scale(&self) -> f32 {
        self.attack_speed_scale
    }

    /// Adds threat, which is how being attacked reaches the AI.
    ///
    /// Called by the simulation when damage lands on this enemy. It does *not* pull the
    /// enemy into combat by itself — [`EnemyBrain::think`] does that on the next tick, so
    /// aggro from damage and aggro from proximity take the same path.
    pub fn add_threat(&mut self, attacker: EntityId, amount: f32) {
        self.threat.add(attacker, amount);
    }

    /// Forces it to attack `taunter` for `seconds`, whatever its threat table says.
    ///
    /// Like [`EnemyBrain::add_threat`] it decides nothing by itself: the next think re-chooses
    /// the target, and an idle enemy that was taunted is pulled by the row the taunt created.
    pub fn taunt(&mut self, taunter: EntityId, seconds: f32) {
        self.threat.taunt(taunter, seconds);
    }

    /// Whether somebody is about to take it off the one it is fighting.
    ///
    /// Only while it is fighting: an enemy walking home or still noticing is attacking
    /// nobody yet, and has no aggro to lose. See [`super::threat::ThreatTable::watch`].
    pub fn watch_aggro(
        &mut self,
        valid: impl FnMut(EntityId) -> bool,
    ) -> Option<super::threat::AggroWarning> {
        let holder = self.target.filter(|_| self.state.is_combat());
        self.threat.watch(holder, valid)
    }

    /// Puts it back exactly as it spawned: home, idle, full of nothing.
    ///
    /// Health is the world's business and is restored by the caller; everything the *brain*
    /// remembers is dropped here.
    pub fn reset(&mut self) {
        self.state = AiState::Idle;
        self.target = None;
        self.threat.clear();
        self.timer = 0.0;
        self.cooldowns = [0.0; ENEMY_ABILITY_COUNT];
        self.cast = None;
        self.combat_time = 0.0;
        self.phase = 0;
        self.stun = 0.0;
        self.silence = 0.0;
        self.attack_speed_scale = 1.0;
        self.frenzy = 0.0;
        self.frenzy_haste = 0.0;
        self.evading = false;
    }

    /// Drops the fight: forget everyone, and walk home.
    ///
    /// What a wipe does to a monster that was fighting. Everything the fight changed is put
    /// back — threat, phase, cooldowns, crowd control, frenzy — except where it stands: it
    /// walks home rather than being teleported there, so a party that wiped watches the
    /// room reset rather than finding it reset. Its health is the simulation's to restore.
    ///
    /// Returns whether there was a fight to drop: false when it is idle at home or dead. One
    /// already walking home counts — losing its target is usually how a monster finds out
    /// its victim died, a moment before anybody tells it why — and is walking home still.
    pub fn evade(&mut self) -> bool {
        if matches!(self.state, AiState::Idle | AiState::Dead) {
            return false;
        }
        self.threat.clear();
        self.target = None;
        self.cooldowns = [0.0; ENEMY_ABILITY_COUNT];
        self.combat_time = 0.0;
        self.phase = 0;
        self.stun = 0.0;
        self.silence = 0.0;
        self.attack_speed_scale = 1.0;
        self.frenzy = 0.0;
        self.frenzy_haste = 0.0;
        self.evading = true;
        true
    }

    /// Which phase it is in. Zero is the first.
    #[must_use]
    pub fn phase(&self) -> usize {
        self.phase
    }

    /// The name of the phase it is in, for the boss frame and the readout.
    #[must_use]
    pub fn phase_name(&self) -> &'static str {
        self.archetype().phase_name(self.phase)
    }

    /// How fast it moves right now, in metres per second.
    ///
    /// The archetype's speed scaled by its phase. Read by movement, which is what makes a
    /// phase's `speed_scale` mean something rather than being a number in a table.
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.archetype().phase_speed(self.phase)
    }

    /// The abilities it can use right now, in preference order.
    pub fn abilities(&self) -> impl Iterator<Item = EnemyAbility> + '_ {
        self.archetype().phase_abilities(self.phase)
    }

    /// Pulls it into a fight it has not noticed itself.
    ///
    /// What makes a pack a pack: one member being pulled brings the rest, whether or not the
    /// player came close enough for each of them to see it. Called by the simulation when an
    /// encounter activates — a pack is a property of the *encounter*, and an enemy has no
    /// idea who else is in one.
    ///
    /// It enters Alert with its own `alert_time` rather than jumping straight to Chase, so a
    /// pack waking up is one readable beat rather than five bodies snapping round at once,
    /// and a monster with a longer notice still takes it.
    ///
    /// Returns whether anything changed. An enemy that is already fighting, walking home or
    /// dead is left exactly as it is.
    pub fn provoke(&mut self, target: EntityId) -> bool {
        if self.state != AiState::Idle {
            return false;
        }
        self.state = AiState::Alert;
        self.target = Some(target);
        self.timer = self.archetype().alert_time;
        self.combat_time = 0.0;
        true
    }

    /// Marks it dead, which stops everything.
    ///
    /// Returns the cast it was running, if it had one, so the caller can announce that the
    /// cast ended — a cast bar that its caster died under has to come down.
    pub fn die(&mut self) -> Option<EnemyCast> {
        self.state = AiState::Dead;
        self.target = None;
        self.timer = 0.0;
        self.threat.clear();
        self.cast.take()
    }

    /// Stops the cast in progress, if there is one, and enters recovery.
    ///
    /// Returns what was stopped. This is the interrupt path: a player ability calls it, the
    /// cast does not land, and the enemy is left with a gap it has to recover from — which
    /// is what makes an interrupt worth pressing.
    pub fn stop_cast(&mut self, reason: CastStop) -> Option<EnemyCast> {
        let cast = self.cast.take()?;
        if self.state == AiState::Cast {
            self.state = match reason {
                CastStop::Died => AiState::Dead,
                // Longer than the ability's own recovery: an interrupted caster is supposed
                // to be exposed, and its own recovery is tuned for a cast that landed.
                CastStop::Interrupted => {
                    self.timer = cast.ability.definition().recovery.max(0.5) * 2.0;
                    AiState::Recover
                }
                CastStop::Lost => {
                    self.timer = 0.0;
                    AiState::Chase
                }
            };
        }
        Some(cast)
    }

    /// Whether an interrupt would do anything right now.
    #[must_use]
    pub fn is_interruptible(&self) -> bool {
        self.cast.is_some_and(|cast| cast.interruptible)
    }

    /// Decides what to do this tick.
    ///
    /// The only entry point. It advances every timer, runs the transitions in the diagram at
    /// the top of this file, and returns what it wants — it changes nothing outside itself.
    pub fn think(&mut self, percept: &AiPercept, step: f32) -> AiDecision {
        let step = if step.is_finite() { step.max(0.0) } else { 0.0 };
        self.tick_timers(step);

        if !percept.alive {
            if self.state != AiState::Dead {
                let cast = self.die();
                if let Some(cast) = cast {
                    return AiDecision {
                        intent: MoveIntent::NONE,
                        request: AiRequest::CancelCast(cast.ability),
                        phase: None,
                    };
                }
            }
            return AiDecision::IDLE;
        }
        if self.state == AiState::Dead {
            return AiDecision::IDLE;
        }

        // Told to drop the fight: the cast goes, and it turns for home this tick. Before the
        // stun, because `evade` has already lifted it.
        if std::mem::take(&mut self.evading) {
            let cast = self.stop_cast(CastStop::Lost);
            self.state = AiState::Returning;
            self.target = None;
            if let Some(cast) = cast {
                return AiDecision {
                    intent: MoveIntent::NONE,
                    request: AiRequest::CancelCast(cast.ability),
                    phase: None,
                };
            }
            return self.think_returning(percept);
        }

        // Stunned: nothing else in this function runs. A cast in progress is lost, exactly
        // as it is to an interrupt, and the caller is told so it can take the telegraph
        // down — a stun that left a cast bar on screen would be a lie about what is
        // happening.
        if self.is_stunned() {
            if let Some(cast) = self.stop_cast(CastStop::Interrupted) {
                self.state = AiState::Recover;
                self.timer = self.timer.max(self.stun);
                return AiDecision {
                    intent: MoveIntent::NONE,
                    request: AiRequest::CancelCast(cast.ability),
                    phase: None,
                };
            }
            return AiDecision::IDLE;
        }

        // The target is re-chosen every tick from threat, so a taunt, a death or a new
        // attacker changes who is being hit without any state needing to know about it.
        //
        // Not while idle: an idle enemy is *aware* of whoever is nearest without having
        // chosen them, and reporting a target it has not decided to fight would make the
        // developer readout - and every test - unable to tell the two apart.
        if self.state != AiState::Idle {
            self.target = percept.victim().map(|target| target.entity);
        }
        if self.state.is_combat() {
            self.combat_time += step;
        }

        // The phase, before anything is decided with it: a boss that crossed a threshold this
        // tick must not spend this tick casting the ability it no longer has.
        if let Some(entered) = self.advance_phase(percept.health_fraction) {
            let cast = self.stop_cast(CastStop::Lost);
            self.state = AiState::Recover;
            self.timer = super::PHASE_STAGGER;
            if let Some(cast) = cast {
                return AiDecision {
                    intent: MoveIntent::NONE,
                    request: AiRequest::CancelCast(cast.ability),
                    phase: Some(entered),
                };
            }
            return AiDecision::IDLE.entering(Some(entered));
        }

        // The leash, for a monster that is not fighting: one knocked or dragged far from home
        // while idle walks back. A monster that has been pulled never gives up on a living
        // victim, however far it is led — it chases until it or its victim dies, and only an
        // empty world (the player dead, see `evade`) sends it home. Kiting a pack away from
        // its spawn and having it reset mid-fight was the thing players asked to be rid of.
        if self.state == AiState::Idle && self.beyond_leash(percept.position) {
            let cast = self.stop_cast(CastStop::Lost);
            self.state = AiState::Returning;
            self.threat.clear();
            self.target = None;
            if let Some(cast) = cast {
                return AiDecision {
                    intent: MoveIntent::NONE,
                    request: AiRequest::CancelCast(cast.ability),
                    phase: None,
                };
            }
            return self.think_returning(percept);
        }

        match self.state {
            AiState::Idle => self.think_idle(percept),
            AiState::Alert => self.think_alert(percept),
            AiState::Chase | AiState::Attack => self.think_engaged(percept),
            AiState::Cast => self.think_cast(percept),
            AiState::Recover => self.think_recover(percept),
            AiState::Returning => self.think_returning(percept),
            AiState::Dead => AiDecision::IDLE,
        }
    }

    /// Counts down every timer this tick.
    fn tick_timers(&mut self, step: f32) {
        self.timer = (self.timer - step).max(0.0);
        self.stun = (self.stun - step).max(0.0);
        self.silence = (self.silence - step).max(0.0);
        self.frenzy = (self.frenzy - step).max(0.0);
        self.threat.tick(step);
        for cooldown in &mut self.cooldowns {
            *cooldown = (*cooldown - step).max(0.0);
        }
        if let Some(cast) = self.cast.as_mut() {
            cast.elapsed += step;
        }
    }

    /// Idle: watch for somebody to come close enough.
    fn think_idle(&mut self, percept: &AiPercept) -> AiDecision {
        // Threat first: something that shot an idle enemy from outside its aggro radius has
        // pulled it, which is what makes a ranged pull work.
        let noticed = percept
            .target
            .filter(|_| !self.threat.is_empty())
            .or_else(|| {
                percept
                    .nearest
                    .filter(|near| near.distance <= self.aggro_radius)
            });
        let Some(noticed) = noticed else {
            return self.face_home(percept);
        };
        self.target = Some(noticed.entity);
        self.state = AiState::Alert;
        self.timer = self.archetype().alert_time;
        self.combat_time = 0.0;
        // It turns to look, and nothing else: the pause is what makes a pull readable.
        AiDecision {
            intent: MoveIntent {
                direction: Vec3::ZERO,
                jump: false,
                facing: Some(yaw_towards(percept.position, noticed.position)),
            },
            request: AiRequest::None,
            phase: None,
        }
    }

    /// Alert: finish noticing, then engage.
    fn think_alert(&mut self, percept: &AiPercept) -> AiDecision {
        let Some(target) = percept.victim() else {
            self.state = AiState::Returning;
            return self.think_returning(percept);
        };
        if self.timer > 0.0 {
            return AiDecision {
                intent: MoveIntent {
                    direction: Vec3::ZERO,
                    jump: false,
                    facing: Some(yaw_towards(percept.position, target.position)),
                },
                request: AiRequest::None,
                phase: None,
            };
        }
        self.state = AiState::Chase;
        self.think_engaged(percept)
    }

    /// Chase and Attack: close the distance, then use whatever is ready.
    fn think_engaged(&mut self, percept: &AiPercept) -> AiDecision {
        let Some(target) = percept.victim() else {
            // Nothing left to fight: it turns round and starts walking in the same tick
            // rather than standing still for one, which would read as a hitch.
            self.state = AiState::Returning;
            return self.think_returning(percept);
        };

        let archetype = self.archetype();
        let facing = yaw_towards(percept.position, target.position);

        // Out of position: close. A dead band on the outside, so an enemy sitting at exactly
        // its preferred range does not twitch between two states. A victim out of sight is
        // out of position however close: nothing is shot through a wall, so the enemy comes
        // round it instead.
        let out_of_position =
            target.distance > archetype.preferred_range + archetype.range_slack || !target.visible;

        // The phase's list *is* the priority list: the first ability that is off cooldown,
        // in range, and has somebody to be used on is the one used. A boss with a rotation is
        // a longer list; a healer is a list whose first entry selects an ally.
        //
        // While closing, only a gap-closer — something with a minimum range, a charge or a
        // leap — is considered: that is what it is for, and a melee monster that stopped to
        // swing at somebody fifteen metres away would be a monster standing still.
        let choice = archetype
            .phase_abilities(self.phase)
            .filter_map(|ability| {
                let definition = ability.definition();
                if self.cooldowns[ability.index()] > 0.0 {
                    return None;
                }
                if out_of_position && definition.min_range <= 0.0 {
                    return None;
                }
                // Silenced: everything with a cast bar is off the list, and the instants
                // stay. Filtered here rather than refused later so a silenced caster picks
                // its swing instead of standing still holding a spell it cannot cast.
                if self.is_silenced() && definition.is_cast() {
                    return None;
                }
                if definition.requires == EnemyRequirement::VictimBleeding
                    && !percept.victim_bleeding
                {
                    return None;
                }
                let recipient = match definition.select {
                    // A random pick is made by the simulation when the cast starts; the brain
                    // only needs to know there is somebody to pick from, and its own target
                    // is proof of that.
                    EnemyTarget::Victim | EnemyTarget::Random => target,
                    EnemyTarget::WoundedAlly => percept.wounded_ally?,
                    EnemyTarget::Farthest => percept.farthest?,
                };
                // A frenzy is aimed at the enemy itself: it is ready whenever there is a fight.
                if definition.effect.is_self() {
                    return Some((ability, recipient));
                }
                // Nothing aimed at somebody goes through a wall: not an arrow, not a swing,
                // not a heal, not a charge.
                if !recipient.visible {
                    return None;
                }
                if recipient.distance < definition.min_range {
                    return None;
                }
                (recipient.distance <= definition.range).then_some((ability, recipient))
            })
            .next();
        if out_of_position && choice.is_none() {
            self.state = AiState::Chase;
            return AiDecision {
                intent: MoveIntent {
                    direction: direction_towards(percept.position, target.position),
                    jump: false,
                    facing: Some(facing),
                },
                request: AiRequest::None,
                phase: None,
            };
        }
        self.state = AiState::Attack;
        let Some((ability, recipient)) = choice else {
            // Nothing ready, and somebody is standing on a ranged monster: it backs away,
            // still facing them. Only when nothing is ready, so stepping back never costs it
            // a cast it would otherwise have made.
            if archetype.retreat_below > 0.0 && target.distance < archetype.retreat_below {
                let away = direction_towards(target.position, percept.position);
                return AiDecision {
                    intent: MoveIntent {
                        direction: away,
                        jump: false,
                        facing: Some(facing),
                    },
                    request: AiRequest::None,
                    phase: None,
                };
            }
            // Nothing ready: hold position and keep facing. Standing still rather than
            // circling is deliberate — movement the player cannot predict is not a
            // foundation, it is noise.
            return AiDecision {
                intent: MoveIntent {
                    direction: Vec3::ZERO,
                    jump: false,
                    facing: Some(facing),
                },
                request: AiRequest::None,
                phase: None,
            };
        };

        let definition = ability.definition();
        // The phase's cooldown scaling is applied here, once, when the ability is started —
        // not when it is read — so a phase change does not retroactively shorten a cooldown
        // that is already running.
        self.cooldowns[ability.index()] =
            definition.cooldown * archetype.phase_cooldown_scale(self.phase) / self.haste();
        let intent = MoveIntent {
            direction: Vec3::ZERO,
            jump: false,
            facing: Some(facing),
        };
        if definition.is_cast() {
            self.state = AiState::Cast;
            let anchor = match definition.aim {
                super::EnemyAim::Target => recipient.position,
                super::EnemyAim::Caster => percept.position,
            };
            // A line or a cone from the caster points at whoever it was aimed at, which for a
            // charge is not necessarily who the enemy is facing.
            let yaw = yaw_towards(percept.position, recipient.position);
            let mut targets = [None; MAX_TARGETS];
            targets[0] = Some(recipient.entity);
            let mut zone_targets = [None; MAX_ZONES];
            zone_targets[0] = Some(recipient.entity);
            self.cast = Some(EnemyCast {
                ability,
                elapsed: 0.0,
                duration: definition.cast_time,
                // Whoever the ability selected, which for a heal is an ally rather than the
                // thing the enemy is fighting.
                target: recipient.entity,
                anchor,
                interruptible: definition.interruptible,
                shape: aimed_shape(definition.shape, yaw),
                zone_count: 1,
                zones: [anchor; MAX_ZONES],
                aims: [recipient.position; MAX_ZONES],
                yaws: [yaw; MAX_ZONES],
                zone_targets,
                targets,
                ticks: 0,
            });
            return AiDecision {
                intent,
                request: AiRequest::BeginCast(ability),
                phase: None,
            };
        }
        self.state = AiState::Recover;
        self.timer = definition.recovery / self.haste();
        AiDecision {
            intent,
            request: AiRequest::Instant(ability),
            phase: None,
        }
    }

    /// Cast: stand still until it lands, or lose it.
    fn think_cast(&mut self, percept: &AiPercept) -> AiDecision {
        let Some(cast) = self.cast else {
            // The cast was taken by an interrupt between ticks; `stop_cast` already moved
            // the state on.
            self.state = AiState::Chase;
            return AiDecision::IDLE;
        };
        // Losing the target or the leash cancels the cast rather than letting it land on
        // nothing: the damage would have nowhere to go and the telegraph would be a lie.
        if percept.victim().is_none() {
            self.stop_cast(CastStop::Lost);
            self.state = AiState::Returning;
            return AiDecision {
                intent: MoveIntent::NONE,
                request: AiRequest::CancelCast(cast.ability),
                phase: None,
            };
        }

        let facing = percept
            .victim()
            .map(|target| yaw_towards(percept.position, target.position));
        if cast.remaining() > crate::TIME_EPSILON {
            return AiDecision {
                intent: MoveIntent {
                    direction: Vec3::ZERO,
                    jump: false,
                    // A caster turns to follow its target while charging, but the *shape*
                    // does not: the anchor was fixed when the cast started.
                    facing,
                },
                request: AiRequest::None,
                phase: None,
            };
        }

        self.cast = None;
        self.state = AiState::Recover;
        self.timer = cast.ability.definition().recovery / self.haste();
        AiDecision {
            intent: MoveIntent {
                direction: Vec3::ZERO,
                jump: false,
                facing,
            },
            request: AiRequest::CompleteCast(cast.ability),
            phase: None,
        }
    }

    /// Recover: the gap after an attack.
    fn think_recover(&mut self, percept: &AiPercept) -> AiDecision {
        if percept.victim().is_none() {
            self.state = AiState::Returning;
            return self.think_returning(percept);
        }
        if self.timer > 0.0 {
            let facing = percept
                .victim()
                .map(|target| yaw_towards(percept.position, target.position));
            return AiDecision {
                intent: MoveIntent {
                    direction: Vec3::ZERO,
                    jump: false,
                    facing,
                },
                request: AiRequest::None,
                phase: None,
            };
        }
        self.state = AiState::Chase;
        self.think_engaged(percept)
    }

    /// Returning: walk home, then forget the whole thing.
    fn think_returning(&mut self, percept: &AiPercept) -> AiDecision {
        let offset = Vec3::new(
            self.home.x - percept.position.x,
            0.0,
            self.home.z - percept.position.z,
        );
        if offset.length() <= HOME_TOLERANCE {
            self.state = AiState::Idle;
            self.target = None;
            self.threat.clear();
            self.combat_time = 0.0;
            return self.face_home(percept);
        }
        AiDecision {
            intent: MoveIntent {
                direction: direction_towards(percept.position, self.home),
                jump: false,
                facing: Some(yaw_towards(percept.position, self.home)),
            },
            request: AiRequest::None,
            phase: None,
        }
    }

    /// Stand at home facing the way it spawned.
    fn face_home(&self, _percept: &AiPercept) -> AiDecision {
        AiDecision {
            intent: MoveIntent {
                direction: Vec3::ZERO,
                jump: false,
                facing: Some(self.home_yaw),
            },
            request: AiRequest::None,
            phase: None,
        }
    }

    /// Moves it into the phase its health says it is in, if that is a later one.
    ///
    /// Returns the phase it entered, once, on the tick it entered it. A hit that crosses two
    /// thresholds at once lands in the deeper of the two and announces that one — there is no
    /// intermediate phase the player would never have seen.
    fn advance_phase(&mut self, health_fraction: f32) -> Option<usize> {
        let archetype = self.archetype();
        if archetype.phases.is_empty() {
            return None;
        }
        let phase = archetype.phase_for_health(health_fraction, self.phase);
        if phase == self.phase {
            return None;
        }
        self.phase = phase;
        // Cooldowns are not cleared: a phase change is not a free rotation reset, and an
        // ability the previous phase just used should still be on its cooldown in the next.
        Some(phase)
    }

    /// Whether it has been pulled further from home than it is willing to go.
    fn beyond_leash(&self, position: Vec3) -> bool {
        let offset = Vec3::new(position.x - self.home.x, 0.0, position.z - self.home.z);
        offset.length() > self.archetype().leash_radius
    }
}

/// The yaw that looks from `from` at `to`, radians about +Y. Zero faces −Z.
/// `shape` with a cone turned to face `yaw`.
///
/// The table cannot know which way something will be looking, so the yaw is filled in when
/// the attack starts and never again.
#[must_use]
pub fn aimed_shape(shape: TelegraphShape, yaw: f32) -> TelegraphShape {
    match shape {
        TelegraphShape::Cone {
            range, half_angle, ..
        } => TelegraphShape::Cone {
            yaw,
            range,
            half_angle,
        },
        TelegraphShape::Line { length, width, .. } => TelegraphShape::Line { yaw, length, width },
        other => other,
    }
}

/// The yaw that looks from `from` at `to`, radians about +Y. Zero faces -Z.
#[must_use]
pub fn yaw_towards(from: Vec3, to: Vec3) -> f32 {
    let offset = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
    if offset.length() <= f32::EPSILON {
        return 0.0;
    }
    // Matches `movement`'s own convention: yaw 0 looks down −Z, and +yaw turns towards −X.
    (-offset.x).atan2(-offset.z)
}

/// A unit direction from `from` towards `to`, flattened onto the ground.
#[must_use]
pub fn direction_towards(from: Vec3, to: Vec3) -> Vec3 {
    let offset = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
    let length = offset.length();
    if length <= f32::EPSILON {
        return Vec3::ZERO;
    }
    offset * (1.0 / length)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    fn player() -> EntityId {
        EntityId::new(0, 1)
    }

    fn brain() -> EnemyBrain {
        EnemyBrain::new(EnemyKind::Melee, Vec3::ZERO, 0.0)
    }

    /// A percept with the player `distance` metres away, straight ahead.
    fn percept(position: Vec3, distance: f32, engaged: bool) -> AiPercept {
        let target = AiTarget {
            entity: player(),
            position: Vec3::new(position.x, 0.0, position.z - distance),
            distance,
            visible: true,
        };
        AiPercept {
            position,
            alive: true,
            target: engaged.then_some(target),
            nearest: Some(target),
            health_fraction: 1.0,
            wounded_ally: None,
            farthest: None,
            victim_bleeding: false,
        }
    }

    /// Runs `ticks` of thinking against a fixed percept.
    fn run(brain: &mut EnemyBrain, percept: &AiPercept, ticks: u32) -> AiDecision {
        let mut decision = AiDecision::IDLE;
        for _ in 0..ticks {
            decision = brain.think(percept, STEP);
        }
        decision
    }

    #[test]
    fn an_idle_enemy_ignores_somebody_far_away() {
        let mut brain = brain();
        run(&mut brain, &percept(Vec3::ZERO, 30.0, false), 60);
        assert_eq!(brain.state(), AiState::Idle);
        assert_eq!(brain.target(), None);
    }

    #[test]
    fn walking_into_the_aggro_radius_goes_idle_alert_chase() {
        let mut brain = brain();
        let near = percept(Vec3::ZERO, 10.0, false);
        brain.think(&near, STEP);
        assert_eq!(brain.state(), AiState::Alert, "noticing is its own state");

        // The alert pause is the archetype's, and it does not move during it.
        let decision = run(&mut brain, &near, 5);
        assert_eq!(decision.intent.direction, Vec3::ZERO);
        assert_eq!(brain.state(), AiState::Alert);

        let engaged = percept(Vec3::ZERO, 10.0, true);
        run(&mut brain, &engaged, 40);
        assert_eq!(brain.state(), AiState::Chase);
        assert!(brain.state().is_combat());
    }

    #[test]
    fn a_ranged_pull_works_from_outside_the_aggro_radius() {
        // Nobody nearby, but somebody has hurt it: that is a pull.
        let mut brain = brain();
        brain.add_threat(player(), 40.0);
        let far = AiPercept {
            position: Vec3::ZERO,
            alive: true,
            target: Some(AiTarget {
                entity: player(),
                position: Vec3::new(0.0, 0.0, -30.0),
                distance: 30.0,
                visible: true,
            }),
            nearest: None,
            health_fraction: 1.0,
            wounded_ally: None,
            farthest: None,
            victim_bleeding: false,
        };
        brain.think(&far, STEP);
        assert_eq!(brain.state(), AiState::Alert);
        assert_eq!(brain.target(), Some(player()));
    }

    #[test]
    fn it_chases_when_far_and_stops_when_close() {
        let mut brain = brain();
        let far = percept(Vec3::ZERO, 10.0, true);
        let decision = run(&mut brain, &far, 30);
        assert_eq!(brain.state(), AiState::Chase);
        assert!(decision.intent.direction.length() > 0.9, "it must move");

        let close = percept(Vec3::ZERO, 1.0, true);
        let decision = brain.think(&close, STEP);
        assert_eq!(decision.intent.direction, Vec3::ZERO, "it must stop");
        assert!(matches!(
            brain.state(),
            AiState::Attack | AiState::Cast | AiState::Recover
        ));
    }

    #[test]
    fn attacking_puts_the_ability_on_cooldown_and_recovers() {
        let mut brain = EnemyBrain::new(EnemyKind::Melee, Vec3::ZERO, 0.0);
        let close = percept(Vec3::ZERO, 1.0, true);
        run(&mut brain, &close, 40); // through Alert
        // The melee enemy prefers Cleave, which is a cast.
        assert_eq!(brain.state(), AiState::Cast);
        assert!(brain.cast().is_some());
        assert!(brain.cooldown(EnemyAbility::Cleave) > 0.0);

        // It lands, and then it is in recovery rather than swinging again immediately.
        let mut landed = false;
        for _ in 0..200 {
            let decision = brain.think(&close, STEP);
            if decision.request == AiRequest::CompleteCast(EnemyAbility::Cleave) {
                landed = true;
                break;
            }
        }
        assert!(landed, "the cast never landed");
        assert_eq!(brain.state(), AiState::Recover);
        assert!(brain.cast().is_none());
    }

    #[test]
    fn an_instant_attack_needs_no_cast_and_still_recovers() {
        let mut brain = brain();
        let close = percept(Vec3::ZERO, 1.0, true);
        run(&mut brain, &close, 40);
        // Burn the Cleave so only the Swing is left.
        let mut swung = false;
        for _ in 0..600 {
            let decision = brain.think(&close, STEP);
            if decision.request == AiRequest::Instant(EnemyAbility::Swing) {
                swung = true;
                assert_eq!(brain.state(), AiState::Recover);
                break;
            }
        }
        assert!(swung, "the melee enemy never swung");
    }

    #[test]
    fn an_interrupt_stops_the_cast_and_leaves_a_gap() {
        let mut brain = brain();
        let close = percept(Vec3::ZERO, 1.0, true);
        run(&mut brain, &close, 40);
        assert_eq!(brain.state(), AiState::Cast);
        assert!(brain.is_interruptible());

        let stopped = brain.stop_cast(CastStop::Interrupted).expect("a cast");
        assert_eq!(stopped.ability, EnemyAbility::Cleave);
        assert_eq!(brain.state(), AiState::Recover);
        assert!(brain.timer() > 0.5, "an interrupt must cost it time");
        assert!(brain.cast().is_none());
        assert!(!brain.is_interruptible());
    }

    #[test]
    fn losing_the_target_sends_it_home_and_it_forgets_everything() {
        let mut brain = brain();
        let close = percept(Vec3::new(0.0, 0.0, -20.0), 1.0, true);
        run(&mut brain, &close, 40);
        brain.add_threat(player(), 100.0);
        assert!(brain.state().is_combat());

        let lost = AiPercept {
            position: Vec3::new(0.0, 0.0, -20.0),
            alive: true,
            target: None,
            nearest: None,
            health_fraction: 1.0,
            wounded_ally: None,
            farthest: None,
            victim_bleeding: false,
        };
        // The tick it loses its target it drops the cast; from the next one it walks.
        brain.think(&lost, STEP);
        assert_eq!(brain.state(), AiState::Returning);
        let decision = brain.think(&lost, STEP);
        assert!(
            decision.intent.direction.z > 0.0,
            "it walks back towards home"
        );

        // And when it gets there it is exactly as it started.
        let home = AiPercept {
            position: Vec3::new(0.0, 0.0, -0.2),
            ..lost
        };
        brain.think(&home, STEP);
        assert_eq!(brain.state(), AiState::Idle);
        assert!(brain.threat().is_empty());
        assert_eq!(brain.target(), None);
    }

    #[test]
    fn a_fight_never_leashes_while_the_victim_lives() {
        let mut brain = brain();
        brain.add_threat(player(), 5_000.0);
        // Engage close to home first.
        run(&mut brain, &percept(Vec3::ZERO, 1.0, true), 60);
        assert!(brain.state().is_combat());
        // Then be led a hundred metres away: it keeps fighting.
        let far_from_home = Vec3::new(0.0, 0.0, -100.0);
        run(&mut brain, &percept(far_from_home, 1.0, true), 120);
        assert_ne!(
            brain.state(),
            AiState::Returning,
            "it gave up on a living victim"
        );
        assert!(
            !brain.threat().is_empty(),
            "it still remembers who it is fighting"
        );
    }

    #[test]
    fn an_idle_monster_pushed_past_its_leash_walks_home() {
        let mut brain = brain();
        let far_from_home = Vec3::new(0.0, 0.0, -100.0);
        let alone = AiPercept {
            target: None,
            nearest: None,
            ..percept(far_from_home, 1.0, true)
        };
        let decision = brain.think(&alone, STEP);
        assert_eq!(brain.state(), AiState::Returning);
        assert!(
            decision.intent.direction.z > 0.0,
            "it walks back towards home"
        );
    }

    #[test]
    fn dying_stops_everything_and_reports_the_cast_it_was_running() {
        let mut brain = brain();
        let close = percept(Vec3::ZERO, 1.0, true);
        run(&mut brain, &close, 40);
        assert_eq!(brain.state(), AiState::Cast);

        let dead = AiPercept {
            alive: false,
            ..close
        };
        let decision = brain.think(&dead, STEP);
        assert_eq!(brain.state(), AiState::Dead);
        assert_eq!(
            decision.request,
            AiRequest::CancelCast(EnemyAbility::Cleave)
        );
        assert!(brain.cast().is_none());
        // And it stays dead, doing nothing, however close the player stands.
        assert_eq!(brain.think(&close, STEP), AiDecision::IDLE);
        assert_eq!(brain.state(), AiState::Dead);
    }

    #[test]
    fn a_reset_puts_it_back_exactly_as_it_spawned() {
        let mut brain = brain();
        let close = percept(Vec3::ZERO, 1.0, true);
        run(&mut brain, &close, 60);
        brain.add_threat(player(), 100.0);
        brain.reset();

        assert_eq!(brain.state(), AiState::Idle);
        assert_eq!(brain.target(), None);
        assert!(brain.threat().is_empty());
        assert!(brain.cast().is_none());
        for ability in EnemyAbility::ALL {
            assert_eq!(brain.cooldown(ability), 0.0);
        }
    }

    #[test]
    fn the_same_state_and_the_same_percept_produce_the_same_decision() {
        // Determinism, at the smallest scale it can be asserted: two brains, one input.
        let close = percept(Vec3::ZERO, 4.0, true);
        let mut first = brain();
        let mut second = brain();
        for _ in 0..300 {
            assert_eq!(first.think(&close, STEP), second.think(&close, STEP));
            assert_eq!(first, second);
        }
    }

    #[test]
    fn a_nonsense_step_neither_advances_nor_rewinds_anything() {
        let mut brain = brain();
        let close = percept(Vec3::ZERO, 1.0, true);
        run(&mut brain, &close, 40);
        let before = brain.clone();
        for step in [f32::NAN, -1.0, f32::INFINITY] {
            brain.think(&close, step);
        }
        assert_eq!(brain.state(), before.state());
    }

    #[test]
    fn a_caster_keeps_its_distance_and_a_brute_closes() {
        // Both see as far as every monster does. Past it, neither notices; inside it, the
        // caster is already in range and holds, and the brute has to close to arm's length.
        let beyond = percept(Vec3::ZERO, super::super::AGGRO_RADIUS + 1.0, true);
        let inside = percept(Vec3::ZERO, super::super::AGGRO_RADIUS - 1.0, true);

        let mut caster = EnemyBrain::new(EnemyKind::Caster, Vec3::ZERO, 0.0);
        run(&mut caster, &beyond, 60);
        assert_eq!(caster.state(), AiState::Idle, "outside the aggro radius");
        run(&mut caster, &inside, 60);
        assert!(caster.state().is_combat(), "{:?}", caster.state());
        let decision = caster.think(&inside, STEP);
        assert_eq!(decision.intent.direction, Vec3::ZERO, "it stops at range");

        let mut brute = brain();
        run(&mut brute, &beyond, 60);
        assert_eq!(brute.state(), AiState::Idle, "outside the aggro radius");
        run(&mut brute, &inside, 60);
        assert_eq!(brute.state(), AiState::Chase, "melee has to close");
    }

    #[test]
    fn yaw_and_direction_agree_about_which_way_the_target_is() {
        let from = Vec3::ZERO;
        for to in [
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(-3.0, 0.0, 4.0),
        ] {
            let yaw = yaw_towards(from, to);
            let facing = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
            let direction = direction_towards(from, to);
            assert!(
                facing.dot(direction) > 0.99,
                "{to:?}: {facing:?} {direction:?}"
            );
        }
        assert_eq!(direction_towards(from, from), Vec3::ZERO);
        assert_eq!(yaw_towards(from, from), 0.0);
    }
}
