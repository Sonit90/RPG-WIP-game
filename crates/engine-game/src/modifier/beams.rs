//! Rotating Beams: emitters, their geometry, and the bounded list that holds them.
//!
//! ```text
//!   engage ──▶ emitter at the pack's centroid
//!                │  warm-up (outline, no damage)   │  active (filled, damages)
//!                ├───────────────────────────────────┼──────────────────────────▶ encounter ends
//!                yaw_i(t) = start + i·2π/n + ω·(t − t₀)
//! ```
//!
//! # A pure function of time
//!
//! Where a beam points is `start_angle + i · 2π / beams + angular_speed · (t − start_time)`
//! and nothing else. The server hits players with it; a client draws it every frame from the
//! one start event. There is no per-frame sync and no accumulated state to drift: two
//! machines that agree on the start event agree on every angle after it.
//!
//! # It deals no damage
//!
//! [`BeamField`] only knows geometry. The simulation asks it who is inside
//! ([`BeamEmitter::contains`]) and deals the damage itself, through the one `deal_damage`.

use engine_core::math::Vec3;

use crate::enemy::TelegraphShape;

/// The most emitters that can stand at once.
///
/// A dungeon pulls a few packs at a time. Past this the oldest emitter is dropped (and
/// announced) rather than the newest refused: the newest is the fight the party is in.
pub const MAX_BEAM_EMITTERS: usize = 8;

/// One emitter: a set of beams turning about a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamEmitter {
    /// Names it in events.
    pub id: u32,
    /// The encounter it belongs to; `usize::MAX` for a mirror on a client, which has no
    /// encounters to ask.
    pub encounter: usize,
    /// Where the beams meet.
    pub center: Vec3,
    /// Yaw of beam zero at `start_time`, radians about +Y. Zero faces −Z.
    pub start_angle: f32,
    /// Radians a second; the sign is the direction.
    pub angular_speed: f32,
    /// How far each beam reaches, in metres.
    pub radius: f32,
    /// How wide each beam is, edge to edge, in metres.
    pub width: f32,
    /// How many beams, evenly spaced.
    pub beams: u8,
    /// When it appeared, in the observer's seconds.
    pub start_time: f64,
    /// When it starts to hurt.
    pub warmup_end_time: f64,
}

impl BeamEmitter {
    /// Yaw of beam `index` at time `now`, radians about +Y. Zero faces −Z.
    #[must_use]
    pub fn yaw(&self, index: u8, now: f64) -> f32 {
        let spacing = std::f32::consts::TAU / f32::from(self.beams.max(1));
        let elapsed = (now - self.start_time) as f32;
        self.start_angle + spacing * f32::from(index) + self.angular_speed * elapsed
    }

    /// Whether it is still only a warning at `now`.
    #[must_use]
    pub fn is_warming(&self, now: f64) -> bool {
        now < self.warmup_end_time
    }

    /// The shape of beam `index` at `now`, in the same type every other line on the floor is.
    #[must_use]
    pub fn shape(&self, index: u8, now: f64) -> TelegraphShape {
        TelegraphShape::Line {
            yaw: self.yaw(index, now),
            length: self.radius,
            width: self.width,
        }
    }

    /// Whether a body of `body_radius` at `point` is inside any beam at `now`.
    ///
    /// Height is ignored: a beam is a footprint on the floor.
    #[must_use]
    pub fn contains(&self, now: f64, point: Vec3, body_radius: f32) -> bool {
        (0..self.beams).any(|index| {
            self.shape(index, now)
                .contains(self.center, point, body_radius)
        })
    }
}

/// Every emitter standing.
#[derive(Debug, Clone, Default)]
pub struct BeamField {
    emitters: Vec<BeamEmitter>,
}

impl BeamField {
    /// An empty field.
    #[must_use]
    pub fn new() -> Self {
        Self {
            emitters: Vec::with_capacity(MAX_BEAM_EMITTERS),
        }
    }

    /// Every emitter, oldest first.
    #[must_use]
    pub fn emitters(&self) -> &[BeamEmitter] {
        &self.emitters
    }

    /// The emitter belonging to `encounter`, if it has one.
    #[must_use]
    pub fn of_encounter(&self, encounter: usize) -> Option<&BeamEmitter> {
        self.emitters
            .iter()
            .find(|emitter| emitter.encounter == encounter)
    }

