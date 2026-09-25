//! Who an enemy is angriest at.
//!
//! ```text
//! ThreatTable
//! ├── EntityId(3)  180.0   ◀── current target
//! ├── EntityId(7)   95.0
//! └── EntityId(9)    4.0
//! ```
//!
//! # Why a table and not a field
//!
//! The game has one player today, and "the enemy attacks the player" would work today. It
//! would also have to be torn out the first time there are two of anything — a second
//! player, a pet, a summon, a taunting tank. A table with one row costs a `Vec` push and
//! makes every one of those a *number*, not a rewrite.
//!
//! # The rules
//!
//! * Damage to the enemy adds threat equal to the damage dealt times the ability's
//!   [`ThreatScale::damage`](crate::firemage::ThreatScale): one for a damage dealer, two
//!   for the tank, more for the tank's threat tools.
//! * Healing adds threat for the healer on every enemy already fighting whoever was healed,
//!   at [`ThreatScale::healing`](crate::firemage::ThreatScale) per point actually restored.
//! * Threat accumulates and never decays on its own.
//! * The target is the highest-threat row that is still *valid*: alive, hostile and known
//!   to the world. Validity is decided by the caller, which is what keeps this file free of
//!   any dependency on the world.
//! * A tie is broken by which row was added first, so a fight does not flip target every
//!   tick between two identical threats.
//! * A **taunt** overrides all of that for its duration: the enemy attacks the taunter
//!   while the taunt runs and the taunter is valid. The taunter's threat is also lifted to
//!   the top of the table, so the enemy does not snap straight back when the taunt ends —
//!   keeping it after that is the taunter's job.
//!
//! # Warnings
//!
//! [`ThreatTable::watch`] compares the enemy's current target with its closest challenger.
//! When the challenger reaches [`AGGRO_WARNING_RATIO`] of the holder's threat it reports an
//! [`AggroWarning`] once, and stays quiet until the challenger falls back under
//! [`AGGRO_REARM_RATIO`] — a warning every tick would be a warning nobody reads.
//!
//! # Bounded
//!
//! [`MAX_THREAT_ROWS`] rows. Past that the smallest is replaced, because an enemy that
//! remembers a hundred attackers is remembering things it will never act on.

use crate::entity::EntityId;

/// How many attackers one enemy remembers.
pub const MAX_THREAT_ROWS: usize = 8;

/// The share of the holder's threat at which a challenger is warned it is about to pull.
pub const AGGRO_WARNING_RATIO: f32 = 0.9;

/// The share a challenger must fall back under before it can be warned again.
///
/// Below the warning ratio on purpose: a challenger hovering at ninety per cent would
/// otherwise be warned every time it crossed the line.
pub const AGGRO_REARM_RATIO: f32 = 0.75;

/// A forced target, and how long it is forced for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Taunt {
    /// Who the enemy must attack.
    pub entity: EntityId,
    /// Seconds left.
    pub remaining: f32,
}

/// Somebody is close to taking an enemy off the one it is attacking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggroWarning {
    /// Who the enemy is attacking now, and is about to lose.
    pub holder: EntityId,
    /// Who is about to take it.
    pub challenger: EntityId,
}

/// One attacker and how much it has annoyed this enemy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThreatRow {
    /// Who.
    pub entity: EntityId,
    /// How much.
    pub threat: f32,
}

/// Everything one enemy is angry at.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ThreatTable {
    rows: Vec<ThreatRow>,
    /// The forced target, while one is forced.
    taunt: Option<Taunt>,
    /// The pairing last warned about, so the warning is not repeated every tick.
    warned: Option<AggroWarning>,
}

