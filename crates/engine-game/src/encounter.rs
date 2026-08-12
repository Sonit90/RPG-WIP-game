//! A fight, as an object: who is in it, what state it is in, and how to put it back.
//!
//! ```text
//! NotStarted ──▶ Active ──▶ Completed
//!      ▲            │            │
//!      └──────── Resetting ◀─────┘
//! ```
//!
//! # Why this is not inside the AI
//!
//! An enemy knows about its own leash and its own home. It does not know that it is the
//! second of five, that the fight is over when all five are down, or that a reset means
//! everybody comes back. Those are properties of the *encounter*, and putting them in the
//! brain is how "wipe and retry" ends up implemented five times, once per enemy type.
//!
//! # What it holds
//!
//! Spawn points — a kind and a place — and the entities they became. Nothing else: it does
//! not own health, positions or AI, because those live on the entities and are reset through
//! the simulation, which is the only thing allowed to change them.
//!
//! # Deliberately not here
//!
//! Waves, phases, scripted timelines, adds, loot and objectives. Every one of them is a
//! layer *above* this lifecycle, and none of them changes its shape.

use engine_core::math::Vec3;

use crate::enemy::EnemyKind;
use crate::entity::EntityId;

/// Where the fight is in its life.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncounterState {
    /// Built, populated, nobody has been pulled.
    #[default]
    NotStarted,
    /// At least one member has been pulled and is still alive.
    Active,
    /// Every member is dead.
    Completed,
    /// Being put back to [`EncounterState::NotStarted`].
    ///
    /// A state rather than an instant, because a reset is a *request* the simulation
    /// carries out on its next tick: resetting in the middle of resolving a tick's damage
    /// would despawn entities that this tick's events still name.
    Resetting,
}

impl EncounterState {
    /// Short uppercase name, for the developer readout.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "NOT STARTED",
            Self::Active => "ACTIVE",
            Self::Completed => "COMPLETED",
            Self::Resetting => "RESETTING",
        }
    }

    /// Whether the fight is running.
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

/// One enemy the encounter puts in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemySpawn {
    /// What to spawn.
    pub kind: EnemyKind,
    /// Where its feet go.
    pub position: Vec3,
    /// Which way it faces while it is idle.
    pub facing_yaw: f32,
}

impl EnemySpawn {
    /// A `kind` standing at `position`, facing −Z.
    #[must_use]
    pub fn new(kind: EnemyKind, position: Vec3) -> Self {
        Self {
            kind,
            position,
            facing_yaw: 0.0,
        }
    }

    /// The same spawn, facing `yaw`.
    #[must_use]
    pub fn facing(mut self, yaw: f32) -> Self {
        self.facing_yaw = yaw;
        self
    }
}

/// A fight: its members, its spawn points and its state.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Encounter {
    name: &'static str,
    state: EncounterState,
    spawns: Vec<EnemySpawn>,
    /// The entity each spawn point currently is, in the same order.
    members: Vec<EntityId>,
    /// How many times it has been reset, for the readout.
    resets: u32,
}

