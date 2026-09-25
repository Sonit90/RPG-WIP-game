//! A dungeon *run*: how complete it is, whether the boss is open, and who the boss called.
//!
//! ```text
//!   kill ──▶ completion value ──▶ credited / required ──▶ fraction ──▶ 100 % ──▶ gate opens
//!                                                                              │
//!   boss pulled ──▶ two adds chosen from the pool ──▶ spawned in their empowered form
//!                                                                              │
//!   boss and adds dead ──────────────────────────────────────────▶ dungeon completed
//! ```
//!
//! # Whose numbers these are
//!
//! Every number that is content — how much a monster is worth, how much counts as 100 %,
//! which pack is the boss, where the gate stands, where adds appear and which monsters they
//! may be — comes from the level ([`engine_environment::DungeonSettings`]) and the monster
//! table ([`crate::enemy::EnemyArchetype::completion`]). This module is the arithmetic and the
//! state, and nothing in it names a monster.
//!
//! # Weighted, not counted
//!
//! Completion is the sum of what was killed, against a requirement. A level can hold more
//! than the requirement — which is what makes a route optional — and the percentage is
//! clamped at 100 however much more is killed. It is the same idea as the forces count of
//! modern dungeon systems, implemented as a weighted sum rather than copied from any of them.
//!
//! # Held by the simulation
//!
//! A run is ticked with the fight it describes, so it lives on
//! [`crate::sim::Simulation`] beside the encounters it reads; see `sim::dungeon_exec` for the
//! half that spawns, seals and unlocks. Presentation reads it through
//! [`crate::sim::Simulation::dungeon_run`] and never writes it.

use engine_core::math::Vec3;
use engine_environment::BossGate;

use crate::enemy::EnemyKind;
use crate::entity::EntityId;

/// What a dungeon asks of a run, resolved from the level.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonRules {
    /// What it is called, for the progress panel.
    pub name: String,
    /// The completion value that counts as 100 %.
    pub required: f32,
    /// Which encounter is the boss, by index into the simulation's encounters.
    pub boss_encounter: Option<usize>,
    /// The barrier the boss waits behind until the requirement is met.
    pub gate: Option<BossGate>,
    /// Where adds appear, and which way they face.
    pub add_spawns: Vec<(Vec3, f32)>,
    /// How many adds the boss calls.
    pub add_count: usize,
    /// Which monsters an add may be. Each is spawned in its empowered form.
    pub add_pool: Vec<EnemyKind>,
    /// Whether one monster may be called twice.
    pub allow_duplicate_adds: bool,
}

impl DungeonRules {
    /// A dungeon with a name and a requirement and nothing else.
    #[must_use]
    pub fn new(name: impl Into<String>, required: f32) -> Self {
        Self {
            name: name.into(),
            required: required.max(f32::EPSILON),
            boss_encounter: None,
            gate: None,
            add_spawns: Vec::new(),
            add_count: 0,
            add_pool: Vec::new(),
            allow_duplicate_adds: false,
        }
    }
}

/// One run through a dungeon.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonRun {
    rules: DungeonRules,
    /// Completion credited so far, in the dungeon's own units.
    credited: f32,
    /// Whether the requirement has been met and the gate opened.
    unlocked: bool,
    /// Whether the boss fight is running: pulled, and its adds called.
    engaged: bool,
    /// The adds the boss called this attempt, in the order they were called.
    adds: Vec<EntityId>,
    /// Which monsters they were, before being empowered.
    chosen: Vec<EnemyKind>,
    /// The adds a test or a developer asked for, instead of a random pick.
    forced: Option<Vec<EnemyKind>>,
    /// Whether the boss and every add are dead.
    completed: bool,
    /// Simulation seconds at the first pull, for the completion time.
    started_at: Option<f64>,
    /// How long the run took, once it is over.
    finished_after: Option<f32>,
    /// How many times the run has been reset.
    resets: u32,
}

impl DungeonRun {
    /// A fresh run of `rules`.
    #[must_use]
    pub fn new(rules: DungeonRules) -> Self {
        Self {
            rules,
            credited: 0.0,
            unlocked: false,
            engaged: false,
            adds: Vec::new(),
            chosen: Vec::new(),
            forced: None,
            completed: false,
            started_at: None,
            finished_after: None,
            resets: 0,
        }
    }

