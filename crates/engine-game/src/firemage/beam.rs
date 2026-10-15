//! Moving Fire Beam: the FireMage's Ultimate, as state and rules.
//!
//! ```text
//! cast ──► BeamChannel::start ──► every tick: steer / turn / advance ──► ticks due
//!                                                                          │
//!                        the simulation hits what is inside shape() ◄──────┘
//! ```
//!
//! A six-second channel that originates at the mage, reaches 22 m and is 2.6 m wide. The
//! direction is **steered**: the player's aim point is the direction the beam wants to face
//! and the beam turns towards it at [`BEAM_TURN_RATE_DEGREES`] a second at most, so it sweeps
//! across a room and can never snap from one pack to the next. Everything inside the
//! rectangle takes [`BEAM_TICK_COEFFICIENT`] of spell power every [`BEAM_TICK_INTERVAL`] and
//! a stack of Ignite once a second.
//!
//! Like every module under [`crate::firemage`] this one is state and rules with no side
//! effects. It reports *that* a tick is due; the simulation decides who stands in the beam,
//! what the tick is worth against the stats of that moment and whether anything dies. The
//! same state also runs on a networked client, where it only turns (so the beam can be
//! drawn where the player is steering) and deals nothing — see
//! [`crate::sim::remote`].
//!
//! ## Tuning
//!
//! Every magnitude is a named constant here and is read through the ability row
//! ([`super::abilities`]) or directly, never restated, so moving them to data later is one
//! change. The damage coefficient is chosen against Pyroblast (see
//! [`BEAM_TICK_COEFFICIENT`]).

use std::f32::consts::{PI, TAU};

use engine_core::math::Vec3;

use super::zone::ZoneShape;

/// Seconds the channel lasts.
pub const BEAM_DURATION: f32 = 6.0;

/// How far the beam reaches from the mage, in metres.
pub const BEAM_LENGTH: f32 = 22.0;

/// How wide the beam is, in metres.
pub const BEAM_WIDTH: f32 = 2.6;

/// The fastest the beam turns, in degrees a second.
///
/// A half turn takes 2.4 s: a player who swings the cursor behind them watches the beam
/// come round for a good part of the channel, which is what stops it being a hitscan
/// cone with a long duration.
pub const BEAM_TURN_RATE_DEGREES: f32 = 75.0;

/// [`BEAM_TURN_RATE_DEGREES`] in radians a second.
pub const BEAM_TURN_RATE: f32 = BEAM_TURN_RATE_DEGREES * (PI / 180.0);

/// Seconds between damage ticks.
pub const BEAM_TICK_INTERVAL: f32 = 0.25;

/// How many ticks a full channel pays.
pub const BEAM_TICKS: u32 = 24;

/// Damage of one tick, as a fraction of spell power.
///
/// **Against Pyroblast.** A Pyroblast is 6.50, so 24 ticks of 0.95 is 22.8 on a lone target:
/// 3.5 Pyroblasts for six seconds of channel and a full Ultimate bar. It is also 22.8 on
/// *every* enemy inside the rectangle, so on a pack of eight it is the largest burst the
/// class has by a wide margin — which is what an Ultimate is for. The simulator validates it
/// (`docs/balance/sim-report.md`).
pub const BEAM_TICK_COEFFICIENT: f32 = 0.95;

/// Seconds between the Ignite stacks the beam applies to everything inside it.
pub const BEAM_IGNITE_INTERVAL: f32 = 1.0;

/// Damage ticks between Ignite stacks: every fourth, so one stack a second.
pub const BEAM_IGNITE_EVERY_TICKS: u32 = 4;

/// Ignite stacks one application adds.
pub const BEAM_IGNITE_STACKS: u8 = 1;

/// The mage's movement speed while channelling, as a fraction of normal.
pub const BEAM_MOVE_SPEED_MULTIPLIER: f32 = 0.40;

/// Seconds before the Ultimate can be used again, counted from the cast.
pub const BEAM_COOLDOWN: f32 = 90.0;

