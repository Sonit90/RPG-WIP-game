//! Ground an enemy left burning.
//!
//! ```text
//! ability with EnemyEffect::Hazard
//!        │  resolves
//!        ▼
//!  EnemyHazard  ──ticks every HAZARD_TICK_INTERVAL──▶ HazardTick ──▶ Simulation::deal_damage
//!        │
//!        └── expires, is cleared by a reset, or dies with its owner
//! ```
//!
//! # Why this is not the FireMage's zone field
//!
//! [`crate::firemage::zone`] is the player's: its kinds are named after the player's
//! abilities, it carries spell-power coefficients, and it applies fire vulnerability. An
//! enemy hazard carries a flat damage per second and nothing else. Sharing the type would
//! mean the enemy table had to name a `ZoneKind`, which is a FireMage concept — so the two
//! are separate runtimes that happen to announce themselves with the same events.
//!
//! # It deals no damage
//!
//! [`EnemyHazardField::tick`] produces [`HazardTick`] values. The simulation turns them into
//! damage through the same `deal_damage` every ability goes through, which is what keeps
//! "every damage instance is observable" true — a hazard tick is attributed to the ability
//! that made the pool and to the enemy that cast it.

use engine_core::math::Vec3;

use crate::entity::EntityId;

use super::EnemyAbility;

/// Seconds between a hazard's damage ticks.
///
/// Half a second rather than a second: at 14–16 damage a second a full-second tick makes
/// clipping the edge of a pool free half the time, which reads as the hazard being broken.
pub const HAZARD_TICK_INTERVAL: f32 = 0.5;

/// The most hazards that can burn at once.
///
/// Two empowered adds' trails and a spreading pool, with room left over. A bounded array
/// rather than an unbounded list: an enemy that somehow cast forever must cost a fixed amount
/// of simulation, and the oldest hazard is dropped
/// rather than the newest refused — the newest is the one the player is being asked to move
/// out of.
pub const MAX_ENEMY_HAZARDS: usize = 32;

/// Seconds between the steps a spreading hazard grows by.
///
/// In steps rather than continuously, because every step is announced — the pool on the floor
/// is redrawn at its new size — and a redraw every tick would be a visual that never settles.
pub const HAZARD_GROWTH_INTERVAL: f32 = 1.5;

/// Metres a spreading hazard's radius grows by at each step.
pub const HAZARD_GROWTH_STEP: f32 = 1.0;

/// One patch of burning ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyHazard {
    /// Names it, so its end can be matched to its start.
    pub id: u32,
    /// Who created it. Damage is credited to them.
    pub owner: EntityId,
    /// Which ability is credited.
    pub ability: EnemyAbility,
    /// Where it is centred.
    pub center: Vec3,
    /// Metres from the centre. The same radius the telegraph drew.
    pub radius: f32,
    /// Seconds left.
    pub remaining: f32,
    /// Seconds it started with, kept so a visual can draw a fraction.
    pub duration: f32,
    /// Damage a second to anything standing in it.
    pub damage_per_second: f32,
    /// The radius it spreads to. Zero, or anything not above its radius, never grows.
    pub grow_to: f32,
    /// Seconds until the next damage tick.
    timer: f32,
    /// Seconds until it next grows.
    grow_timer: f32,
}

impl EnemyHazard {
    /// How much of its life is left, in `[0, 1]`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.remaining / self.duration).clamp(0.0, 1.0)
    }
}

/// One hazard's damage, for one tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HazardTick {
    /// Which hazard.
    pub id: u32,
    /// Who created it.
    pub owner: EntityId,
    /// Which ability is credited.
    pub ability: EnemyAbility,
    /// Where it is centred.
    pub center: Vec3,
    /// Metres from the centre.
    pub radius: f32,
    /// Damage this tick deals to everything inside.
    pub damage: f32,
}

/// Where enemy hazard ids start.
///
/// Area-effect events name their area with a `u32`, and the FireMage's own zones already use
/// that space from zero upwards. An observer that keyed a visual by id would otherwise be
/// handed one id for two different fires — so the enemy's live in the top half and the two
/// cannot meet.
pub const HAZARD_ID_BASE: u32 = 1 << 31;

/// Every hazard burning right now.
#[derive(Debug, Clone, PartialEq)]
pub struct EnemyHazardField {
    hazards: Vec<EnemyHazard>,
    next_id: u32,
}

impl Default for EnemyHazardField {
    fn default() -> Self {
        Self::new()
    }
}

