//! Conversions between table types and `engine-realm` rule types, and the small helpers every
//! reducer shares: config, time, rate limiting, notices, structured logging.

use engine_realm::RealmError;
use engine_realm::instance::{InstanceConfig, InstanceState};
use engine_realm::matchmaking::MatchmakingConfig;
use engine_realm::party::{PartyConfig, PartyState, Presence, ReadyResponse};
use engine_realm::rate_limit::{Operation, TokenBucket};
use engine_realm::{Micros, Role};
use spacetimedb::{Identity, ReducerContext, Table, Timestamp};

use crate::schema::*;

/// A timestamp as realm microseconds.
pub fn micros(timestamp: Timestamp) -> Micros {
    timestamp.to_micros_since_unix_epoch()
}

/// Realm microseconds as a timestamp.
pub fn timestamp(micros: Micros) -> Timestamp {
    Timestamp::from_micros_since_unix_epoch(micros)
}

/// The config row. `init` always writes it, so its absence is a broken database.
pub fn config(ctx: &ReducerContext) -> Result<RealmConfig, RealmError> {
    ctx.db
        .realm_config()
        .id()
        .find(0)
        .ok_or_else(|| RealmError::protocol("The realm is not configured."))
}

/// The matchmaking tunables from the config row.
pub fn matchmaking_config(config: &RealmConfig) -> MatchmakingConfig {
    MatchmakingConfig {
        min_tolerance: config.mm_min_tolerance,
        max_tolerance: config.mm_max_tolerance,
        expand_start: config.mm_expand_start_us,
        expand_end: config.mm_expand_end_us,
        ..MatchmakingConfig::default()
    }
}

/// The party tunables from the config row.
pub fn party_config(config: &RealmConfig) -> PartyConfig {
    PartyConfig {
        ready_check_timeout: config.ready_check_timeout_us,
    }
}

/// The instance tunables from the config row.
pub fn instance_config(config: &RealmConfig) -> InstanceConfig {
    InstanceConfig {
        reconnect_grace: config.reconnect_grace_us,
        finished_linger: config.finished_linger_us,
    }
}

/// The identity the persistence bridge connects as.
pub fn service_identity(config: &RealmConfig) -> Identity {
    Identity::from_claims(&config.issuer, &config.service_subject)
}

/// The caller's account, or an authentication error.
pub fn caller_account(ctx: &ReducerContext) -> Result<Account, RealmError> {
    ctx.db
        .account()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| {
            RealmError::new(
                engine_realm::ErrorCategory::Authentication,
                "Sign in to play online.",
            )
        })
}

/// Spends one call of `operation` for the caller, or refuses.
pub fn rate_limit(ctx: &ReducerContext, operation: Operation) -> Result<(), RealmError> {
    rate_limit_for(ctx, ctx.sender(), operation)
}

/// Spends one call of `operation` for `identity`, or refuses.
pub fn rate_limit_for(
    ctx: &ReducerContext,
    identity: Identity,
    operation: Operation,
) -> Result<(), RealmError> {
    let key = format!("{}:{}", identity.to_hex(), operation.key());
    let limit = operation.limit();
    let now = micros(ctx.timestamp);
    let mut bucket = match ctx.db.rate_limit().key().find(&key) {
        Some(row) => TokenBucket {
            tokens: row.tokens,
            refilled_at: micros(row.refilled_at),
        },
        None => TokenBucket::full(limit, now),
    };
    let allowed = bucket.try_take(limit, now);
    let row = RateLimit {
        key: key.clone(),
        tokens: bucket.tokens,
        refilled_at: ctx.timestamp,
    };
    if ctx.db.rate_limit().key().find(&key).is_some() {
        ctx.db.rate_limit().key().update(row);
    } else {
        ctx.db.rate_limit().insert(row);
    }
    if allowed {
        Ok(())
    } else {
        log::warn!(
            "event=rate_limited identity={} operation={}",
            identity.to_abbreviated_hex(),
            operation.key()
        );
        Err(RealmError::rate_limited())
    }
}

/// Leaves a notice for one player.
pub fn notify(
    ctx: &ReducerContext,
    identity: Identity,
    code: &str,
    level: NoticeLevel,
    message: impl Into<String>,
) {
    ctx.db.player_notice().insert(PlayerNotice {
        id: 0,
        identity,
        code: code.to_owned(),
        level,
        message: message.into(),
        created_at: ctx.timestamp,
    });
}

