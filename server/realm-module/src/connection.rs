//! Who may connect, and what connecting and disconnecting do to a player's realm state.
//!
//! ```text
//! token ─► SpacetimeDB verifies signature via the issuer's JWKS ─► client_connected
//!                                                                   │ issuer + audience?
//!                                                                   ▼
//!                                   account row (by identity) ── connections += 1
//!                                   party / instance presence ── Present again
//! ```
//!
//! A disconnect never removes anyone. It marks presence and starts the clock; the janitor
//! (`instance.rs`) decides, after the configured grace, what an absence costs. A ready check
//! is the one exception: a disconnected player cannot confirm, so an open check is cancelled
//! at once rather than left to time out.

use engine_realm::{ErrorCategory, RealmError};
use spacetimedb::{ReducerContext, Table};

use crate::schema::*;
use crate::util::{caller_account, config, notify, service_identity};

#[spacetimedb::reducer(init)]
pub fn init(ctx: &ReducerContext) {
    let defaults = engine_realm::matchmaking::MatchmakingConfig::default();
    let party = engine_realm::party::PartyConfig::default();
    let instance = engine_realm::instance::InstanceConfig::default();
    ctx.db.realm_config().insert(RealmConfig {
        id: 0,
        owner: ctx.sender(),
        // Development defaults: the realm API on this machine. A deployment overrides all
        // three with `admin_configure` straight after publishing.
        issuer: "http://127.0.0.1:8080".into(),
        audience: "mygame-realm".into(),
        service_subject: "service:realm-bridge".into(),
        mm_min_tolerance: defaults.min_tolerance,
        mm_max_tolerance: defaults.max_tolerance,
        mm_expand_start_us: defaults.expand_start,
        mm_expand_end_us: defaults.expand_end,
        ready_check_timeout_us: party.ready_check_timeout,
        reconnect_grace_us: instance.reconnect_grace,
        queue_disconnect_grace_us: 60 * engine_realm::SECOND,
        finished_linger_us: instance.finished_linger,
        notice_ttl_us: 120 * engine_realm::SECOND,
    });
    crate::matchmaking::schedule(ctx);
    crate::instance::schedule(ctx);
    log::info!("event=realm_initialized owner={}", ctx.sender().to_abbreviated_hex());
}

/// Optional claims the realm reads beyond `iss`, `sub` and `aud`.
#[derive(serde::Deserialize, Default)]
struct ExtraClaims {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
}

#[spacetimedb::reducer(client_connected)]
pub fn client_connected(ctx: &ReducerContext) -> Result<(), String> {
    let config = config(ctx)?;
    if ctx.sender() == config.owner {
        return Ok(());
    }
    let jwt = ctx.sender_auth().jwt().ok_or_else(|| {
        RealmError::new(ErrorCategory::Authentication, "Sign in to play online.").to_wire()
    })?;
    if jwt.issuer() != config.issuer {
        log::warn!("event=auth_rejected reason=issuer issuer={}", jwt.issuer());
        return Err(RealmError::new(
            ErrorCategory::Authentication,
            "This sign-in is not valid for this realm.",
        )
        .to_wire());
    }
    if !jwt.audience().contains(&config.audience) {
        log::warn!("event=auth_rejected reason=audience");
        return Err(RealmError::new(
            ErrorCategory::Authentication,
            "This sign-in is not valid for this realm.",
        )
        .to_wire());
    }
    if ctx.sender() == service_identity(&config) {
        // The persistence bridge: no account, no presence.
        return Ok(());
    }

    let account_id = jwt.subject().to_owned();
    let extra: ExtraClaims = serde_json::from_str(jwt.raw_payload()).unwrap_or_default();
    let display_name = extra
        .preferred_username
        .or(extra.name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Adventurer".to_owned());

    let previous = ctx.db.account().identity().find(ctx.sender());
    let reconnecting = previous
        .as_ref()
        .is_some_and(|account| account.disconnected_at.is_some());
    let account = Account {
        identity: ctx.sender(),
        account_id: account_id.clone(),
        display_name,
        connections: previous.as_ref().map_or(0, |a| a.connections) + 1,
        disconnected_at: None,
        last_seen: ctx.timestamp,
    };
    if previous.is_some() {
        ctx.db.account().identity().update(account);
    } else {
        ctx.db.account().try_insert(account).map_err(|error| {
            log::error!("event=account_insert_failed account_id={account_id} error={error}");
            RealmError::protocol("Could not register this account with the realm.").to_wire()
        })?;
    }
    if let Some(connection_id) = ctx.connection_id() {
        ctx.db.connection().insert(Connection {
            connection_id,
            identity: ctx.sender(),
            connected_at: ctx.timestamp,
        });
    }

    // Back in the party and the instance.
    if let Some(member) = ctx.db.party_member().identity().find(ctx.sender())
        && member.presence != PresenceTag::Present
    {
        ctx.db.party_member().identity().update(PartyMember {
            presence: PresenceTag::Present,
            disconnected_at: None,
            ..member
        });
    }
    if let Some(player) = ctx.db.instance_player().identity().find(ctx.sender())
        && player.presence != PresenceTag::Present
    {
        ctx.db.instance_player().identity().update(InstancePlayer {
            presence: PresenceTag::Present,
            // A fresh budget: the time away does not bank frames.
            budget_tokens: 0.0,
            budget_refilled_at: ctx.timestamp,
            last_input_at: ctx.timestamp,
            ..player
        });
        log::info!(
            "event=player_reconnected account_id={account_id} instance_id={}",
            player.instance_id
        );
    }
    if reconnecting {
        notify(
            ctx,
            ctx.sender(),
            "reconnected",
            NoticeLevel::Info,
            "Reconnected to the realm.",
        );
    }
    log::info!(
        "event=connected account_id={account_id} reconnect={reconnecting} connections={}",
        previous.map_or(0, |a| a.connections) + 1
    );
    Ok(())
}

#[spacetimedb::reducer(client_disconnected)]
pub fn client_disconnected(ctx: &ReducerContext) {
    if let Some(connection_id) = ctx.connection_id() {
        ctx.db.connection().connection_id().delete(connection_id);
    }
    let Ok(account) = caller_account(ctx) else {
        return;
    };
    let connections = account.connections.saturating_sub(1);
    let gone = connections == 0;
    let account_id = account.account_id.clone();
    ctx.db.account().identity().update(Account {
        connections,
        disconnected_at: if gone { Some(ctx.timestamp) } else { None },
        last_seen: ctx.timestamp,
        ..account
    });
    if !gone {
        // Another connection (another machine, a reconnect racing this close) is still open.
        return;
    }
    log::info!("event=disconnected account_id={account_id}");

    if let Some(member) = ctx.db.party_member().identity().find(ctx.sender()) {
        let party_id = member.party_id;
        ctx.db.party_member().identity().update(PartyMember {
            presence: PresenceTag::TemporarilyDisconnected,
            disconnected_at: Some(ctx.timestamp),
            ..member
        });
        if let Some(party) = ctx.db.party().id().find(party_id)
            && party.state == PartyStateTag::ReadyCheck
        {
            crate::party::resolve_ready_check(ctx, party_id);
        }
    }
    if let Some(player) = ctx.db.instance_player().identity().find(ctx.sender()) {
        log::info!(
            "event=player_disconnected account_id={account_id} instance_id={}",
            player.instance_id
        );
        ctx.db.instance_player().identity().update(InstancePlayer {
            presence: PresenceTag::TemporarilyDisconnected,
            ..player
        });
    }
}