/// The Ultimate Energy the cast consumes: all of it.
pub const BEAM_ENERGY_COST: f32 = 100.0;

/// The nearest an aim point may be to the mage and still mean a direction, in metres.
///
/// Closer than this the bearing to it is noise, so the beam keeps the way it is facing.
pub const BEAM_MIN_AIM_DISTANCE: f32 = 0.3;

/// The fewest seconds between the yaw reports the authority sends, so at most ten a second.
pub const BEAM_REPORT_INTERVAL: f32 = 0.1;

/// How far the steering target must move before it is worth reporting, in radians.
///
/// Half a degree: below it a remote observer extrapolating at the turn rate is already
/// indistinguishable from the truth.
pub const BEAM_REPORT_EPSILON: f32 = 0.5 * (PI / 180.0);

/// How far a client's predicted yaw may differ from the authority's before it is snapped to
/// it, in radians. Ten degrees: smaller errors are the round trip, not a disagreement.
pub const BEAM_CORRECTION_SNAP: f32 = 10.0 * (PI / 180.0);

/// How the channel ended.
///
/// Carried on the wire by position, so the order is part of the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BeamEnd {
    /// It ran its full six seconds.
    Completed,
    /// The mage pressed the Ultimate again.
    Cancelled,
    /// The mage was stunned.
    Stunned,
    /// The mage was silenced.
    Silenced,
    /// The mage died.
    Died,
    /// The mage cast something else.
    Interrupted,
    /// The fight was reset or the mage became another hero.
    Reset,
}

impl BeamEnd {
    /// Every reason, in wire order.
    pub const ALL: [Self; 7] = [
        Self::Completed,
        Self::Cancelled,
        Self::Stunned,
        Self::Silenced,
        Self::Died,
        Self::Interrupted,
        Self::Reset,
    ];

    /// The reason at wire position `index`; anything unknown reads as a reset.
    #[must_use]
    pub fn from_index(index: u8) -> Self {
        Self::ALL
            .get(usize::from(index))
            .copied()
            .unwrap_or(Self::Reset)
    }

    /// Its wire position.
    #[must_use]
    pub fn index(self) -> u8 {
        Self::ALL
            .iter()
            .position(|reason| *reason == self)
            .map_or(u8::MAX, |index| index as u8)
    }

    /// Whether the channel ran its course, which is the only ending that is not a loss.
    #[must_use]
    pub fn is_completed(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Brings an angle into `-PI..PI`.
#[must_use]
pub fn wrap_angle(angle: f32) -> f32 {
    angle - TAU * ((angle + PI) / TAU).floor()
}

/// Turns `current` towards `target` by at most `max_delta` radians, the short way round.
#[must_use]
pub fn turn_towards(current: f32, target: f32, max_delta: f32) -> f32 {
    let difference = wrap_angle(target - current);
    if difference.abs() <= max_delta {
        return target;
    }
    current + max_delta.copysign(difference)
}

/// The yaw that faces `aim` from `origin`, or `None` when it is too close to mean anything.
///
/// Yaw zero faces -Z, the convention every aimed ability uses.
#[must_use]
pub fn yaw_to(origin: Vec3, aim: Vec3) -> Option<f32> {
    let (dx, dz) = (aim.x - origin.x, aim.z - origin.z);
    if (dx * dx + dz * dz).sqrt() < BEAM_MIN_AIM_DISTANCE {
        return None;
    }
    Some((-dx).atan2(-dz))
}

/// What one step of the channel produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BeamAdvance {
    /// The 1-based index of the first damage tick that came due, if any did.
    pub first_tick: u32,
    /// How many damage ticks came due this step.
    pub ticks: u32,
    /// Whether the channel has now run its full length.
    pub finished: bool,
}

/// The channel in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamChannel {
    active: bool,
    /// Which way the beam points now, radians about +Y.
    yaw: f32,
    /// The ground point the player is steering towards.
    aim: Option<Vec3>,
    /// Seconds the channel has run.
    elapsed: f32,
    /// Damage ticks already paid.
    ticks_paid: u32,
    /// The steering target last reported to observers.
    reported_target: f32,
    /// Seconds since the last report.
    since_report: f32,
}

