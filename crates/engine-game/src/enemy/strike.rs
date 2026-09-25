//! Zones that land *after* the cast that laid them: rains, barrages, quakes, marks and
//! shockwaves.
//!
//! ```text
//! cast completes ──▶ PendingStrike per zone ──▶ telegraph drawn for `delay` seconds
//!                          │ follows its victim, if it is a mark
//!                          ▼
//!                 remaining reaches zero ──▶ landed ──▶ Simulation resolves the shape
//!                                                         │ a chain mark lays more
//! ```
//!
//! # Why this is not the cast
//!
//! A cast holds its caster still until it lands; a strike does not. The Bonecaster that has
//! marked somebody goes back to bolting while the mark counts down, and the Brute that laid a
//! quake is already swinging at the tank when it lands. That is what makes a delayed zone a
//! *second* thing on the screen rather than a longer cast bar — and it needs its own clock,
//! which is this.
//!
//! # It deals no damage
//!
//! Like the hazard field it hands back what landed, and the simulation resolves it through
//! the same shape test and the same `deal_damage` every other enemy attack uses.

use engine_core::math::Vec3;

use crate::entity::EntityId;

use super::{EnemyAbility, TelegraphShape};

/// The most strikes that can be pending at once.
///
/// Two empowered adds' worst turn and a boss mark, with room to spare. Bounded, like the
/// hazard field, so a runaway caster costs a fixed amount — and the oldest strike is the one
/// dropped, because the newest is the one the player can still react to.
pub const MAX_PENDING_STRIKES: usize = 48;

/// One zone waiting to land.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingStrike {
    /// The telegraph id its warning was announced with, so its end can name it.
    pub telegraph: u32,
    /// Who laid it. Damage is credited to them.
    pub owner: EntityId,
    /// Which ability it belongs to.
    pub ability: EnemyAbility,
    /// Where it is anchored now.
    pub anchor: Vec3,
    /// The entity it follows, for a mark.
    pub follow: Option<EntityId>,
    /// The ground it will cover.
    pub shape: TelegraphShape,
    /// Seconds until it lands.
    pub remaining: f32,
    /// What its damage is multiplied by: a shockwave's share, a chained mark's.
    pub damage_scale: f32,
    /// Whether it was laid by another strike's chain, and so may not chain again.
    pub chained: bool,
}

/// Every strike waiting to land.
#[derive(Debug, Clone, PartialEq)]
pub struct StrikeField {
    strikes: Vec<PendingStrike>,
}

impl Default for StrikeField {
    fn default() -> Self {
        Self::new()
    }
}

impl StrikeField {
    /// An empty field.
    #[must_use]
    pub fn new() -> Self {
        Self {
            strikes: Vec::with_capacity(MAX_PENDING_STRIKES),
        }
    }

    /// Every pending strike, in the order they were laid.
    #[must_use]
    pub fn strikes(&self) -> &[PendingStrike] {
        &self.strikes
    }

    /// How many are pending.
    #[must_use]
    pub fn len(&self) -> usize {
        self.strikes.len()
    }

