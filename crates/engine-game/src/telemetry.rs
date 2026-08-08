//! The combat meter: what the events add up to.
//!
//! ```text
//! Combat simulation -> CombatEvent -> CombatMeter -> overlay
//!                                     ^^^^^^^^^^^
//!                                     reads, never hooks
//! ```
//!
//! The meter is an **observer**. It is not called by the ability system, it registers
//! nothing, and removing it would not change a single damage number. It reads a slice of
//! [`CombatEvent`]s and accumulates. That is the Milestone 16 requirement — damage is
//! observable without the damage code knowing about the observer — and it is why a combat
//! log, floating numbers or a replication layer can be added later by reading the same
//! slice.
//!
//! ## Why DPS is measured in simulation time
//!
//! Frames are not a unit of time: the renderer runs anywhere from 240 to 1500 FPS, so a
//! per-frame or per-render-second rate would report a different number on a faster machine
//! for identical combat. Every timestamp here is simulation time accumulated from the fixed
//! timestep, which is the same on every machine.

use crate::ability::{ABILITY_SLOTS, AbilityId};
use crate::entity::EntityId;
use crate::event::CombatEvent;

/// Shortest window a DPS figure is computed over, in seconds.
///
/// One instant of damage is not an infinite rate. Without a floor, a single Fireball 45
/// milliseconds into a session reports tens of thousands of DPS and then decays, which
/// makes the number useless exactly when a tester is looking at it.
pub const MINIMUM_DPS_WINDOW: f64 = 1.0;

/// What one ability did this session.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct AbilityTotals {
    /// Damage dealt.
    pub damage: f64,
    /// Healing done.
    pub healing: f64,
    /// Times it was used.
    pub casts: u32,
    /// Individual damage instances it produced. An area cast that hits four dummies is one
    /// cast and four hits.
    pub hits: u32,
}

/// What one target took this session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetTotals {
    /// Who it was.
    pub entity: EntityId,
    /// Damage taken.
    pub damage: f64,
    /// Damage instances taken.
    pub hits: u32,
    /// Whether they died during the session.
    pub died: bool,
}

/// Damage, healing and timing for one combat session.
///
/// Allocation: the per-ability totals are a fixed array, and the per-target list and its
/// slot index are `Vec`s that are cleared rather than freed on reset. A session that has
/// been reset a hundred times allocates as much as one that has never been.
#[derive(Debug, Default, Clone)]
pub struct CombatMeter {
    abilities: [AbilityTotals; ABILITY_SLOTS],
    targets: Vec<TargetTotals>,
    /// Entity slot to index in `targets`, plus one. Zero means "not seen yet", which is
    /// what makes attributing a hit a lookup rather than a scan of every target.
    by_slot: Vec<u32>,
    total_damage: f64,
    total_healing: f64,
    total_overkill: f64,
    deaths: u32,
    first_damage_at: Option<f64>,
    last_damage_at: f64,
    session_started_at: f64,
    now: f64,
}

impl CombatMeter {
    /// An empty session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Accumulates a slice of events that happened at simulation time `now`.
    ///
    /// Called once per simulation tick with that tick's events. Cheap enough to be in the
    /// tick: a handful of adds per event and no allocation once the target list has seen
    /// everyone.
    pub fn observe(&mut self, events: &[CombatEvent], now: f64) {
        self.now = now;
        for event in events {
            match *event {
                CombatEvent::AbilityUsed { ability, .. } => {
                    if let Some(totals) = self.ability_mut(ability) {
                        totals.casts += 1;
                    }
                }
                CombatEvent::Damage(damage) => {
                    self.total_damage += f64::from(damage.amount);
                    self.total_overkill += f64::from(damage.overkill);
                    if let Some(totals) = self.ability_mut(damage.ability) {
                        totals.damage += f64::from(damage.amount);
                        totals.hits += 1;
                    }
                    let index = self.target_index(damage.target);
                    let target = &mut self.targets[index];
                    target.damage += f64::from(damage.amount);
                    target.hits += 1;

                    self.first_damage_at.get_or_insert(now);
                    self.last_damage_at = now;
                }
                // No target row: the target list is the damage-taken breakdown, and a healed
                // ally appearing in it with zero damage reads as a bug in the meter.
                CombatEvent::Healing {
                    amount, ability, ..
                } => {
                    self.total_healing += f64::from(amount);
                    if let Some(totals) = self.ability_mut(ability) {
                        totals.healing += f64::from(amount);
                    }
                }
                CombatEvent::Death { entity, .. } => {
                    self.deaths += 1;
                    let index = self.target_index(entity);
                    self.targets[index].died = true;
                }
                // Statuses and casts move no damage number. A cast that lands emits its own
                // `AbilityUsed` and `Damage` when it does, so counting the cast as well
                // would report every cast ability twice.
                CombatEvent::StatusApplied { .. }
                | CombatEvent::StatusRemoved { .. }
                | CombatEvent::CastStarted { .. }
                | CombatEvent::CastEnded { .. } => {}
            }
        }
    }