impl ThreatTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Vec::with_capacity(2),
            taunt: None,
            warned: None,
        }
    }

    /// Adds `amount` of threat for `entity`, creating its row if it has none.
    ///
    /// A non-positive or non-finite amount still *creates* the row: being attacked for zero
    /// is still being attacked, and an enemy that ignored a miss would ignore whoever missed.
    pub fn add(&mut self, entity: EntityId, amount: f32) {
        if !entity.is_valid() {
            return;
        }
        let amount = if amount.is_finite() {
            amount.max(0.0)
        } else {
            0.0
        };
        if let Some(row) = self.rows.iter_mut().find(|row| row.entity == entity) {
            row.threat += amount;
            return;
        }
        if self.rows.len() < MAX_THREAT_ROWS {
            self.rows.push(ThreatRow {
                entity,
                threat: amount,
            });
            return;
        }
        // Full: the least threatening row is the one that gets forgotten, and only if the
        // newcomer is more threatening than it.
        let Some(weakest) = self
            .rows
            .iter_mut()
            .min_by(|left, right| left.threat.total_cmp(&right.threat))
        else {
            return;
        };
        if amount > weakest.threat {
            *weakest = ThreatRow {
                entity,
                threat: amount,
            };
        }
    }

    /// How much threat `entity` has. Zero if it has no row.
    #[must_use]
    pub fn threat(&self, entity: EntityId) -> f32 {
        self.rows
            .iter()
            .find(|row| row.entity == entity)
            .map_or(0.0, |row| row.threat)
    }

    /// Whether `entity` has a row: whether this enemy is fighting them at all.
    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.rows.iter().any(|row| row.entity == entity)
    }

    /// The highest-threat entity `valid` accepts.
    ///
    /// `valid` is the caller's rule — alive, hostile, in the world — so this file needs no
    /// access to the world and can be tested with a closure.
    #[must_use]
    pub fn highest(&self, mut valid: impl FnMut(EntityId) -> bool) -> Option<EntityId> {
        let mut best: Option<ThreatRow> = None;
        for row in &self.rows {
            if !valid(row.entity) {
                continue;
            }
            // Strictly greater, so the earliest row wins a tie and the target does not flip
            // between two equal attackers every tick.
            if best.is_none_or(|current| row.threat > current.threat) {
                best = Some(*row);
            }
        }
        best.map(|row| row.entity)
    }

    /// Who the enemy attacks: the taunter while a taunt runs and they are valid, the
    /// highest valid threat otherwise.
    #[must_use]
    pub fn target(&self, mut valid: impl FnMut(EntityId) -> bool) -> Option<EntityId> {
        if let Some(taunt) = self.taunt
            && valid(taunt.entity)
        {
            return Some(taunt.entity);
        }
        self.highest(valid)
    }

    /// Forces the enemy onto `entity` for `seconds`.
    ///
    /// Also lifts the taunter's threat to the highest in the table, so when the taunt ends
    /// the enemy stays where it was put until somebody out-threatens the taunter. A taunter
    /// with no row gets one even in a full table: a taunt that could be refused for lack of
    /// room would be a button that sometimes does nothing.
    pub fn taunt(&mut self, entity: EntityId, seconds: f32) {
        if !entity.is_valid() || !seconds.is_finite() || seconds <= 0.0 {
            return;
        }
        let top = self
            .rows
            .iter()
            .map(|row| row.threat)
            .fold(0.0_f32, f32::max);
        if let Some(row) = self.rows.iter_mut().find(|row| row.entity == entity) {
            row.threat = row.threat.max(top);
        } else {
            let row = ThreatRow {
                entity,
                threat: top,
            };
            if self.rows.len() < MAX_THREAT_ROWS {
                self.rows.push(row);
            } else if let Some(weakest) = self
                .rows
                .iter_mut()
                .min_by(|left, right| left.threat.total_cmp(&right.threat))
            {
                *weakest = row;
            }
        }
        // A fresh taunt replaces a running one, whoever cast it: the last tank to taunt is
        // the one who asked most recently.
        self.taunt = Some(Taunt {
            entity,
            remaining: seconds,
        });
    }

    /// The running taunt, if there is one.
    #[must_use]
    pub fn taunted(&self) -> Option<Taunt> {
        self.taunt
    }

    /// Runs the taunt's clock down.
    pub fn tick(&mut self, step: f32) {
        if let Some(taunt) = self.taunt.as_mut() {
            taunt.remaining -= step;
            if taunt.remaining <= crate::TIME_EPSILON {
                self.taunt = None;
            }
        }
    }

    /// Whether somebody is close to taking the enemy off `holder`.
    ///
    /// Returns a warning the first time the closest valid challenger reaches
    /// [`AGGRO_WARNING_RATIO`] of the holder's threat, and nothing again until that
    /// challenger falls under [`AGGRO_REARM_RATIO`] or the pairing changes. `None` for the
    /// holder forgets the last warning: an enemy attacking nobody has nobody to lose.
    pub fn watch(
        &mut self,
        holder: Option<EntityId>,
        mut valid: impl FnMut(EntityId) -> bool,
    ) -> Option<AggroWarning> {
        let Some(holder) = holder else {
            self.warned = None;
            return None;
        };
        let Some(challenger) = self.highest(|entity| entity != holder && valid(entity)) else {
            self.warned = None;
            return None;
        };
        let held = self.threat(holder);
        let challenging = self.threat(challenger);
        let ratio = if held > 0.0 {
            challenging / held
        } else if challenging > 0.0 {
            f32::INFINITY
        } else {
            0.0
        };
        let warning = AggroWarning { holder, challenger };
        if self.warned != Some(warning) {
            self.warned = None;
        }
        if ratio < AGGRO_REARM_RATIO {
            self.warned = None;
            return None;
        }
        if ratio >= AGGRO_WARNING_RATIO && self.warned.is_none() {
            self.warned = Some(warning);
            return Some(warning);
        }
        None
    }

    /// Forgets one attacker.
    pub fn remove(&mut self, entity: EntityId) {
        self.rows.retain(|row| row.entity != entity);
        if self.taunt.is_some_and(|taunt| taunt.entity == entity) {
            self.taunt = None;
        }
    }

    /// Forgets everything, keeping the allocation.
    ///
    /// What a leash reset does: an enemy that went home is not still angry, and no longer
    /// taunted either.
    pub fn clear(&mut self) {
        self.rows.clear();
        self.taunt = None;
        self.warned = None;
    }

    /// Every row, in insertion order.
    #[must_use]
    pub fn rows(&self) -> &[ThreatRow] {
        &self.rows
    }

    /// How many attackers are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether nobody has done anything to this enemy.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn id(index: u32) -> EntityId {
        EntityId::new(index, 1)
    }

    #[test]
    fn damage_generates_threat_and_it_accumulates() {
        let mut table = ThreatTable::new();
        table.add(id(1), 50.0);
        table.add(id(1), 30.0);
        assert_eq!(table.threat(id(1)), 80.0);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn the_highest_valid_threat_is_the_target() {
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        table.add(id(2), 250.0);
        table.add(id(3), 10.0);
        assert_eq!(table.highest(|_| true), Some(id(2)));
    }

    #[test]
    fn an_invalid_target_is_skipped_rather_than_chased() {
        // What a dead attacker looks like: still in the table, no longer a legal target.
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        table.add(id(2), 250.0);
        assert_eq!(table.highest(|entity| entity != id(2)), Some(id(1)));
        assert_eq!(table.highest(|_| false), None);
    }

    #[test]
    fn a_tie_keeps_the_target_it_already_had() {
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        table.add(id(2), 100.0);
        assert_eq!(table.highest(|_| true), Some(id(1)));
    }

    #[test]
    fn an_empty_table_names_nobody() {
        let table = ThreatTable::new();
        assert!(table.is_empty());
        assert_eq!(table.highest(|_| true), None);
        assert_eq!(table.threat(id(1)), 0.0);
    }

    #[test]
    fn nonsense_threat_does_not_poison_the_table() {
        let mut table = ThreatTable::new();
        table.add(id(1), f32::NAN);
        table.add(id(1), f32::INFINITY);
        table.add(id(1), -50.0);
        assert_eq!(table.threat(id(1)), 0.0);
        assert_eq!(
            table.highest(|_| true),
            Some(id(1)),
            "a miss is still an attack"
        );
        table.add(EntityId::INVALID, 100.0);
        assert_eq!(table.len(), 1, "nobody is not an attacker");
    }

    #[test]
    fn the_table_is_bounded_and_forgets_the_least_threatening() {
        let mut table = ThreatTable::new();
        for index in 0..MAX_THREAT_ROWS as u32 {
            table.add(id(index), 10.0 + index as f32);
        }
        assert_eq!(table.len(), MAX_THREAT_ROWS);

        // Weaker than everything: ignored.
        table.add(id(99), 1.0);
        assert_eq!(table.threat(id(99)), 0.0);
        assert_eq!(table.len(), MAX_THREAT_ROWS);

        // Stronger than the weakest: it takes that row.
        table.add(id(98), 500.0);
        assert_eq!(table.threat(id(98)), 500.0);
        assert_eq!(
            table.threat(id(0)),
            0.0,
            "the smallest row was the one lost"
        );
        assert_eq!(table.highest(|_| true), Some(id(98)));
    }

    #[test]
    fn a_taunt_forces_the_target_until_it_runs_out() {
        let mut table = ThreatTable::new();
        table.add(id(1), 500.0);
        table.add(id(2), 10.0);
        assert_eq!(table.target(|_| true), Some(id(1)));

        table.taunt(id(2), 3.0);
        assert_eq!(table.target(|_| true), Some(id(2)), "the taunt forces it");
        table.add(id(1), 5_000.0);
        assert_eq!(
            table.target(|_| true),
            Some(id(2)),
            "no amount of threat breaks a running taunt"
        );

        table.tick(2.0);
        assert!(table.taunted().is_some());
        table.tick(1.0);
        assert!(table.taunted().is_none());
        assert_eq!(
            table.target(|_| true),
            Some(id(1)),
            "then threat decides again"
        );
    }

    #[test]
    fn a_taunt_lifts_the_taunter_to_the_top_threat() {
        let mut table = ThreatTable::new();
        table.add(id(1), 400.0);
        table.add(id(2), 50.0);
        table.taunt(id(2), 3.0);
        assert_eq!(table.threat(id(2)), 400.0);
        assert_eq!(table.threat(id(1)), 400.0, "nobody else's threat moves");

        // Once it ends, the taunter keeps the enemy by staying ahead.
        table.tick(3.0);
        table.add(id(2), 1.0);
        assert_eq!(table.target(|_| true), Some(id(2)));
    }

    #[test]
    fn a_taunt_from_somebody_new_takes_a_row_even_in_a_full_table() {
        let mut table = ThreatTable::new();
        for index in 0..MAX_THREAT_ROWS as u32 {
            table.add(id(index), 100.0 + index as f32);
        }
        table.taunt(id(50), 2.0);
        assert_eq!(table.len(), MAX_THREAT_ROWS);
        assert_eq!(table.target(|_| true), Some(id(50)));
    }

    #[test]
    fn an_invalid_taunter_is_not_chased() {
        // A taunter who died mid-taunt: the enemy goes back to its threat table.
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        table.taunt(id(2), 5.0);
        assert_eq!(table.target(|entity| entity != id(2)), Some(id(1)));
    }

    #[test]
    fn nonsense_taunts_are_ignored() {
        let mut table = ThreatTable::new();
        table.taunt(EntityId::INVALID, 3.0);
        table.taunt(id(1), f32::NAN);
        table.taunt(id(1), 0.0);
        assert!(table.taunted().is_none());
        assert!(table.is_empty());
    }

    #[test]
    fn a_challenger_close_to_the_holder_is_warned_once() {
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        table.add(id(2), 50.0);
        assert_eq!(
            table.watch(Some(id(1)), |_| true),
            None,
            "half is not close"
        );

        table.add(id(2), 40.0);
        assert_eq!(
            table.watch(Some(id(1)), |_| true),
            Some(AggroWarning {
                holder: id(1),
                challenger: id(2),
            })
        );
        assert_eq!(table.watch(Some(id(1)), |_| true), None, "and only once");
    }

    #[test]
    fn a_warning_rearms_once_the_challenger_falls_back() {
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        table.add(id(2), 95.0);
        assert!(table.watch(Some(id(1)), |_| true).is_some());

        // Hovering between the two ratios stays quiet.
        table.add(id(1), 10.0);
        assert!(table.watch(Some(id(1)), |_| true).is_none());

        // The holder pulls well ahead: the warning re-arms...
        table.add(id(1), 100.0);
        assert!(table.watch(Some(id(1)), |_| true).is_none());
        // ...and the challenger catching up again is warned again.
        table.add(id(2), 100.0);
        assert!(table.watch(Some(id(1)), |_| true).is_some());
    }

    #[test]
    fn nobody_to_hold_or_nobody_to_challenge_is_no_warning() {
        let mut table = ThreatTable::new();
        table.add(id(1), 100.0);
        assert_eq!(table.watch(Some(id(1)), |_| true), None);
        assert_eq!(table.watch(None, |_| true), None);
        table.add(id(2), 99.0);
        assert_eq!(
            table.watch(Some(id(1)), |entity| entity != id(2)),
            None,
            "an invalid challenger is no threat"
        );
    }

    #[test]
    fn a_reset_forgets_everyone() {
        let mut table = ThreatTable::new();
        table.add(id(1), 10.0);
        table.remove(id(1));
        assert!(table.is_empty());
        table.add(id(2), 10.0);
        table.taunt(id(2), 5.0);
        table.clear();
        assert!(table.is_empty());
        assert!(table.taunted().is_none(), "a reset ends the taunt too");
    }
}
