//! Plague Explosion: fuses, clouds, and the bounded lists that hold them.
//!
//! ```text
//!   monster dies ──▶ PlagueFuse (5 s) ──▶ burst (damage inside radius) ──▶ PlagueCloud (6 s)
//!                                                                            │ ticks, applies Plagued
//!                                                                            ▼ ends
//! ```
//!
//! Like [`crate::enemy::hazard`], this holds time and shape and deals no damage: the
//! simulation turns what [`PlagueField::advance`] reports into `deal_damage` calls and
//! status applications.
//!
//! # Bounded
//!
//! A mass pull can kill a dozen monsters in a second. Fuses and clouds live in fixed-capacity
//! lists; past the cap the oldest is dropped (and announced) so the cost of a stampede is a
//! constant, not a function of how many things died.

use engine_core::math::Vec3;

/// The most corpses that can be festering at once.
pub const MAX_PLAGUE_FUSES: usize = 48;

/// The most clouds that can be on the ground at once.
pub const MAX_PLAGUE_CLOUDS: usize = 16;

/// A corpse about to burst.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlagueFuse {
    /// Names it through its fuse, burst and cloud.
    pub id: u32,
    /// Where it lies.
    pub position: Vec3,
    /// Radius of the burst, in metres.
    pub radius: f32,
    /// When the fuse was lit, in the observer's seconds.
    pub armed_time: f64,
    /// When it bursts.
    pub explode_time: f64,
}

impl PlagueFuse {
    /// How far along the fuse is at `now`, `0..=1`: the progress fill of its warning.
    #[must_use]
    pub fn progress(&self, now: f64) -> f32 {
        let span = self.explode_time - self.armed_time;
        if span <= 0.0 {
            return 1.0;
        }
        (((now - self.armed_time) / span) as f32).clamp(0.0, 1.0)
    }
}

/// Plague on the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlagueCloud {
    /// Matches the fuse it came from.
    pub id: u32,
    /// Where it is centred.
    pub position: Vec3,
    /// Radius, in metres.
    pub radius: f32,
    /// When it thins out.
    pub end_time: f64,
}

/// What one [`PlagueField::advance`] produced, in reusable buffers.
#[derive(Debug, Default, Clone)]
pub struct PlagueStep {
    /// Fuses that ran out this step: they burst now.
    pub burst: Vec<PlagueFuse>,
    /// Ids of clouds that thinned out this step.
    pub ended: Vec<u32>,
}

impl PlagueStep {
    /// Empties both lists, keeping their allocations.
    pub fn clear(&mut self) {
        self.burst.clear();
        self.ended.clear();
    }
}

/// Every fuse and cloud.
#[derive(Debug, Clone, Default)]
pub struct PlagueField {
    fuses: Vec<PlagueFuse>,
    clouds: Vec<PlagueCloud>,
}

/// How early a fuse may fire, in seconds: the same float-error allowance every other
/// fixed-step timer in this crate uses.
const FIRE_EPSILON: f64 = 1e-4;

impl PlagueField {
    /// An empty field.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fuses: Vec::with_capacity(MAX_PLAGUE_FUSES),
            clouds: Vec::with_capacity(MAX_PLAGUE_CLOUDS),
        }
    }

    /// Every corpse festering, oldest first.
    #[must_use]
    pub fn fuses(&self) -> &[PlagueFuse] {
        &self.fuses
    }

    /// Every cloud, oldest first.
    #[must_use]
    pub fn clouds(&self) -> &[PlagueCloud] {
        &self.clouds
    }

    /// Lights a fuse; returns the id of the oldest if one was dropped to make room.
    pub fn insert_fuse(&mut self, fuse: PlagueFuse) -> Option<u32> {
        let dropped = (self.fuses.len() >= MAX_PLAGUE_FUSES).then(|| self.fuses.remove(0).id);
        self.fuses.push(fuse);
        dropped
    }

    /// Lays a cloud; returns the id of the oldest if one was dropped to make room.
    pub fn insert_cloud(&mut self, cloud: PlagueCloud) -> Option<u32> {
        let dropped = (self.clouds.len() >= MAX_PLAGUE_CLOUDS).then(|| self.clouds.remove(0).id);
        self.clouds.push(cloud);
        dropped
    }

    /// Takes fuse `id` away. Returns whether it was there.
    pub fn remove_fuse(&mut self, id: u32) -> bool {
        let before = self.fuses.len();
        self.fuses.retain(|fuse| fuse.id != id);
        self.fuses.len() != before
    }

    /// Takes cloud `id` away. Returns whether it was there.
    pub fn remove_cloud(&mut self, id: u32) -> bool {
        let before = self.clouds.len();
        self.clouds.retain(|cloud| cloud.id != id);
        self.clouds.len() != before
    }

    /// Moves time to `now`: fuses that are due leave the field into `step.burst` (in the
    /// order they were lit), clouds past their end into `step.ended`.
    ///
    /// Deterministic: it reads the clock it is given and nothing else. A fuse fires on the
    /// first step at or after `armed + fuse`, so a 5 s fuse lit on tick `n` bursts on tick
    /// `n + 300` and not one earlier.
    pub fn advance(&mut self, now: f64, step: &mut PlagueStep) {
        self.fuses.retain(|fuse| {
            if now + FIRE_EPSILON >= fuse.explode_time {
                step.burst.push(*fuse);
                return false;
            }
            true
        });
        self.clouds.retain(|cloud| {
            if now + FIRE_EPSILON >= cloud.end_time {
                step.ended.push(cloud.id);
                return false;
            }
            true
        });
    }

    /// Whether `point`, on a body of `body_radius`, stands in a cloud.
    #[must_use]
    pub fn in_cloud(&self, point: Vec3, body_radius: f32) -> bool {
        self.clouds
            .iter()
            .any(|cloud| inside(cloud.position, cloud.radius, point, body_radius))
    }

    /// Drops mirrored ground that outlived any plausible end event.
    pub fn prune(&mut self, now: f64) {
        self.fuses.retain(|fuse| now < fuse.explode_time + 10.0);
        self.clouds.retain(|cloud| now < cloud.end_time);
    }

    /// Takes everything away, yielding the ids.
    pub fn drain_ids(&mut self) -> impl Iterator<Item = u32> + '_ {
        let fuses = self.fuses.drain(..).map(|fuse| fuse.id);
        let clouds = self.clouds.drain(..).map(|cloud| cloud.id);
        fuses.chain(clouds)
    }
}