    /// Whether nothing is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strikes.is_empty()
    }

    /// Lays a strike. `dropped` receives one that had to make room, so its warning can be
    /// taken down.
    pub fn push(&mut self, strike: PendingStrike, dropped: &mut Vec<PendingStrike>) {
        if self.strikes.len() >= MAX_PENDING_STRIKES {
            dropped.push(self.strikes.remove(0));
        }
        self.strikes.push(strike);
    }

    /// Moves every mark to where its victim is standing now.
    ///
    /// `position_of` answers `None` for a victim that is gone, and the mark then stays where
    /// it last was — it still lands, on the spot the victim died on.
    pub fn follow(&mut self, mut position_of: impl FnMut(EntityId) -> Option<Vec3>) {
        for strike in &mut self.strikes {
            if let Some(victim) = strike.follow
                && let Some(position) = position_of(victim)
            {
                strike.anchor = position;
            }
        }
    }

    /// Counts every strike down, moving the ones that land into `landed`.
    ///
    /// In the order they were laid, so two strikes landing on one tick resolve in a fixed
    /// order and a replay is a replay.
    pub fn tick(&mut self, step: f32, landed: &mut Vec<PendingStrike>) {
        let step = if step.is_finite() { step.max(0.0) } else { 0.0 };
        for strike in &mut self.strikes {
            strike.remaining -= step;
        }
        self.strikes.retain(|strike| {
            if strike.remaining > crate::TIME_EPSILON {
                return true;
            }
            landed.push(*strike);
            false
        });
    }

    /// Takes away everything `owner` laid, moving it into `removed`.
    ///
    /// What a death runs: a caster killed before its mark lands has saved the player it
    /// marked, which is a reward for killing the right thing first.
    pub fn clear_owner(&mut self, owner: EntityId, removed: &mut Vec<PendingStrike>) {
        self.strikes.retain(|strike| {
            if strike.owner != owner {
                return true;
            }
            removed.push(*strike);
            false
        });
    }

    /// Takes everything away. What a reset runs.
    pub fn clear_into(&mut self, removed: &mut Vec<PendingStrike>) {
        removed.append(&mut self.strikes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strike(remaining: f32) -> PendingStrike {
        PendingStrike {
            telegraph: 1,
            owner: EntityId::new(2, 1),
            ability: EnemyAbility::BoneQuake,
            anchor: Vec3::ZERO,
            follow: None,
            shape: TelegraphShape::Circle { radius: 3.0 },
            remaining,
            damage_scale: 1.0,
            chained: false,
        }
    }

    #[test]
    fn a_strike_lands_when_its_clock_runs_out_and_not_before() {
        let mut field = StrikeField::new();
        let mut landed = Vec::new();
        field.push(strike(1.0), &mut Vec::new());
        for _ in 0..59 {
            field.tick(1.0 / 60.0, &mut landed);
        }
        assert!(landed.is_empty(), "it landed early");
        field.tick(1.0 / 60.0, &mut landed);
        assert_eq!(landed.len(), 1);
        assert!(field.is_empty());
    }

    #[test]
    fn a_mark_follows_its_victim_until_it_lands() {
        let mut field = StrikeField::new();
        let victim = EntityId::new(0, 1);
        field.push(
            PendingStrike {
                follow: Some(victim),
                ..strike(2.0)
            },
            &mut Vec::new(),
        );
        field.follow(|entity| (entity == victim).then_some(Vec3::new(5.0, 0.0, -3.0)));
        assert_eq!(field.strikes()[0].anchor, Vec3::new(5.0, 0.0, -3.0));
        // A victim that is gone leaves the mark where it was.
        field.follow(|_| None);
        assert_eq!(field.strikes()[0].anchor, Vec3::new(5.0, 0.0, -3.0));
    }

    #[test]
    fn killing_the_caster_takes_its_strikes_away() {
        let mut field = StrikeField::new();
        let other = EntityId::new(9, 1);
        field.push(strike(1.0), &mut Vec::new());
        field.push(
            PendingStrike {
                owner: other,
                ..strike(1.0)
            },
            &mut Vec::new(),
        );
        let mut removed = Vec::new();
        field.clear_owner(EntityId::new(2, 1), &mut removed);
        assert_eq!(removed.len(), 1);
        assert_eq!(field.len(), 1);
        assert_eq!(field.strikes()[0].owner, other);
    }

    #[test]
    fn a_full_field_drops_the_oldest() {
        let mut field = StrikeField::new();
        let mut dropped = Vec::new();
        for index in 0..=MAX_PENDING_STRIKES {
            field.push(
                PendingStrike {
                    telegraph: index as u32,
                    ..strike(5.0)
                },
                &mut dropped,
            );
        }
        assert_eq!(field.len(), MAX_PENDING_STRIKES);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].telegraph, 0);
    }
}
