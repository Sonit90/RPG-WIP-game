//! Token buckets for everything a client can call.
//!
//! Each `(caller, operation)` pair has a bucket: a capacity (the burst a human could produce)
//! and a refill rate (the sustained rate a human could keep up). The module stores buckets in
//! a private table and checks one before doing any work, so a client flooding `queue_join`
//! costs one row read per refused call and never a persistence request.

use crate::Micros;

/// Something a client can ask for, each with its own limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Operation {
    /// Joining or leaving the queue, or selecting a character.
    Queue,
    /// Answering a ready check.
    ReadyCheck,
    /// Leaving a party.
    Party,
    /// Leader-only operations: kick, transfer, start.
    Leader,
    /// Movement input messages.
    Movement,
    /// Clock synchronisation pings.
    TimeSync,
}

impl Operation {
    /// Every operation.
    pub const ALL: [Self; 6] = [
        Self::Queue,
        Self::ReadyCheck,
        Self::Party,
        Self::Leader,
        Self::Movement,
        Self::TimeSync,
    ];

    /// The stable key the module stores buckets under.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Queue => "queue",
            Self::ReadyCheck => "ready_check",
            Self::Party => "party",
            Self::Leader => "leader",
            Self::Movement => "movement",
            Self::TimeSync => "time_sync",
        }
    }

    /// Its limit.
    #[must_use]
    pub fn limit(self) -> Limit {
        match self {
            Self::Queue => Limit::new(5.0, 1.0),
            Self::ReadyCheck => Limit::new(4.0, 1.0),
            Self::Party => Limit::new(5.0, 1.0),
            Self::Leader => Limit::new(5.0, 1.0),
            // A client sends ~30 messages a second; double that is the ceiling.
            Self::Movement => Limit::new(60.0, 60.0),
            Self::TimeSync => Limit::new(10.0, 2.0),
        }
    }
}

/// A bucket's shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    /// The most calls that may be banked.
    pub capacity: f32,
    /// Calls earned per second.
    pub per_second: f32,
}

impl Limit {
    /// A bucket of `capacity` refilled at `per_second`.
    #[must_use]
    pub const fn new(capacity: f32, per_second: f32) -> Self {
        Self {
            capacity,
            per_second,
        }
    }
}

/// One bucket's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenBucket {
    /// Calls that may still be made.
    pub tokens: f32,
    /// When it was last refilled.
    pub refilled_at: Micros,
}

impl TokenBucket {
    /// A full bucket at `now`.
    #[must_use]
    pub fn full(limit: Limit, now: Micros) -> Self {
        Self {
            tokens: limit.capacity,
            refilled_at: now,
        }
    }

    /// Refills, then spends one call if there is one. Returns whether the call is allowed.
    pub fn try_take(&mut self, limit: Limit, now: Micros) -> bool {
        let elapsed = (now - self.refilled_at).clamp(0, 3_600_000_000);
        #[allow(clippy::cast_precision_loss, reason = "elapsed is clamped to an hour")]
        let earned = elapsed as f32 * 1e-6 * limit.per_second;
        self.tokens = (self.tokens + earned).min(limit.capacity);
        self.refilled_at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SECOND;

    #[test]
    fn bursts_then_refuses_then_recovers() {
        let limit = Operation::Queue.limit();
        let mut bucket = TokenBucket::full(limit, 0);
        for _ in 0..5 {
            assert!(bucket.try_take(limit, 0));
        }
        assert!(!bucket.try_take(limit, 0));
        assert!(!bucket.try_take(limit, SECOND / 2));
        assert!(bucket.try_take(limit, SECOND + SECOND / 2));
    }

    #[test]
    fn a_flood_is_held_to_the_rate() {
        let limit = Operation::Leader.limit();
        let mut bucket = TokenBucket::full(limit, 0);
        let allowed = (0..10_000)
            .filter(|i| bucket.try_take(limit, i * SECOND / 1000))
            .count();
        // Ten seconds at one per second, plus the burst.
        assert!(allowed <= 16, "allowed {allowed}");
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<&str> = Operation::ALL.iter().map(|op| op.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), Operation::ALL.len());
    }
}