/// Whether a body of `body_radius` at `point` touches the disc of `radius` about `center`.
/// Height is ignored: a cloud is a footprint on the floor.
#[must_use]
pub fn inside(center: Vec3, radius: f32, point: Vec3, body_radius: f32) -> bool {
    let (dx, dz) = (point.x - center.x, point.z - center.z);
    (dx * dx + dz * dz).sqrt() - body_radius <= radius
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fuse(id: u32, armed: f64) -> PlagueFuse {
        PlagueFuse {
            id,
            position: Vec3::ZERO,
            radius: 4.0,
            armed_time: armed,
            explode_time: armed + 5.0,
        }
    }

    #[test]
    fn a_fuse_bursts_on_its_time_and_not_before() {
        let mut field = PlagueField::new();
        field.insert_fuse(fuse(1, 10.0));
        let mut step = PlagueStep::default();
        field.advance(14.99, &mut step);
        assert!(step.burst.is_empty());
        field.advance(15.0, &mut step);
        assert_eq!(step.burst.len(), 1);
        assert!(field.fuses().is_empty());
    }

    #[test]
    fn progress_fills_towards_the_burst() {
        let fuse = fuse(1, 10.0);
        assert_eq!(fuse.progress(10.0), 0.0);
        assert!((fuse.progress(12.5) - 0.5).abs() < 1e-6);
        assert_eq!(fuse.progress(99.0), 1.0);
    }

    #[test]
    fn clouds_end_and_are_reported_once() {
        let mut field = PlagueField::new();
        field.insert_cloud(PlagueCloud {
            id: 7,
            position: Vec3::ZERO,
            radius: 4.0,
            end_time: 6.0,
        });
        let mut step = PlagueStep::default();
        field.advance(5.9, &mut step);
        assert!(step.ended.is_empty());
        field.advance(6.0, &mut step);
        assert_eq!(step.ended, vec![7]);
        step.clear();
        field.advance(7.0, &mut step);
        assert!(step.ended.is_empty());
    }

    #[test]
    fn both_lists_are_bounded_and_report_what_they_drop() {
        let mut field = PlagueField::new();
        for id in 0..MAX_PLAGUE_FUSES as u32 {
            assert_eq!(field.insert_fuse(fuse(id, 0.0)), None);
        }
        assert_eq!(field.insert_fuse(fuse(999, 0.0)), Some(0));
        assert_eq!(field.fuses().len(), MAX_PLAGUE_FUSES);
        for id in 0..MAX_PLAGUE_CLOUDS as u32 {
            let cloud = PlagueCloud {
                id,
                position: Vec3::ZERO,
                radius: 4.0,
                end_time: 9.0,
            };
            assert_eq!(field.insert_cloud(cloud), None);
        }
        let cloud = PlagueCloud {
            id: 500,
            position: Vec3::ZERO,
            radius: 4.0,
            end_time: 9.0,
        };
        assert_eq!(field.insert_cloud(cloud), Some(0));
        assert_eq!(field.clouds().len(), MAX_PLAGUE_CLOUDS);
    }

    #[test]
    fn a_body_is_caught_by_its_edge() {
        assert!(inside(Vec3::ZERO, 4.0, Vec3::new(4.5, 3.0, 0.0), 0.6));
        assert!(!inside(Vec3::ZERO, 4.0, Vec3::new(5.0, 0.0, 0.0), 0.6));
    }
}
