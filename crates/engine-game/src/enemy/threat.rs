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
//! * Damage to the enemy adds threat equal to the damage dealt.
//! * Threat accumulates and never decays on its own. (Decay, forgiveness on a leash reset
//!   and per-ability multipliers are all deliberate omissions — see `phase_enemy_ai.md`.)
//! * The target is the highest-threat row that is still *valid*: alive, hostile and known
//!   to the world. Validity is decided by the caller, which is what keeps this file free of
//!   any dependency on the world.
//! * A tie is broken by which row was added first, so a fight does not flip target every
//!   tick between two identical threats.
//!
//! # Bounded
//!
//! [`MAX_THREAT_ROWS`] rows. Past that the smallest is replaced, because an enemy that
//! remembers a hundred attackers is remembering things it will never act on.

use crate::entity::EntityId;

/// How many attackers one enemy remembers.
pub const MAX_THREAT_ROWS: usize = 8;

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
}

impl ThreatTable {
    /// An empty table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Vec::with_capacity(2),
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

    /// Forgets one attacker.
    pub fn remove(&mut self, entity: EntityId) {
        self.rows.retain(|row| row.entity != entity);
    }

    /// Forgets everything, keeping the allocation.
    ///
    /// What a leash reset does: an enemy that went home is not still angry.
    pub fn clear(&mut self) {
        self.rows.clear();
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
    fn a_reset_forgets_everyone() {
        let mut table = ThreatTable::new();
        table.add(id(1), 10.0);
        table.remove(id(1));
        assert!(table.is_empty());
        table.add(id(2), 10.0);
        table.clear();
        assert!(table.is_empty());
    }
}