    /// Adds an emitter, returning the id of the oldest if one was dropped to make room.
    pub fn insert(&mut self, emitter: BeamEmitter) -> Option<u32> {
        let dropped =
            (self.emitters.len() >= MAX_BEAM_EMITTERS).then(|| self.emitters.remove(0).id);
        self.emitters.push(emitter);
        dropped
    }

    /// Takes emitter `id` away. Returns whether it was there.
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.emitters.len();
        self.emitters.retain(|emitter| emitter.id != id);
        self.emitters.len() != before
    }

    /// Takes everything away, yielding the ids.
    pub fn drain_ids(&mut self) -> impl Iterator<Item = u32> + '_ {
        self.emitters.drain(..).map(|emitter| emitter.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, TAU};

    fn emitter() -> BeamEmitter {
        BeamEmitter {
            id: 1,
            encounter: 0,
            center: Vec3::new(10.0, 0.0, 10.0),
            start_angle: 0.0,
            angular_speed: 35.0_f32.to_radians(),
            radius: 9.0,
            width: 0.9,
            beams: 3,
            start_time: 100.0,
            warmup_end_time: 102.0,
        }
    }

    #[test]
    fn beams_are_evenly_spaced_and_turn_at_the_angular_speed() {
        let emitter = emitter();
        let gap = emitter.yaw(1, 100.0) - emitter.yaw(0, 100.0);
        assert!((gap - TAU / 3.0).abs() < 1e-5, "120 degrees apart");
        let turned = emitter.yaw(0, 104.0) - emitter.yaw(0, 100.0);
        assert!((turned - 35.0_f32.to_radians() * 4.0).abs() < 1e-5);
    }

    #[test]
    fn the_angle_is_a_pure_function_of_the_clock() {
        let emitter = emitter();
        // Asking in a different order, or twice, changes nothing: there is no state to drift.
        let late = emitter.yaw(2, 123.456);
        let _ = emitter.yaw(2, 100.0);
        assert_eq!(emitter.yaw(2, 123.456), late);
    }

    #[test]
    fn a_point_is_inside_a_beam_only_where_the_beam_is_at_that_time() {
        let mut emitter = emitter();
        emitter.angular_speed = FRAC_PI_2; // a quarter turn a second
        // Beam zero points down -Z (yaw 0): a point 5 m ahead is inside at t0.
        let ahead = emitter.center + Vec3::new(0.0, 0.0, -5.0);
        assert!(emitter.contains(100.0, ahead, 0.0));
        // A quarter turn later (+yaw turns towards -X) that beam has moved off it...
        assert!(
            !emitter.contains(101.0, ahead, 0.0) || {
                // ...unless another of the three happens to cover it, which at 90 degrees of
                // turn and 120 of spacing none does.
                false
            }
        );
        // ...and is now over the point 5 m to the -X side.
        let side = emitter.center + Vec3::new(-5.0, 0.0, 0.0);
        assert!(emitter.contains(101.0, side, 0.0));
    }

    #[test]
    fn reach_and_width_bound_a_beam() {
        let emitter = emitter();
        let t = 100.0;
        assert!(emitter.contains(t, emitter.center + Vec3::new(0.0, 0.0, -8.9), 0.0));
        assert!(!emitter.contains(t, emitter.center + Vec3::new(0.0, 0.0, -9.6), 0.0));
        assert!(!emitter.contains(t, emitter.center + Vec3::new(0.6, 0.0, -5.0), 0.0));
        // A body's radius widens the beam it touches.
        assert!(emitter.contains(t, emitter.center + Vec3::new(0.6, 0.0, -5.0), 0.3));
    }

    #[test]
    fn warm_up_ends_at_its_time() {
        let emitter = emitter();
        assert!(emitter.is_warming(101.9));
        assert!(!emitter.is_warming(102.0));
    }

    #[test]
    fn the_field_is_bounded_and_reports_what_it_drops() {
        let mut field = BeamField::new();
        for index in 0..MAX_BEAM_EMITTERS as u32 {
            assert_eq!(
                field.insert(BeamEmitter {
                    id: index,
                    ..emitter()
                }),
                None
            );
        }
        let dropped = field.insert(BeamEmitter {
            id: 99,
            ..emitter()
        });
        assert_eq!(dropped, Some(0), "the oldest goes");
        assert_eq!(field.emitters().len(), MAX_BEAM_EMITTERS);
    }
}
