//! Smooth motion for everyone the local player does not control.
//!
//! Server updates arrive at the server's pace, irregularly. Each remote player keeps a short
//! history of where the server said they were and when that arrived; the game draws them at
//! `now − delay`, between two known positions, so a 30 Hz stream looks continuous at any frame
//! rate. The delay costs a tenth of a second of latency on *other* players and nothing on the
//! local one, which is predicted instead.

use std::collections::VecDeque;

use engine_core::math::Vec3;

/// How far behind the newest sample remote players are drawn.
pub const INTERPOLATION_DELAY: f64 = 0.1;

/// Samples kept per entity: about a second at 30 Hz.
const CAPACITY: usize = 32;

/// One known state.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    time: f64,
    position: Vec3,
    yaw: f32,
}

/// A remote entity's recent positions.
#[derive(Debug, Clone, Default)]
pub struct SnapshotBuffer {
    samples: VecDeque<Sample>,
}

impl SnapshotBuffer {
    /// Records a state that arrived at local time `time` (seconds, monotonic).
    pub fn push(&mut self, time: f64, position: Vec3, yaw: f32) {
        if self.samples.back().is_some_and(|last| time < last.time) {
            return;
        }
        self.samples.push_back(Sample {
            time,
            position,
            yaw,
        });
        while self.samples.len() > CAPACITY {
            self.samples.pop_front();
        }
    }

    /// Where to draw it at local time `now`: interpolated at `now − delay`, clamped to the
    /// oldest and newest samples (it never extrapolates past what the server said).
    #[must_use]
    pub fn sample(&self, now: f64) -> Option<(Vec3, f32)> {
        let target = now - INTERPOLATION_DELAY;
        let first = self.samples.front()?;
        if target <= first.time {
            return Some((first.position, first.yaw));
        }
        for pair in self.samples.iter().zip(self.samples.iter().skip(1)) {
            let (a, b) = pair;
            if target <= b.time {
                let span = b.time - a.time;
                #[allow(clippy::cast_possible_truncation, reason = "a fraction in [0, 1]")]
                let t = if span <= f64::EPSILON {
                    1.0
                } else {
                    ((target - a.time) / span) as f32
                };
                return Some((a.position.lerp(b.position, t), lerp_angle(a.yaw, b.yaw, t)));
            }
        }
        self.samples.back().map(|last| (last.position, last.yaw))
    }

    /// Forgets everything (a teleport, a re-entry).
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// Interpolates two angles the short way round.
#[must_use]
pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let delta = (b - a + PI).rem_euclid(TAU) - PI;
    a + delta * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_between_samples_behind_the_newest() {
        let mut buffer = SnapshotBuffer::default();
        buffer.push(0.0, Vec3::ZERO, 0.0);
        buffer.push(0.1, Vec3::new(1.0, 0.0, 0.0), 0.0);
        buffer.push(0.2, Vec3::new(2.0, 0.0, 0.0), 0.0);
        let (position, _) = buffer.sample(0.25).unwrap_or((Vec3::ZERO, 0.0));
        assert!((position.x - 1.5).abs() < 1e-4);
    }

    #[test]
    fn never_extrapolates() {
        let mut buffer = SnapshotBuffer::default();
        buffer.push(0.0, Vec3::ZERO, 0.0);
        buffer.push(0.1, Vec3::new(1.0, 0.0, 0.0), 0.0);
        let (position, _) = buffer.sample(10.0).unwrap_or((Vec3::ZERO, 0.0));
        assert!((position.x - 1.0).abs() < 1e-4);
        let (position, _) = buffer.sample(-5.0).unwrap_or((Vec3::ONE, 0.0));
        assert!(position.x.abs() < 1e-4);
        assert!(SnapshotBuffer::default().sample(0.0).is_none());
    }

    #[test]
    fn angles_take_the_short_way() {
        let halfway = lerp_angle(3.0, -3.0, 0.5);
        assert!(halfway.abs() > 3.0, "went through zero: {halfway}");
    }

    #[test]
    fn out_of_order_samples_are_dropped() {
        let mut buffer = SnapshotBuffer::default();
        buffer.push(1.0, Vec3::ONE, 0.0);
        buffer.push(0.5, Vec3::ZERO, 0.0);
        let (position, _) = buffer.sample(2.0).unwrap_or((Vec3::ZERO, 0.0));
        assert!((position.x - 1.0).abs() < 1e-4);
    }
}