    /// What the dungeon asks.
    #[must_use]
    pub fn rules(&self) -> &DungeonRules {
        &self.rules
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.rules.name
    }

    /// Completion credited so far, in the dungeon's own units. Not clamped.
    #[must_use]
    pub fn credited(&self) -> f32 {
        self.credited
    }

    /// The completion value that counts as 100 %.
    #[must_use]
    pub fn required(&self) -> f32 {
        self.rules.required
    }

    /// Completion as a fraction of the requirement, clamped to `[0, 1]`.
    ///
    /// Clamped because killing more than the requirement is allowed — an optional route
    /// cleared on top of the main one — and a panel reading 118 % would be answering a
    /// question nobody asked.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (self.credited / self.rules.required).clamp(0.0, 1.0)
    }

    /// Completion as a whole percentage, `0..=100`, rounded down.
    ///
    /// Down, so 99.6 % reads 99 and not a 100 that has not opened the gate.
    #[must_use]
    pub fn percent(&self) -> u32 {
        (self.fraction() * 100.0 + 1e-3).floor() as u32
    }

    /// Whether the requirement has been met.
    #[must_use]
    pub fn is_unlocked(&self) -> bool {
        self.unlocked
    }

    /// Whether the boss fight is running.
    #[must_use]
    pub fn is_engaged(&self) -> bool {
        self.engaged
    }

    /// The adds the boss called this attempt.
    #[must_use]
    pub fn adds(&self) -> &[EntityId] {
        &self.adds
    }

    /// Which monsters the adds were, before being empowered.
    #[must_use]
    pub fn chosen(&self) -> &[EnemyKind] {
        &self.chosen
    }

    /// Whether the boss and every add are dead.
    #[must_use]
    pub fn is_completed(&self) -> bool {
        self.completed
    }

    /// How long the run took, once it is over.
    #[must_use]
    pub fn finished_after(&self) -> Option<f32> {
        self.finished_after
    }

    /// How many times it has been reset.
    #[must_use]
    pub fn resets(&self) -> u32 {
        self.resets
    }

    /// Asks for these adds instead of a random pick, from the next pull on.
    ///
    /// What a test uses to make "the boss calls a Brute and a Ghoul" a fact rather than a
    /// seed, and what a developer uses to practise against one pair. `None` goes back to
    /// random.
    pub fn force_adds(&mut self, kinds: Option<Vec<EnemyKind>>) {
        self.forced = kinds;
    }

    /// The adds that were asked for, if any.
    #[must_use]
    pub fn forced_adds(&self) -> Option<&[EnemyKind]> {
        self.forced.as_deref()
    }

    /// Credits `value` of completion. Returns whether this is the credit that met the
    /// requirement, so the gate opens exactly once.
    pub fn credit(&mut self, value: f32) -> bool {
        if !value.is_finite() || value <= 0.0 {
            return false;
        }
        self.credited += value;
        if !self.unlocked && self.credited + 1e-4 >= self.rules.required {
            self.unlocked = true;
            return true;
        }
        false
    }

    /// Opens the gate whatever the count says. A development control.
    pub fn unlock(&mut self) -> bool {
        let changed = !self.unlocked;
        self.unlocked = true;
        changed
    }

    /// Notes that the run's first fight has started, for the completion time.
    pub fn note_started(&mut self, now: f64) {
        if self.started_at.is_none() {
            self.started_at = Some(now);
        }
    }

    /// Records that the boss fight began with these adds.
    pub fn engage(&mut self, adds: Vec<EntityId>, chosen: Vec<EnemyKind>) {
        self.engaged = true;
        self.adds = adds;
        self.chosen = chosen;
    }

    /// Records that the boss fight was abandoned, and returns the adds to take away.
    pub fn disengage(&mut self) -> Vec<EntityId> {
        self.engaged = false;
        self.chosen.clear();
        std::mem::take(&mut self.adds)
    }

    /// Records that the dungeon is done. Returns the run's time, once.
    pub fn complete(&mut self, now: f64) -> Option<f32> {
        if self.completed {
            return None;
        }
        self.completed = true;
        let seconds = self
            .started_at
            .map_or(0.0, |started| (now - started).max(0.0) as f32);
        self.finished_after = Some(seconds);
        Some(seconds)
    }

    /// Puts the run back to its start: nothing credited, the gate shut, no boss fight.
    ///
    /// Returns the adds that were standing, for the caller to take out of the world. A forced
    /// add choice is kept, because it is a developer's setting rather than a fact about the
    /// attempt.
    pub fn reset(&mut self) -> Vec<EntityId> {
        self.credited = 0.0;
        self.unlocked = false;
        self.completed = false;
        self.started_at = None;
        self.finished_after = None;
        self.resets += 1;
        self.disengage()
    }
}