impl From<Role> for RoleTag {
    fn from(role: Role) -> Self {
        match role {
            Role::Tank => Self::Tank,
            Role::Healer => Self::Healer,
            Role::Dps => Self::Dps,
        }
    }
}

impl From<RoleTag> for Role {
    fn from(role: RoleTag) -> Self {
        match role {
            RoleTag::Tank => Self::Tank,
            RoleTag::Healer => Self::Healer,
            RoleTag::Dps => Self::Dps,
        }
    }
}

impl From<PartyStateTag> for PartyState {
    fn from(state: PartyStateTag) -> Self {
        match state {
            PartyStateTag::ReadyCheck => Self::ReadyCheck,
            PartyStateTag::Ready => Self::Ready,
            PartyStateTag::Starting => Self::Starting,
            PartyStateTag::InDungeon => Self::InDungeon,
            PartyStateTag::Completed => Self::Completed,
            PartyStateTag::Failed => Self::Failed,
            PartyStateTag::Disbanded => Self::Disbanded,
            PartyStateTag::ChoosingKey => Self::ChoosingKey,
        }
    }
}

impl From<PartyState> for PartyStateTag {
    fn from(state: PartyState) -> Self {
        match state {
            PartyState::ReadyCheck => Self::ReadyCheck,
            PartyState::Ready => Self::Ready,
            PartyState::Starting => Self::Starting,
            PartyState::InDungeon => Self::InDungeon,
            PartyState::Completed => Self::Completed,
            PartyState::Failed => Self::Failed,
            PartyState::Disbanded => Self::Disbanded,
            PartyState::ChoosingKey => Self::ChoosingKey,
        }
    }
}

impl From<ReadyTag> for ReadyResponse {
    fn from(tag: ReadyTag) -> Self {
        match tag {
            ReadyTag::Pending => Self::Pending,
            ReadyTag::Accepted => Self::Accepted,
            ReadyTag::Declined => Self::Declined,
        }
    }
}

impl From<ReadyResponse> for ReadyTag {
    fn from(response: ReadyResponse) -> Self {
        match response {
            ReadyResponse::Pending => Self::Pending,
            ReadyResponse::Accepted => Self::Accepted,
            ReadyResponse::Declined => Self::Declined,
        }
    }
}

impl From<PresenceTag> for Presence {
    fn from(tag: PresenceTag) -> Self {
        match tag {
            PresenceTag::Present => Self::Present,
            PresenceTag::TemporarilyDisconnected => Self::TemporarilyDisconnected,
        }
    }
}

impl From<InstanceStateTag> for InstanceState {
    fn from(tag: InstanceStateTag) -> Self {
        match tag {
            InstanceStateTag::Created => Self::Created,
            InstanceStateTag::Loading => Self::Loading,
            InstanceStateTag::Ready => Self::Ready,
            InstanceStateTag::Countdown => Self::Countdown,
            InstanceStateTag::Active => Self::Active,
            InstanceStateTag::Completed => Self::Completed,
            InstanceStateTag::Failed => Self::Failed,
            InstanceStateTag::CleaningUp => Self::CleaningUp,
            InstanceStateTag::Destroyed => Self::Destroyed,
        }
    }
}

impl From<InstanceState> for InstanceStateTag {
    fn from(state: InstanceState) -> Self {
        match state {
            InstanceState::Created => Self::Created,
            InstanceState::Loading => Self::Loading,
            InstanceState::Ready => Self::Ready,
            InstanceState::Countdown => Self::Countdown,
            InstanceState::Active => Self::Active,
            InstanceState::Completed => Self::Completed,
            InstanceState::Failed => Self::Failed,
            InstanceState::CleaningUp => Self::CleaningUp,
            InstanceState::Destroyed => Self::Destroyed,
        }
    }
}

impl From<engine_core::math::Vec3> for V3 {
    fn from(v: engine_core::math::Vec3) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

impl From<V3> for engine_core::math::Vec3 {
    fn from(v: V3) -> Self {
        Self::new(v.x, v.y, v.z)
    }
}
