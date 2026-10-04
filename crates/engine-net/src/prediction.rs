//! Client-side prediction and reconciliation for the local player.
//!
//! ```text
//! tick:   input ─► InputFrame{seq} ─► apply locally (shown now) ─► outbox ─► server
//! server: row { position, velocity, last_seq } arrives ~RTT later
//!         └► rewind to the row, replay every frame with seq > last_seq ─► new prediction
//!            the visual error (old prediction − new) is kept as an offset and decays
//! ```
//!
//! The server never sees a position from the client, and the client never waits for the
//! server to see its own feet move. When the two agree — which is always, unless something
//! was lost, refused or collided differently — reconciliation reproduces the same position
//! and the correction is zero.

use std::collections::VecDeque;

use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;
use engine_game::movement::Body;
use engine_realm::movement::{InputFrame, MAX_FRAMES_PER_MESSAGE, MovementRules, apply_input};

/// How many unacknowledged frames are kept. Four seconds: past that the server is not
/// answering and replaying more would only make the eventual correction later.
const MAX_PENDING: usize = 240;

/// A correction larger than this is a teleport (a respawn, a void rescue): snap, don't slide.
const SNAP_DISTANCE: f32 = 4.0;

/// Seconds for a correction to fade to a third of its size.
const SMOOTHING_SECONDS: f32 = 0.1;

/// The local player's predicted movement.
#[derive(Debug, Default)]
pub struct Prediction {
    pending: VecDeque<InputFrame>,
    outbox: Vec<InputFrame>,
    next_seq: u32,
    last_acked: u32,
    /// Where the drawn character is relative to the predicted body: the decaying remains of
    /// the last correction.
    visual_offset: Vec3,
}

impl Prediction {
    /// A fresh prediction. Sequence numbers start after `last_acked`, which is what the
    /// server's row says when the client (re)joins an instance.
    #[must_use]
    pub fn new(last_acked: u32) -> Self {
        Self {
            next_seq: last_acked.wrapping_add(1),
            last_acked,
            ..Self::default()
        }
    }

    /// Records this tick's intent and returns the frame to apply locally.
    pub fn record(
        &mut self,
        forward: f32,
        right: f32,
        yaw: f32,
        face_camera: bool,
        jump: bool,
    ) -> InputFrame {
        let frame = InputFrame {
            seq: self.next_seq,
            forward,
            right,
            yaw,
            face_camera,
            jump,
        };
        self.next_seq = self.next_seq.wrapping_add(1);
        self.pending.push_back(frame);
        while self.pending.len() > MAX_PENDING {
            self.pending.pop_front();
        }
        self.outbox.push(frame);
        frame
    }

    /// Frames to send now, at most one message's worth; the rest wait for the next send.
    pub fn take_outbox(&mut self) -> Vec<InputFrame> {
        let count = self.outbox.len().min(MAX_FRAMES_PER_MESSAGE);
        self.outbox.drain(..count).collect()
    }

    /// Frames recorded but not yet sent.
    #[must_use]
    pub fn outbox_len(&self) -> usize {
        self.outbox.len()
    }

    /// Frames the server has not acknowledged.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// The last frame the server applied.
    #[must_use]
    pub fn last_acked(&self) -> u32 {
        self.last_acked
    }

    /// Rewinds to the server's body at `acked`, replays everything after it, and returns the
    /// new predicted body. `current` is the body as predicted before this update; the jump
    /// between the two is folded into the visual offset.
    pub fn reconcile(
        &mut self,
        current: &Body,
        authoritative: Body,
        acked: u32,
        rules: &MovementRules,
        world: &CollisionWorld,
        buffers: &mut QueryBuffers,
    ) -> Body {
        // Older or equal acknowledgements change nothing we have not already replayed.
        if acked < self.last_acked {
            return *current;
        }
        self.last_acked = acked;
        while self
            .pending
            .front()
            .is_some_and(|frame| frame.seq <= acked)
        {
            self.pending.pop_front();
        }
        let mut body = authoritative;
        for frame in &self.pending {
            apply_input(&mut body, frame, rules, world, buffers);
        }
        let error = current.position - body.position;
        if error.length() > SNAP_DISTANCE {
            self.visual_offset = Vec3::ZERO;
        } else {
            self.visual_offset += error;
        }
        body
    }