/// Chooses `count` adds from `pool` using `random`, which answers numbers in `[0, 1)`.
///
/// Without duplicates unless they are allowed — and a pool smaller than `count` with
/// duplicates forbidden yields the whole pool rather than repeating, which is the promise
/// [`engine_environment::DungeonSettings::is_valid`] checks at load.
pub fn choose_adds(
    pool: &[EnemyKind],
    count: usize,
    allow_duplicates: bool,
    mut random: impl FnMut() -> f32,
) -> Vec<EnemyKind> {
    let mut chosen = Vec::with_capacity(count);
    if pool.is_empty() {
        return chosen;
    }
    let mut remaining: Vec<EnemyKind> = pool.to_vec();
    for _ in 0..count {
        let source: &[EnemyKind] = if allow_duplicates { pool } else { &remaining };
        if source.is_empty() {
            break;
        }
        let index = ((random() * source.len() as f32) as usize).min(source.len() - 1);
        let kind = source[index];
        chosen.push(kind);
        if !allow_duplicates {
            remaining.remove(index);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enemy::GRAVEYARD_ROSTER;

    #[test]
    fn completion_is_a_weighted_sum_clamped_at_the_requirement() {
        let mut run = DungeonRun::new(DungeonRules::new("CRYPT", 100.0));
        assert!(!run.credit(40.0));
        assert_eq!(run.percent(), 40);
        assert!(
            run.credit(60.0),
            "the credit that meets the requirement says so"
        );
        assert!(run.is_unlocked());
        assert!(!run.credit(30.0), "and only that one does");
        assert_eq!(run.percent(), 100, "never more than a hundred");
        assert!((run.fraction() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn nonsense_credit_is_ignored() {
        let mut run = DungeonRun::new(DungeonRules::new("CRYPT", 10.0));
        for value in [f32::NAN, -5.0, 0.0, f32::INFINITY] {
            run.credit(value);
        }
        assert_eq!(run.credited(), 0.0);
    }

    #[test]
    fn ninety_nine_point_six_is_ninety_nine() {
        let mut run = DungeonRun::new(DungeonRules::new("CRYPT", 100.0));
        run.credit(99.6);
        assert_eq!(run.percent(), 99);
        assert!(!run.is_unlocked());
    }

    #[test]
    fn a_reset_puts_everything_back_and_hands_over_the_adds() {
        let mut run = DungeonRun::new(DungeonRules::new("CRYPT", 10.0));
        run.credit(12.0);
        let add = EntityId::new(5, 1);
        run.engage(vec![add], vec![EnemyKind::GraveBrute]);
        run.force_adds(Some(vec![EnemyKind::CryptGhoul]));
        let adds = run.reset();
        assert_eq!(adds, vec![add]);
        assert_eq!(run.credited(), 0.0);
        assert!(!run.is_unlocked() && !run.is_engaged() && !run.is_completed());
        assert!(
            run.forced_adds().is_some(),
            "a forced choice is a setting, not a fact"
        );
    }

    #[test]
    fn two_adds_from_six_are_two_different_monsters() {
        let mut state = 0.37_f32;
        for _ in 0..200 {
            let chosen = choose_adds(&GRAVEYARD_ROSTER, 2, false, || {
                state = (state * 7.13 + 0.311).fract();
                state
            });
            assert_eq!(chosen.len(), 2);
            assert_ne!(chosen[0], chosen[1]);
            assert!(chosen.iter().all(|kind| GRAVEYARD_ROSTER.contains(kind)));
        }
    }

    #[test]
    fn duplicates_only_when_allowed() {
        let chosen = choose_adds(&[EnemyKind::GraveBrute], 2, true, || 0.0);
        assert_eq!(chosen, vec![EnemyKind::GraveBrute, EnemyKind::GraveBrute]);
        let chosen = choose_adds(&[EnemyKind::GraveBrute], 2, false, || 0.0);
        assert_eq!(chosen, vec![EnemyKind::GraveBrute]);
    }
}