    /// Total damage dealt this session.
    #[must_use]
    pub fn total_damage(&self) -> f64 {
        self.total_damage
    }

    /// Total healing done this session.
    #[must_use]
    pub fn total_healing(&self) -> f64 {
        self.total_healing
    }

    /// Damage that exceeded what the targets had left.
    #[must_use]
    pub fn total_overkill(&self) -> f64 {
        self.total_overkill
    }

    /// Deaths observed this session.
    #[must_use]
    pub fn deaths(&self) -> u32 {
        self.deaths
    }

    /// Seconds of simulation time between the first and last damage.
    ///
    /// Zero before any damage. This is what DPS divides by, floored at
    /// [`MINIMUM_DPS_WINDOW`].
    #[must_use]
    pub fn combat_seconds(&self) -> f64 {
        match self.first_damage_at {
            Some(first) => (self.last_damage_at - first).max(0.0),
            None => 0.0,
        }
    }

    /// Damage per second of combat.
    ///
    /// Simulation time, never render frames, and never wall-clock: identical combat reports
    /// an identical number on any machine.
    #[must_use]
    pub fn dps(&self) -> f64 {
        if self.first_damage_at.is_none() {
            return 0.0;
        }
        self.total_damage / self.combat_seconds().max(MINIMUM_DPS_WINDOW)
    }

    /// Whether any damage has been recorded.
    #[must_use]
    pub fn has_damage(&self) -> bool {
        self.first_damage_at.is_some()
    }

    /// Seconds of simulation time since the session started.
    #[must_use]
    pub fn session_seconds(&self) -> f64 {
        (self.now - self.session_started_at).max(0.0)
    }

    /// Per-ability totals, indexed by slot.
    #[must_use]
    pub fn abilities(&self) -> &[AbilityTotals; ABILITY_SLOTS] {
        &self.abilities
    }

    /// One ability's totals.
    #[must_use]
    pub fn ability(&self, id: AbilityId) -> AbilityTotals {
        self.abilities.get(id.slot()).copied().unwrap_or_default()
    }

    /// Every target that has taken damage, in the order they were first hit.
    #[must_use]
    pub fn targets(&self) -> &[TargetTotals] {
        &self.targets
    }

    /// One target's totals, if it has been hit.
    #[must_use]
    pub fn target(&self, entity: EntityId) -> Option<TargetTotals> {
        let index = *self.by_slot.get(entity.index())? as usize;
        if index == 0 {
            return None;
        }
        self.targets
            .get(index - 1)
            .copied()
            .filter(|totals| totals.entity == entity)
    }

    /// The abilities that did damage, most first, written into `out`.
    ///
    /// Writes into a caller-owned buffer because the only caller is the overlay, which
    /// rebuilds a few times a second and keeps its buffers.
    pub fn ranked_abilities(&self, out: &mut Vec<(AbilityId, AbilityTotals)>) {
        out.clear();
        for (slot, totals) in self.abilities.iter().enumerate() {
            if totals.damage <= 0.0 && totals.casts == 0 {
                continue;
            }
            if let Some(id) = AbilityId::from_slot(slot) {
                out.push((id, *totals));
            }
        }
        out.sort_by(|a, b| b.1.damage.total_cmp(&a.1.damage));
    }

    /// The targets that took damage, most first, written into `out`.
    pub fn ranked_targets(&self, out: &mut Vec<TargetTotals>) {
        out.clear();
        out.extend(self.targets.iter().copied());
        out.sort_by(|a, b| b.damage.total_cmp(&a.damage));
    }

    /// Starts a new session at simulation time `now`, keeping every allocation.
    pub fn reset(&mut self, now: f64) {
        self.abilities = [AbilityTotals::default(); ABILITY_SLOTS];
        self.targets.clear();
        // Zeroed rather than cleared: the slot index is addressed by entity slot, so it
        // must stay as long as the world's slots do.
        self.by_slot.fill(0);
        self.total_damage = 0.0;
        self.total_healing = 0.0;
        self.total_overkill = 0.0;
        self.deaths = 0;
        self.first_damage_at = None;
        self.last_damage_at = now;
        self.session_started_at = now;
        self.now = now;
    }

    /// How many distinct targets the meter can hold before it grows.
    #[must_use]
    pub fn target_capacity(&self) -> usize {
        self.targets.capacity()
    }