    /// Decays the visual offset by `seconds` and returns it.
    pub fn smooth(&mut self, seconds: f32) -> Vec3 {
        let keep = (-seconds.max(0.0) / SMOOTHING_SECONDS).exp();
        self.visual_offset *= keep;
        if self.visual_offset.length() < 1e-4 {
            self.visual_offset = Vec3::ZERO;
        }
        self.visual_offset
    }

    /// The current visual offset.
    #[must_use]
    pub fn visual_offset(&self) -> Vec3 {
        self.visual_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_environment::EnvironmentScene;
    use engine_realm::movement::LevelGeometry;

    fn level() -> LevelGeometry {
        LevelGeometry::build(&EnvironmentScene::empty("flat"), &[])
    }

    #[test]
    fn agreeing_server_produces_no_correction() {
        let level = level();
        let mut buffers = QueryBuffers::new();
        let mut client = Prediction::new(0);
        let mut predicted = Body::at(Vec3::ZERO);
        let mut server = Body::at(Vec3::ZERO);
        let mut server_seq = 0;
        for tick in 0..120 {
            let frame = client.record(1.0, 0.3, 0.2, false, tick == 10);
            apply_input(&mut predicted, &frame, &level.rules, &level.world, &mut buffers);
            // The server applies everything sent, ten ticks behind.
            if tick % 2 == 1 {
                for frame in client.take_outbox() {
                    apply_input(&mut server, &frame, &level.rules, &level.world, &mut buffers);
                    server_seq = frame.seq;
                }
            }
            if tick >= 10 && tick % 5 == 0 {
                predicted = client.reconcile(
                    &predicted,
                    server,
                    server_seq,
                    &level.rules,
                    &level.world,
                    &mut buffers,
                );
            }
        }
        assert!(client.visual_offset().length() < 1e-3);
    }

    #[test]
    fn a_refused_frame_is_corrected_towards_the_server() {
        let level = level();
        let mut buffers = QueryBuffers::new();
        let mut client = Prediction::new(0);
        let mut predicted = Body::at(Vec3::ZERO);
        for _ in 0..30 {
            let frame = client.record(1.0, 0.0, 0.0, false, false);
            apply_input(&mut predicted, &frame, &level.rules, &level.world, &mut buffers);
        }
        // The server applied nothing (say, the barrier was still closed) yet acknowledged.
        let server = Body::at(Vec3::ZERO);
        let corrected = client.reconcile(
            &predicted,
            server,
            30,
            &level.rules,
            &level.world,
            &mut buffers,
        );
        assert!(corrected.position.length() < 1e-4);
        assert_eq!(client.pending_len(), 0);
        // The drawn character slides rather than jumps.
        assert!(client.visual_offset().length() > 0.5);
        for _ in 0..60 {
            client.smooth(1.0 / 60.0);
        }
        assert!(client.visual_offset().length() < 0.01);
    }

    #[test]
    fn outbox_is_sent_in_bounded_messages() {
        let mut client = Prediction::new(41);
        for _ in 0..40 {
            client.record(0.0, 0.0, 0.0, false, false);
        }
        let first = client.take_outbox();
        assert_eq!(first.len(), MAX_FRAMES_PER_MESSAGE);
        assert_eq!(first[0].seq, 42);
        assert_eq!(client.outbox_len(), 40 - MAX_FRAMES_PER_MESSAGE);
    }

    #[test]
    fn stale_acknowledgements_are_ignored() {
        let level = level();
        let mut buffers = QueryBuffers::new();
        let mut client = Prediction::new(10);
        let body = Body::at(Vec3::new(1.0, 0.0, 0.0));
        let kept = client.reconcile(&body, Body::at(Vec3::ZERO), 5, &level.rules, &level.world, &mut buffers);
        assert_eq!(kept.position, body.position);
    }
}