impl EnemyHazardField {
    /// An empty field.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hazards: Vec::with_capacity(MAX_ENEMY_HAZARDS),
            next_id: HAZARD_ID_BASE,
        }
    }

    /// Every hazard, in creation order.
    #[must_use]
    pub fn hazards(&self) -> &[EnemyHazard] {
        &self.hazards
    }

    /// How many are burning.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hazards.len()
    }

    /// Whether nothing is burning.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hazards.is_empty()
    }

    /// Lights a patch of ground and returns its id.
    ///
    /// `expired` receives the id and centre of anything dropped to make room, so the caller
    /// can announce its end — a visual whose end was never announced is a visual that burns
    /// on the floor forever.
    #[allow(
        clippy::too_many_arguments,
        reason = "a hazard is this many facts, and naming a struct for one call site hides them"
    )]
    pub fn spawn(
        &mut self,
        owner: EntityId,
        ability: EnemyAbility,
        center: Vec3,
        radius: f32,
        duration: f32,
        damage_per_second: f32,
        expired: &mut Vec<(u32, Vec3)>,
    ) -> u32 {
        if self.hazards.len() >= MAX_ENEMY_HAZARDS {
            let dropped = self.hazards.remove(0);
            expired.push((dropped.id, dropped.center));
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.hazards.push(EnemyHazard {
            id,
            owner,
            ability,
            center,
            radius: radius.max(0.0),
            remaining: duration.max(0.0),
            duration: duration.max(0.0),
            damage_per_second: damage_per_second.max(0.0),
            grow_to: 0.0,
            timer: HAZARD_TICK_INTERVAL,
            grow_timer: HAZARD_GROWTH_INTERVAL,
        });
        id
    }

    /// Makes hazard `id` spread to `radius` over its life.
    pub fn set_growth(&mut self, id: u32, radius: f32) {
        if let Some(hazard) = self.hazards.iter_mut().find(|hazard| hazard.id == id) {
            hazard.grow_to = if radius.is_finite() {
                radius.max(0.0)
            } else {
                0.0
            };
        }
    }

    /// Grows every spreading hazard whose step is due, copying each one that grew into
    /// `grown` so its new size can be announced.
    pub fn grow(&mut self, step: f32, grown: &mut Vec<EnemyHazard>) {
        let step = if step.is_finite() { step.max(0.0) } else { 0.0 };
        for hazard in &mut self.hazards {
            if hazard.grow_to <= hazard.radius {
                continue;
            }
            hazard.grow_timer -= step;
            if hazard.grow_timer > crate::TIME_EPSILON {
                continue;
            }
            hazard.grow_timer += HAZARD_GROWTH_INTERVAL;
            hazard.radius = (hazard.radius + HAZARD_GROWTH_STEP).min(hazard.grow_to);
            grown.push(*hazard);
        }
    }

    /// Advances every hazard, collecting the damage it deals and the ones that burned out.
    ///
    /// Deterministic: the tick cadence is driven by the fixed timestep and nothing here
    /// reads a clock or a random number.
    pub fn tick(&mut self, step: f32, ticks: &mut Vec<HazardTick>, expired: &mut Vec<(u32, Vec3)>) {
        let step = if step.is_finite() { step.max(0.0) } else { 0.0 };
        for hazard in &mut self.hazards {
            hazard.remaining = (hazard.remaining - step).max(0.0);
            hazard.timer -= step;
            // A loop rather than a branch: one long catch-up step must not swallow ticks.
            // The epsilon is what stops a hazard ticked in sixtieths of a second from losing
            // a tick to accumulated float error and dealing one fewer than it should.
            while hazard.timer <= crate::TIME_EPSILON {
                hazard.timer += HAZARD_TICK_INTERVAL;
                if hazard.damage_per_second > 0.0 {
                    ticks.push(HazardTick {
                        id: hazard.id,
                        owner: hazard.owner,
                        ability: hazard.ability,
                        center: hazard.center,
                        radius: hazard.radius,
                        damage: hazard.damage_per_second * HAZARD_TICK_INTERVAL,
                    });
                }
            }
        }
        self.hazards.retain(|hazard| {
            if hazard.remaining > 0.0 {
                return true;
            }
            expired.push((hazard.id, hazard.center));
            false
        });
    }

    /// Puts out everything `owner` lit.
    ///
    /// What a death runs: a pool whose caster is gone is a pool nothing will ever explain.
    pub fn clear_owner(&mut self, owner: EntityId, expired: &mut Vec<(u32, Vec3)>) {
        self.hazards.retain(|hazard| {
            if hazard.owner != owner {
                return true;
            }
            expired.push((hazard.id, hazard.center));
            false
        });
    }

    /// Puts everything out. What a reset runs.
    pub fn clear_into(&mut self, expired: &mut Vec<(u32, Vec3)>) {
        for hazard in self.hazards.drain(..) {
            expired.push((hazard.id, hazard.center));
        }
    }

    /// Whether `point`, on a body of `body_radius`, is standing in hazard `index`.
    #[must_use]
    pub fn contains(hazard: &EnemyHazard, point: Vec3, body_radius: f32) -> bool {
        let (dx, dz) = (point.x - hazard.center.x, point.z - hazard.center.z);
        (dx * dx + dz * dz).sqrt() - body_radius <= hazard.radius
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn field() -> (EnemyHazardField, Vec<(u32, Vec3)>) {
        (EnemyHazardField::new(), Vec::new())
    }

    #[test]
    fn a_hazard_ticks_at_a_fixed_cadence_and_stops_at_its_duration() {
        let (mut field, mut expired) = field();
        field.spawn(
            EntityId::new(1, 1),
            EnemyAbility::CinderPool,
            Vec3::ZERO,
            3.5,
            2.0,
            14.0,
            &mut expired,
        );

        let mut ticks = Vec::new();
        // Two seconds at a half-second cadence is four ticks, and then it is gone.
        for _ in 0..20 {
            field.tick(0.1, &mut ticks, &mut expired);
        }
        assert_eq!(ticks.len(), 4);
        assert!((ticks[0].damage - 7.0).abs() < 1e-4);
        assert!(field.is_empty());
        assert_eq!(expired.len(), 1);
    }

    #[test]
    fn one_long_step_does_not_swallow_ticks() {
        let (mut field, mut expired) = field();
        field.spawn(
            EntityId::new(1, 1),
            EnemyAbility::CinderPool,
            Vec3::ZERO,
            3.5,
            4.0,
            10.0,
            &mut expired,
        );
        let mut ticks = Vec::new();
        field.tick(2.0, &mut ticks, &mut expired);
        assert_eq!(ticks.len(), 4, "two seconds is four half-second ticks");
    }

    #[test]
    fn a_body_is_caught_by_its_edge() {
        let (mut field, mut expired) = field();
        field.spawn(
            EntityId::new(1, 1),
            EnemyAbility::CinderPool,
            Vec3::ZERO,
            3.5,
            9.0,
            14.0,
            &mut expired,
        );
        let hazard = &field.hazards()[0];
        assert!(EnemyHazardField::contains(
            hazard,
            Vec3::new(4.0, 0.0, 0.0),
            0.6
        ));
        assert!(!EnemyHazardField::contains(
            hazard,
            Vec3::new(5.0, 0.0, 0.0),
            0.6
        ));
        // Height is ignored: a hazard is a footprint on the floor.
        assert!(EnemyHazardField::contains(
            hazard,
            Vec3::new(0.0, 40.0, 0.0),
            0.0
        ));
    }

    #[test]
    fn clearing_an_owner_leaves_everyone_elses_ground_burning() {
        let (mut field, mut expired) = field();
        let mine = EntityId::new(1, 1);
        let theirs = EntityId::new(2, 1);
        field.spawn(
            mine,
            EnemyAbility::CinderPool,
            Vec3::ZERO,
            3.5,
            9.0,
            14.0,
            &mut expired,
        );
        field.spawn(
            theirs,
            EnemyAbility::CinderRain,
            Vec3::new(10.0, 0.0, 0.0),
            4.5,
            10.0,
            16.0,
            &mut expired,
        );

        expired.clear();
        field.clear_owner(mine, &mut expired);
        assert_eq!(field.len(), 1);
        assert_eq!(expired.len(), 1);
        assert_eq!(field.hazards()[0].owner, theirs);

        expired.clear();
        field.clear_into(&mut expired);
        assert!(field.is_empty());
        assert_eq!(expired.len(), 1, "every removal is announced");
    }

    #[test]
    fn the_field_is_bounded_and_announces_what_it_drops() {
        let (mut field, mut expired) = field();
        for index in 0..MAX_ENEMY_HAZARDS + 3 {
            field.spawn(
                EntityId::new(1, 1),
                EnemyAbility::CinderPool,
                Vec3::new(index as f32, 0.0, 0.0),
                3.5,
                9.0,
                14.0,
                &mut expired,
            );
        }
        assert_eq!(field.len(), MAX_ENEMY_HAZARDS);
        assert_eq!(expired.len(), 3, "every dropped hazard is announced");
    }
}
