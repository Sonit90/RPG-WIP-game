//! The client's estimate of the server's clock.
//!
//! A countdown is a server timestamp. To show "3" at the same moment on three machines, each
//! client needs `server_now()`, not its own wall clock. It asks the server for the time a few
//! times (`server_time` procedure) and keeps the answer from the round trip with the least
//! delay — the classic NTP estimate, `offset = server − (sent + received) / 2`, whose error is
//! at most half that round trip.

use engine_realm::Micros;

/// The estimate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClockSync {
    best_round_trip: Option<Micros>,
    offset: Micros,
    samples: u32,
}

impl ClockSync {
    /// Records one exchange: sent and received on the local clock, `server` in between.
    pub fn record(&mut self, sent: Micros, received: Micros, server: Micros) {
        let round_trip = (received - sent).max(0);
        self.samples += 1;
        if self.best_round_trip.is_none_or(|best| round_trip <= best) {
            self.best_round_trip = Some(round_trip);
            self.offset = server - (sent + received) / 2;
        }
    }

    /// Whether any exchange has been recorded.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.samples > 0
    }

    /// The server's time when the local clock reads `local`.
    #[must_use]
    pub fn server_now(&self, local: Micros) -> Micros {
        local + self.offset
    }

    /// The best round trip seen, microseconds.
    #[must_use]
    pub fn round_trip(&self) -> Option<Micros> {
        self.best_round_trip
    }

    /// Forgets the estimate (after a reconnect the path may have changed).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The local wall clock, microseconds since the Unix epoch.
#[must_use]
pub fn local_now() -> Micros {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_micros()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_tightest_round_trip() {
        let mut sync = ClockSync::default();
        // Server is 5 s ahead. A slow exchange with an asymmetric delay first…
        sync.record(0, 400_000, 5_300_000);
        // …then a fast one.
        sync.record(1_000_000, 1_020_000, 6_010_000);
        assert_eq!(sync.round_trip(), Some(20_000));
        assert_eq!(sync.server_now(2_000_000), 7_000_000);
        // A slower one later does not replace it.
        sync.record(3_000_000, 3_500_000, 8_000_000);
        assert_eq!(sync.server_now(2_000_000), 7_000_000);
    }
}