impl Encounter {
    /// An empty encounter named `name`.
    #[must_use]
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            state: EncounterState::NotStarted,
            spawns: Vec::new(),
            members: Vec::new(),
            resets: 0,
        }
    }

    /// An encounter with these spawn points, not yet populated.
    #[must_use]
    pub fn with_spawns(name: &'static str, spawns: Vec<EnemySpawn>) -> Self {
        Self {
            name,
            state: EncounterState::NotStarted,
            members: Vec::with_capacity(spawns.len()),
            spawns,
            resets: 0,
        }
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Where it is in its life.
    #[must_use]
    pub fn state(&self) -> EncounterState {
        self.state
    }

    /// Its spawn points.
    #[must_use]
    pub fn spawns(&self) -> &[EnemySpawn] {
        &self.spawns
    }

    /// The entities currently standing in for its spawn points.
    #[must_use]
    pub fn members(&self) -> &[EntityId] {
        &self.members
    }

    /// How many enemies it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether it holds nobody.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// How many times it has been reset.
    #[must_use]
    pub fn resets(&self) -> u32 {
        self.resets
    }

    /// Whether `entity` belongs to this encounter.
    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.members.contains(&entity)
    }

    /// Adds a spawn point. Only meaningful before the encounter is populated.
    pub fn push_spawn(&mut self, spawn: EnemySpawn) {
        self.spawns.push(spawn);
    }

    /// Records the entities the spawn points became.
    ///
    /// Called by the simulation, which is the only thing that can spawn.
    pub fn set_members(&mut self, members: Vec<EntityId>) {
        self.members = members;
        self.state = EncounterState::NotStarted;
    }

    /// Marks the fight as running. Idempotent.
    ///
    /// A fight starts when an enemy is *pulled*, not when the player walks into the room:
    /// the simulation calls this the first time a member leaves Idle.
    pub fn activate(&mut self) {
        if matches!(self.state, EncounterState::NotStarted) {
            self.state = EncounterState::Active;
        }
    }

    /// Marks the fight as won.
    pub fn complete(&mut self) {
        if self.state.is_active() {
            self.state = EncounterState::Completed;
        }
    }

    /// Asks for a reset. The simulation performs it on its next tick.
    pub fn request_reset(&mut self) {
        self.state = EncounterState::Resetting;
    }

    /// Whether a reset is pending.
    #[must_use]
    pub fn is_resetting(&self) -> bool {
        matches!(self.state, EncounterState::Resetting)
    }

    /// Declares the reset finished.
    pub fn finish_reset(&mut self) {
        self.state = EncounterState::NotStarted;
        self.resets += 1;
    }

    /// Updates the state from how many members are still alive.
    ///
    /// Called once a tick by the simulation with the count it already has. Returns whether
    /// the state changed, so the caller can announce a completion exactly once.
    pub fn observe_alive(&mut self, alive: usize) -> bool {
        match self.state {
            EncounterState::Active if alive == 0 => {
                self.state = EncounterState::Completed;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn encounter() -> Encounter {
        Encounter::with_spawns(
            "TEST",
            vec![
                EnemySpawn::new(EnemyKind::Melee, Vec3::new(0.0, 0.0, -10.0)),
                EnemySpawn::new(EnemyKind::Caster, Vec3::new(6.0, 0.0, -14.0)).facing(1.0),
            ],
        )
    }

    #[test]
    fn a_fresh_encounter_has_not_started() {
        let encounter = encounter();
        assert_eq!(encounter.state(), EncounterState::NotStarted);
        assert_eq!(encounter.spawns().len(), 2);
        assert!(
            encounter.is_empty(),
            "nothing is spawned until it is populated"
        );
    }

    #[test]
    fn it_runs_from_not_started_to_completed_and_back() {
        let mut encounter = encounter();
        encounter.set_members(vec![EntityId::new(1, 1), EntityId::new(2, 1)]);
        assert_eq!(encounter.len(), 2);

        encounter.activate();
        assert!(encounter.state().is_active());
        // Activating twice changes nothing.
        encounter.activate();
        assert!(encounter.state().is_active());

        assert!(!encounter.observe_alive(1), "one alive is still a fight");
        assert!(encounter.observe_alive(0), "the last death completes it");
        assert_eq!(encounter.state(), EncounterState::Completed);
        assert!(!encounter.observe_alive(0), "completion is announced once");

        encounter.request_reset();
        assert!(encounter.is_resetting());
        encounter.finish_reset();
        assert_eq!(encounter.state(), EncounterState::NotStarted);
        assert_eq!(encounter.resets(), 1);
    }

    #[test]
    fn an_encounter_that_never_started_cannot_complete() {
        let mut encounter = encounter();
        encounter.set_members(vec![EntityId::new(1, 1)]);
        assert!(!encounter.observe_alive(0));
        assert_eq!(encounter.state(), EncounterState::NotStarted);
    }

    #[test]
    fn membership_is_by_entity() {
        let mut encounter = encounter();
        let member = EntityId::new(3, 2);
        encounter.set_members(vec![member]);
        assert!(encounter.contains(member));
        assert!(
            !encounter.contains(EntityId::new(3, 3)),
            "a stale id is not a member"
        );
    }
}
