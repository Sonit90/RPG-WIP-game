//! In-memory token buckets for the endpoints a password guesser or a script would hammer.
//!
//! One process, one map: enough for a single API instance, which is the deployment this
//! project runs. Behind several instances, move the buckets to the shared database (or a
//! store built for it) — the call sites do not change.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use generic_auth_core::AppError;

/// A bucket's shape: how many calls may be banked and how fast they come back.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub capacity: f64,
    pub per_second: f64,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Every throttle the API applies.
#[derive(Default)]
pub struct Limits {
    buckets: Mutex<HashMap<String, Bucket>>,
    /// Multiplies every capacity and rate. 1 in production; development raises it so a dozen
    /// local bots registering from one address in a second are not mistaken for an attack.
    scale: f64,
}

/// Login attempts per email: five, then one every twelve seconds.
pub const LOGIN_PER_EMAIL: Rule = Rule {
    capacity: 5.0,
    per_second: 1.0 / 12.0,
};
/// Login attempts per client address.
pub const LOGIN_PER_ADDRESS: Rule = Rule {
    capacity: 20.0,
    per_second: 0.5,
};
/// Registrations per client address.
pub const REGISTER_PER_ADDRESS: Rule = Rule {
    capacity: 3.0,
    per_second: 1.0 / 60.0,
};
/// Token refreshes per client address.
pub const REFRESH_PER_ADDRESS: Rule = Rule {
    capacity: 30.0,
    per_second: 1.0,
};
/// Queue requests per account.
pub const QUEUE_PER_ACCOUNT: Rule = Rule {
    capacity: 5.0,
    per_second: 1.0,
};
/// Character writes (moves, talents) per account.
pub const CHARACTER_WRITES_PER_ACCOUNT: Rule = Rule {
    capacity: 20.0,
    per_second: 4.0,
};
/// History and leaderboard reads per account: paging through is fine, scraping is not.
pub const READS_PER_ACCOUNT: Rule = Rule {
    capacity: 20.0,
    per_second: 2.0,
};
/// Settings saves per account.
pub const SETTINGS_PER_ACCOUNT: Rule = Rule {
    capacity: 5.0,
    per_second: 0.2,
};

impl Limits {
    /// Limits multiplied by `scale` (at least 1).
    pub fn scaled(scale: f64) -> Self {
        Self {
            buckets: Mutex::default(),
            scale: scale.max(1.0),
        }
    }

    /// Spends one call of `rule` under `key`, or refuses with 429.
    pub fn check(&self, key: &str, rule: Rule) -> Result<(), AppError> {
        self.check_at(key, rule, Instant::now())
    }

    fn check_at(&self, key: &str, rule: Rule, now: Instant) -> Result<(), AppError> {
        let scale = if self.scale >= 1.0 { self.scale } else { 1.0 };
        let rule = Rule {
            capacity: rule.capacity * scale,
            per_second: rule.per_second * scale,
        };
        let mut buckets = self
            .buckets
            .lock()
            .map_err(|_| AppError::internal("rate limiter poisoned"))?;
        if buckets.len() > 100_000 {
            // A flood of distinct keys: forget idle ones rather than grow without bound.
            buckets.retain(|_, b| now.duration_since(b.at).as_secs() < 600);
        }
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: rule.capacity,
            at: now,
        });
        let earned = now.duration_since(bucket.at).as_secs_f64() * rule.per_second;
        bucket.tokens = (bucket.tokens + earned).min(rule.capacity);
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            tracing::warn!(key = %key.split(':').next().unwrap_or(""), "rate limited");
            Err(AppError::TooManyRequests(
                "Too many attempts. Wait a moment and try again.".into(),
            ))
        }
    }
}

/// The client's address as the throttles see it.
///
/// `X-Forwarded-For` is honoured only when the socket peer is a loopback or private address,
/// meaning the reverse proxy in front of this service. A client connecting directly cannot
/// choose its own throttle key by sending the header.
pub fn client_address(
    headers: &axum::http::HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> String {
    let behind_proxy = peer.is_none_or(|p| match p.ip() {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        std::net::IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
    });
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match (behind_proxy, forwarded) {
        (true, Some(forwarded)) => forwarded,
        _ => peer
            .map(|p| p.ip().to_string())
            .unwrap_or_else(|| "unknown".into()),
    }
}

/// The caller's address, as an extractor.
pub struct ClientAddress(pub String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ClientAddress {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0);
        Ok(Self(client_address(&parts.headers, peer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bursts_then_refuses_then_recovers() {
        let limits = Limits::default();
        let start = Instant::now();
        for _ in 0..5 {
            assert!(limits
                .check_at("login:a@b.c", LOGIN_PER_EMAIL, start)
                .is_ok());
        }
        assert!(limits
            .check_at("login:a@b.c", LOGIN_PER_EMAIL, start)
            .is_err());
        // Other keys are unaffected.
        assert!(limits
            .check_at("login:x@y.z", LOGIN_PER_EMAIL, start)
            .is_ok());
        assert!(limits
            .check_at(
                "login:a@b.c",
                LOGIN_PER_EMAIL,
                start + Duration::from_secs(13)
            )
            .is_ok());
    }

    #[test]
    fn forwarded_for_wins_over_the_socket() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.9, 10.0.0.1".parse().unwrap());
        assert_eq!(client_address(&headers, None), "203.0.113.9");
        let proxy: std::net::SocketAddr = "10.0.0.1:443".parse().unwrap();
        assert_eq!(client_address(&headers, Some(proxy)), "203.0.113.9");
        // A direct client cannot pick its own key.
        let direct: std::net::SocketAddr = "198.51.100.7:5555".parse().unwrap();
        assert_eq!(client_address(&headers, Some(direct)), "198.51.100.7");
        assert_eq!(
            client_address(&axum::http::HeaderMap::new(), None),
            "unknown"
        );
    }
}