    /// One ability's totals, mutably, if the id names a real slot.
    fn ability_mut(&mut self, id: AbilityId) -> Option<&mut AbilityTotals> {
        self.abilities.get_mut(id.slot())
    }

    /// The index of `entity` in the target list, adding it if it is new.
    fn target_index(&mut self, entity: EntityId) -> usize {
        let slot = entity.index();
        if self.by_slot.len() <= slot {
            self.by_slot.resize(slot + 1, 0);
        }

        let stored = self.by_slot[slot] as usize;
        if stored > 0
            && self
                .targets
                .get(stored - 1)
                .is_some_and(|totals| totals.entity == entity)
        {
            return stored - 1;
        }

        // Either unseen, or the slot has been reused by a new entity since — a reset dummy
        // keeps its id, but a despawned and respawned one does not, and its damage belongs
        // to a separate row rather than to its predecessor's.
        self.targets.push(TargetTotals {
            entity,
            damage: 0.0,
            hits: 0,
            died: false,
        });
        self.by_slot[slot] = self.targets.len() as u32;
        self.targets.len() - 1
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::combat::DamageType;
    use crate::event::DamageEvent;
    use crate::status::{StatusEnd, StatusKind};

    fn player() -> EntityId {
        EntityId::new(0, 1)
    }

    fn dummy(index: u32) -> EntityId {
        EntityId::new(index + 1, 1)
    }

    fn ability(slot: usize) -> AbilityId {
        AbilityId::from_slot(slot).expect("the slot exists")
    }

    fn hit(target: EntityId, slot: usize, amount: f32) -> CombatEvent {
        CombatEvent::Damage(DamageEvent {
            source: player(),
            target,
            amount,
            overkill: 0.0,
            ability: ability(slot),
            damage_type: DamageType::Fire,
            killed: false,
            tick: 0,
        })
    }

    #[test]
    fn a_fresh_meter_reports_nothing_rather_than_dividing_by_zero() {
        let meter = CombatMeter::new();
        assert_eq!(meter.total_damage(), 0.0);
        assert_eq!(meter.dps(), 0.0);
        assert_eq!(meter.combat_seconds(), 0.0);
        assert!(!meter.has_damage());
        assert!(meter.targets().is_empty());
    }

    #[test]
    fn damage_accumulates_in_total_by_ability_and_by_target() {
        let mut meter = CombatMeter::new();
        meter.observe(&[hit(dummy(0), 0, 45.0)], 1.0);
        meter.observe(&[hit(dummy(1), 0, 45.0), hit(dummy(0), 1, 10.0)], 2.0);

        assert_eq!(meter.total_damage(), 100.0);
        assert_eq!(meter.ability(ability(0)).damage, 90.0);
        assert_eq!(meter.ability(ability(0)).hits, 2);
        assert_eq!(meter.ability(ability(1)).damage, 10.0);
        assert_eq!(
            meter.target(dummy(0)).map(|totals| totals.damage),
            Some(55.0)
        );
        assert_eq!(
            meter.target(dummy(1)).map(|totals| totals.damage),
            Some(45.0)
        );
        assert_eq!(meter.targets().len(), 2);
    }

    #[test]
    fn dps_uses_simulation_time_between_the_first_and_last_hit() {
        let mut meter = CombatMeter::new();
        meter.observe(&[hit(dummy(0), 0, 100.0)], 10.0);
        meter.observe(&[hit(dummy(0), 0, 100.0)], 14.0);

        assert_eq!(meter.combat_seconds(), 4.0);
        assert_eq!(meter.dps(), 50.0, "200 damage over four seconds");
    }

    #[test]
    fn a_single_instant_of_damage_does_not_report_an_absurd_rate() {
        let mut meter = CombatMeter::new();
        meter.observe(&[hit(dummy(0), 0, 45.0)], 0.05);
        assert_eq!(meter.dps(), 45.0, "floored at a one-second window");
        assert!(meter.dps().is_finite());
    }

    #[test]
    fn casts_hits_and_overkill_are_counted_separately() {
        // One area cast that hits four dummies is one cast and four hits.
        let mut meter = CombatMeter::new();
        let mut events = vec![CombatEvent::AbilityUsed {
            source: player(),
            ability: ability(2),
            targets: 4,
            tick: 1,
        }];
        for index in 0..4 {
            events.push(hit(dummy(index), 2, 20.0));
        }
        events.push(CombatEvent::Damage(DamageEvent {
            source: player(),
            target: dummy(0),
            amount: 5.0,
            overkill: 95.0,
            ability: ability(2),
            damage_type: DamageType::Fire,
            killed: true,
            tick: 1,
        }));
        events.push(CombatEvent::Death {
            entity: dummy(0),
            killer: player(),
            tick: 1,
        });
        meter.observe(&events, 1.0);

        let totals = meter.ability(ability(2));
        assert_eq!(totals.casts, 1);
        assert_eq!(totals.hits, 5);
        assert_eq!(totals.damage, 85.0);
        assert_eq!(meter.total_overkill(), 95.0);
        assert_eq!(meter.deaths(), 1);
        assert_eq!(meter.target(dummy(0)).map(|t| t.died), Some(true));
    }

    #[test]
    fn healing_is_tracked_apart_from_damage() {
        let mut meter = CombatMeter::new();
        meter.observe(
            &[CombatEvent::Healing {
                source: player(),
                target: player(),
                amount: 30.0,
                ability: ability(3),
                tick: 0,
            }],
            1.0,
        );
        assert_eq!(meter.total_healing(), 30.0);
        assert_eq!(meter.total_damage(), 0.0);
        assert!(!meter.has_damage(), "healing is not damage");
        assert_eq!(meter.ability(ability(3)).healing, 30.0);
    }

    #[test]
    fn status_events_do_not_move_any_number() {
        let mut meter = CombatMeter::new();
        meter.observe(
            &[
                CombatEvent::StatusApplied {
                    source: player(),
                    target: dummy(0),
                    status: StatusKind::Burning,
                    duration: 6.0,
                    tick: 0,
                },
                CombatEvent::StatusRemoved {
                    target: dummy(0),
                    status: StatusKind::Burning,
                    reason: StatusEnd::Consumed,
                    tick: 0,
                },
            ],
            1.0,
        );
        assert_eq!(meter.total_damage(), 0.0);
        assert!(meter.targets().is_empty(), "a status is not a hit");
    }

    #[test]
    fn ranking_puts_the_biggest_contributor_first() {
        let mut meter = CombatMeter::new();
        meter.observe(
            &[
                hit(dummy(0), 0, 10.0),
                hit(dummy(1), 1, 90.0),
                hit(dummy(2), 2, 50.0),
            ],
            1.0,
        );

        let mut abilities = Vec::new();
        meter.ranked_abilities(&mut abilities);
        assert_eq!(
            abilities
                .iter()
                .map(|(id, _)| id.slot())
                .collect::<Vec<_>>(),
            vec![1, 2, 0]
        );

        let mut targets = Vec::new();
        meter.ranked_targets(&mut targets);
        assert_eq!(
            targets.iter().map(|t| t.damage).collect::<Vec<_>>(),
            vec![90.0, 50.0, 10.0]
        );
    }

    #[test]
    fn resetting_clears_the_session_and_keeps_the_allocations() {
        let mut meter = CombatMeter::new();
        for index in 0..16 {
            meter.observe(&[hit(dummy(index), 0, 10.0)], f64::from(index));
        }
        let capacity = meter.target_capacity();

        meter.reset(100.0);
        assert_eq!(meter.total_damage(), 0.0);
        assert_eq!(meter.dps(), 0.0);
        assert!(!meter.has_damage());
        assert!(meter.targets().is_empty());
        assert_eq!(meter.ability(ability(0)), AbilityTotals::default());
        assert_eq!(meter.session_seconds(), 0.0);
        assert_eq!(meter.target_capacity(), capacity, "the meter reallocated");

        // And it keeps working afterwards.
        meter.observe(&[hit(dummy(0), 0, 20.0)], 101.0);
        assert_eq!(meter.total_damage(), 20.0);
    }

    #[test]
    fn a_reused_slot_gets_its_own_row_rather_than_inheriting_one() {
        // A despawned dummy's replacement lives in the same slot with a new generation. Its
        // damage must not be added to the dead one's total.
        let mut meter = CombatMeter::new();
        let first = EntityId::new(4, 1);
        let second = EntityId::new(4, 2);
        meter.observe(&[hit(first, 0, 30.0)], 1.0);
        meter.observe(&[hit(second, 0, 70.0)], 2.0);

        assert_eq!(meter.targets().len(), 2);
        assert_eq!(meter.target(second).map(|t| t.damage), Some(70.0));
        assert_eq!(meter.total_damage(), 100.0);
    }

    #[test]
    fn a_steady_stream_of_events_does_not_reallocate() {
        let mut meter = CombatMeter::new();
        for index in 0..8 {
            meter.observe(&[hit(dummy(index), 0, 1.0)], 1.0);
        }
        let capacity = meter.target_capacity();
        for tick in 0..10_000 {
            meter.observe(&[hit(dummy(tick % 8), 0, 1.0)], f64::from(tick));
        }
        assert_eq!(meter.target_capacity(), capacity);
    }
}