impl Default for BeamChannel {
    fn default() -> Self {
        Self::idle()
    }
}

impl BeamChannel {
    /// No channel.
    #[must_use]
    pub const fn idle() -> Self {
        Self {
            active: false,
            yaw: 0.0,
            aim: None,
            elapsed: 0.0,
            ticks_paid: 0,
            reported_target: 0.0,
            since_report: 0.0,
        }
    }

    /// Whether a channel is running.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Which way the beam points now.
    #[must_use]
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    /// The point the player is steering towards, if they have said.
    #[must_use]
    pub fn aim(&self) -> Option<Vec3> {
        self.aim
    }

    /// Seconds the channel has run.
    #[must_use]
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Seconds left.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        if self.active {
            (BEAM_DURATION - self.elapsed).max(0.0)
        } else {
            0.0
        }
    }

    /// How far through the channel it is, `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.active {
            (self.elapsed / BEAM_DURATION).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Damage ticks paid so far.
    #[must_use]
    pub fn ticks_paid(&self) -> u32 {
        self.ticks_paid
    }

    /// Starts a channel pointing at `yaw` and steering towards `aim`.
    pub fn start(&mut self, yaw: f32, aim: Option<Vec3>) {
        *self = Self {
            active: true,
            yaw,
            aim,
            elapsed: 0.0,
            ticks_paid: 0,
            reported_target: yaw,
            since_report: 0.0,
        };
    }

    /// Ends the channel, wherever it had got to.
    pub fn stop(&mut self) {
        *self = Self::idle();
    }

    /// Points the steering at a new ground point.
    pub fn steer(&mut self, aim: Vec3) {
        if self.active {
            self.aim = Some(aim);
        }
    }

    /// The yaw the beam is turning towards from `origin`: the aim point's bearing, or the
    /// way it already faces if there is no usable aim.
    #[must_use]
    pub fn target_yaw(&self, origin: Vec3) -> f32 {
        self.aim
            .and_then(|aim| yaw_to(origin, aim))
            .unwrap_or(self.yaw)
    }

    /// Turns the beam towards its aim by one step, never faster than [`BEAM_TURN_RATE`].
    pub fn turn(&mut self, origin: Vec3, step: f32) {
        if !self.active {
            return;
        }
        let target = self.target_yaw(origin);
        self.yaw = wrap_angle(turn_towards(
            self.yaw,
            target,
            BEAM_TURN_RATE * step.max(0.0),
        ));
    }

    /// Moves the clock on and reports which damage ticks came due.
    ///
    /// A tick is due when the channel has run for `n × BEAM_TICK_INTERVAL`, so a channel pays
    /// [`BEAM_TICKS`] on any machine at any frame rate, however the steps fall.
    pub fn advance(&mut self, step: f32) -> BeamAdvance {
        if !self.active {
            return BeamAdvance::default();
        }
        self.elapsed += step.max(0.0);
        self.since_report += step.max(0.0);
        let due = (((self.elapsed + crate::TIME_EPSILON) / BEAM_TICK_INTERVAL).floor() as u32)
            .min(BEAM_TICKS);
        let report = BeamAdvance {
            first_tick: self.ticks_paid + 1,
            ticks: due.saturating_sub(self.ticks_paid),
            finished: self.elapsed + crate::TIME_EPSILON >= BEAM_DURATION,
        };
        self.ticks_paid = self.ticks_paid.max(due);
        report
    }

    /// Whether damage tick `index` (1-based) also applies Ignite.
    #[must_use]
    pub fn applies_ignite(index: u32) -> bool {
        index > 0 && index % BEAM_IGNITE_EVERY_TICKS == 0
    }

    /// The rectangle the beam covers from `origin`'s point of view, anchored at the mage.
    #[must_use]
    pub fn shape(&self) -> ZoneShape {
        ZoneShape::Line {
            yaw: self.yaw,
            length: BEAM_LENGTH,
            width: BEAM_WIDTH,
        }
    }

    /// The yaw report an observer is owed this step, if any: `(yaw, target_yaw)`.
    ///
    /// At most one every [`BEAM_REPORT_INTERVAL`], and only when the steering target has
    /// moved by [`BEAM_REPORT_EPSILON`] since the last one — the observer turns at the same
    /// rate the beam does, so a target that has not changed needs no news.
    pub fn take_report(&mut self, origin: Vec3) -> Option<(f32, f32)> {
        if !self.active || self.since_report + crate::TIME_EPSILON < BEAM_REPORT_INTERVAL {
            return None;
        }
        let target = self.target_yaw(origin);
        if wrap_angle(target - self.reported_target).abs() < BEAM_REPORT_EPSILON {
            return None;
        }
        self.reported_target = target;
        self.since_report = 0.0;
        Some((self.yaw, target))
    }

    /// Lays an authority's yaw over a predicted one, if they disagree by more than the
    /// round trip explains. Returns whether it snapped.
    pub fn correct(&mut self, authoritative: f32) -> bool {
        if self.active && wrap_angle(authoritative - self.yaw).abs() > BEAM_CORRECTION_SNAP {
            self.yaw = authoritative;
            return true;
        }
        false
    }

    /// Starts a channel that was reported rather than cast, `elapsed` seconds in.
    pub fn present(&mut self, yaw: f32, elapsed: f32) {
        self.start(yaw, None);
        self.elapsed = elapsed.clamp(0.0, BEAM_DURATION);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    fn aimed_at(yaw: f32) -> Vec3 {
        // A point ten metres out along `yaw`, from the origin.
        Vec3::new(-yaw.sin() * 10.0, 0.0, -yaw.cos() * 10.0)
    }

    #[test]
    fn the_constants_agree_with_one_another() {
        assert!((BEAM_TURN_RATE_DEGREES.to_radians() - BEAM_TURN_RATE).abs() < 1e-6);
        assert_eq!(
            (BEAM_DURATION / BEAM_TICK_INTERVAL).round() as u32,
            BEAM_TICKS
        );
        assert_eq!(
            (BEAM_IGNITE_INTERVAL / BEAM_TICK_INTERVAL).round() as u32,
            BEAM_IGNITE_EVERY_TICKS
        );
        assert!((BEAM_REPORT_INTERVAL - 0.1).abs() < 1e-6, "ten a second");
    }

    #[test]
    fn a_channel_pays_twenty_four_ticks_and_then_finishes() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, None);
        let mut paid = 0;
        let mut finished_at = None;
        for step in 1..=400 {
            let advance = beam.advance(STEP);
            paid += advance.ticks;
            if advance.finished && finished_at.is_none() {
                finished_at = Some(step);
            }
        }
        assert_eq!(paid, BEAM_TICKS);
        assert_eq!(
            finished_at,
            Some(360),
            "six seconds at sixty steps a second"
        );
    }

    #[test]
    fn the_tick_count_does_not_depend_on_the_step() {
        for step in [1.0 / 30.0, 1.0 / 60.0, 1.0 / 144.0, 0.4] {
            let mut beam = BeamChannel::idle();
            beam.start(0.0, None);
            let mut paid = 0;
            while !beam.advance(step).finished {
                paid = beam.ticks_paid();
            }
            paid = paid.max(beam.ticks_paid());
            assert_eq!(paid, BEAM_TICKS, "step {step}");
        }
    }

    #[test]
    fn ticks_are_numbered_from_one_and_ignite_is_every_fourth() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, None);
        let mut numbers = Vec::new();
        for _ in 0..361 {
            let advance = beam.advance(STEP);
            for offset in 0..advance.ticks {
                numbers.push(advance.first_tick + offset);
            }
        }
        assert_eq!(numbers, (1..=BEAM_TICKS).collect::<Vec<_>>());
        let ignites: Vec<u32> = numbers
            .into_iter()
            .filter(|index| BeamChannel::applies_ignite(*index))
            .collect();
        assert_eq!(ignites, [4, 8, 12, 16, 20, 24]);
    }

    #[test]
    fn the_beam_turns_at_most_seventy_five_degrees_a_second() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, Some(aimed_at(PI)));
        // A full second of turning.
        for _ in 0..60 {
            beam.turn(Vec3::ZERO, STEP);
        }
        assert!(
            (beam.yaw().abs() - BEAM_TURN_RATE).abs() < 1e-3,
            "turned {} rad in a second",
            beam.yaw()
        );
    }

    #[test]
    fn aim_directly_behind_takes_two_point_four_seconds() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, Some(aimed_at(PI - 0.001)));
        let mut seconds = 0.0;
        while wrap_angle(beam.target_yaw(Vec3::ZERO) - beam.yaw()).abs() > 1e-3 {
            beam.turn(Vec3::ZERO, STEP);
            seconds += STEP;
            assert!(seconds < 5.0, "never arrived");
        }
        assert!((seconds - 2.4).abs() < 0.05, "took {seconds} s");
    }

    #[test]
    fn it_takes_the_short_way_round() {
        let turned = turn_towards(0.1, -0.1 + TAU, 0.05);
        assert!(turned < 0.1, "went the long way: {turned}");
        let turned = turn_towards(3.0, -3.0, 0.1);
        assert!(turned > 3.0, "should cross the seam upwards: {turned}");
    }

    #[test]
    fn an_aim_underfoot_keeps_the_heading() {
        let mut beam = BeamChannel::idle();
        beam.start(1.0, Some(Vec3::new(0.05, 0.0, 0.05)));
        beam.turn(Vec3::ZERO, 1.0);
        assert!((beam.yaw() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reports_are_rate_limited_and_only_for_a_moved_target() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, Some(aimed_at(0.0)));
        // Nothing has changed: silent however long it runs.
        for _ in 0..120 {
            beam.advance(STEP);
            assert_eq!(beam.take_report(Vec3::ZERO), None);
        }
        // A new aim is reported once the interval allows, and then not again.
        beam.steer(aimed_at(1.0));
        let mut reports = 0;
        for _ in 0..60 {
            beam.advance(STEP);
            if beam.take_report(Vec3::ZERO).is_some() {
                reports += 1;
            }
        }
        assert_eq!(reports, 1);
        // Constant motion of the target is reported at most ten times a second.
        let mut reports = 0;
        for index in 0..120 {
            beam.advance(STEP);
            beam.steer(aimed_at(1.0 + index as f32 * 0.02));
            if beam.take_report(Vec3::ZERO).is_some() {
                reports += 1;
            }
        }
        assert!(reports <= 20, "{reports} reports in two seconds");
        assert!(reports >= 15, "{reports} reports is too few");
    }

    #[test]
    fn a_small_disagreement_is_kept_and_a_large_one_is_corrected() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, None);
        assert!(!beam.correct(0.05));
        assert_eq!(beam.yaw(), 0.0);
        assert!(beam.correct(1.0));
        assert_eq!(beam.yaw(), 1.0);
    }

    #[test]
    fn the_reasons_survive_the_wire() {
        for reason in BeamEnd::ALL {
            assert_eq!(BeamEnd::from_index(reason.index()), reason);
        }
        assert_eq!(BeamEnd::from_index(200), BeamEnd::Reset);
        assert!(BeamEnd::Completed.is_completed());
        assert!(!BeamEnd::Cancelled.is_completed());
    }

    #[test]
    fn the_shape_is_the_rectangle_the_tooltip_promises() {
        let mut beam = BeamChannel::idle();
        beam.start(0.0, None);
        let shape = beam.shape();
        // 10 m ahead is inside; 30 m ahead, 3 m to the side and behind are not.
        let at = |x: f32, z: f32| shape.contains(Vec3::ZERO, Vec3::new(x, 0.0, z), 0.0);
        assert!(at(0.0, -10.0));
        assert!(at(1.2, -21.0));
        assert!(!at(0.0, -30.0));
        assert!(!at(3.0, -10.0));
        assert!(!at(0.0, 5.0));
    }
}
