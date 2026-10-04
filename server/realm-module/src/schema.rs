//! Every table and every type a table or a client call carries.
//!
//! All tables are private: clients see rows only through the views in `views.rs`. The tag
//! enums mirror `engine-realm`'s rule enums one for one (`util.rs` converts), so the rules
//! never depend on SpacetimeDB types and the wire never carries a string where an enum fits.

use spacetimedb::{Identity, SpacetimeType, Timestamp};

/// A point or a vector.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Default)]
pub struct V3 {
    /// X.
    pub x: f32,
    /// Y.
    pub y: f32,
    /// Z.
    pub z: f32,
}

/// Tank, Healer or DPS.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleTag {
    /// Tank.
    Tank,
    /// Healer.
    Healer,
    /// DPS.
    Dps,
}

/// `engine_realm::party::PartyState`.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyStateTag {
    /// Waiting for every member to accept.
    ReadyCheck,
    /// Waiting for the leader to start.
    Ready,
    /// Counting down.
    Starting,
    /// In the dungeon.
    InDungeon,
    /// Finished successfully.
    Completed,
    /// Failed.
    Failed,
    /// Gone.
    Disbanded,
}

/// `engine_realm::party::ReadyResponse`.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyTag {
    /// Not answered.
    Pending,
    /// Accepted.
    Accepted,
    /// Declined.
    Declined,
}

/// `engine_realm::party::Presence`.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceTag {
    /// Connected.
    Present,
    /// Disconnected; their place is held for the reconnect grace period.
    TemporarilyDisconnected,
}

/// `engine_realm::instance::InstanceState`.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStateTag {
    /// Rows exist.
    Created,
    /// Being placed.
    Loading,
    /// Placed behind the barrier.
    Ready,
    /// Counting down.
    Countdown,
    /// Barrier open, run under way.
    Active,
    /// Completed.
    Completed,
    /// Failed.
    Failed,
    /// Being torn down.
    CleaningUp,
    /// Gone.
    Destroyed,
}

/// Where a queue ticket is.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketStatus {
    /// Waiting for a party.
    Searching,
    /// Offered a party; answering a ready check.
    Matched,
}

/// How a notice should be shown.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Neutral information.
    Info,
    /// Something the player should notice.
    Warning,
    /// Something went wrong.
    Error,
}

/// One tick of movement intent, as a client sends it.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq)]
pub struct NetInputFrame {
    /// Increases by one per tick.
    pub seq: u32,
    /// Forward axis, [-1, 1].
    pub forward: f32,
    /// Right axis, [-1, 1].
    pub right: f32,
    /// Camera yaw, radians.
    pub yaw: f32,
    /// Camera steering the character.
    pub face_camera: bool,
    /// Jump this tick.
    pub jump: bool,
}

/// The module's tunables. One row, id 0.
#[spacetimedb::table(accessor = realm_config)]
#[derive(Debug, Clone)]
pub struct RealmConfig {
    /// Always 0.
    #[primary_key]
    pub id: u8,
    /// Who published the module. May call `admin_*`.
    pub owner: Identity,
    /// The OIDC issuer whose tokens are accepted.
    pub issuer: String,
    /// The audience a token must carry.
    pub audience: String,
    /// The `sub` the persistence bridge's token carries.
    pub service_subject: String,
    /// Matchmaking: starting tolerance.
    pub mm_min_tolerance: f64,
    /// Matchmaking: widest tolerance.
    pub mm_max_tolerance: f64,
    /// Matchmaking: when widening starts, microseconds of waiting.
    pub mm_expand_start_us: i64,
    /// Matchmaking: when widening ends.
    pub mm_expand_end_us: i64,
    /// Ready check timeout.
    pub ready_check_timeout_us: i64,
    /// How long a disconnected member's place in a party or instance is held.
    pub reconnect_grace_us: i64,
    /// How long a disconnected player's queue ticket is held.
    pub queue_disconnect_grace_us: i64,
    /// How long a finished instance lingers.
    pub finished_linger_us: i64,
    /// How long a notice is kept.
    pub notice_ttl_us: i64,
}

/// A player account, as the realm knows it. Created on first connection.
///
/// Keyed by SpacetimeDB identity, which is derived from the token's issuer and subject — so
/// the same account on another machine, or after a reconnect, is the same row.
#[spacetimedb::table(accessor = account)]
#[derive(Debug, Clone)]
pub struct Account {
    /// The SpacetimeDB identity.
    #[primary_key]
    pub identity: Identity,
    /// The persistence API's account id (the token's `sub`).
    #[unique]
    pub account_id: String,
    /// What other players see.
    pub display_name: String,
    /// Open connections. Zero means disconnected.
    pub connections: u32,
    /// When the last connection closed, while there are none.
    pub disconnected_at: Option<Timestamp>,
    /// Last activity.
    pub last_seen: Timestamp,
}

/// One open connection. An account can have several (two machines, a reconnect racing the
/// old socket's close).
#[spacetimedb::table(accessor = connection)]
#[derive(Debug, Clone)]
pub struct Connection {
    /// The connection.
    #[primary_key]
    pub connection_id: spacetimedb::ConnectionId,
    /// Whose.
    #[index(btree)]
    pub identity: Identity,
    /// When it opened.
    pub connected_at: Timestamp,
}

/// A character waiting for a party. One per account at most.
#[spacetimedb::table(accessor = queue_ticket)]
#[derive(Debug, Clone)]
pub struct QueueTicket {
    /// Ticket id.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// The player. Unique: one ticket per account, whatever character.
    #[unique]
    pub identity: Identity,
    /// Their account id.
    pub account_id: String,
    /// The persistent character.
    pub character_id: String,
    /// Its name.
    pub character_name: String,
    /// Its catalog definition.
    pub definition_id: String,
    /// Its level, from the persistence API.
    pub level: u32,
    /// Its gear score, from the persistence API. Never from a client.
    pub gear_score: u32,
    /// Which dungeon.
    #[index(btree)]
    pub dungeon_id: String,
    /// As which role.
    pub role: RoleTag,
    /// When it was queued. Survives a cancelled ready check.
    pub enqueued_at: Timestamp,
    /// Searching or in a ready check.
    pub status: TicketStatus,
    /// The party offered, while matched.
    pub party_id: Option<u64>,
}

/// A party.
#[spacetimedb::table(accessor = party)]
#[derive(Debug, Clone)]
pub struct Party {
    /// Party id.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// The dungeon it formed for.
    pub dungeon_id: String,
    /// Where it is in its life.
    pub state: PartyStateTag,
    /// The leader, once chosen.
    pub leader: Option<Identity>,
    /// When the ready check expires.
    pub ready_deadline: Timestamp,
    /// Its instance, once started.
    pub instance_id: Option<u64>,
    /// When it formed.
    pub created_at: Timestamp,
}

/// A party member. Keyed by identity: a player is in one party at most.
#[spacetimedb::table(accessor = party_member)]
#[derive(Debug, Clone)]
pub struct PartyMember {
    /// The player.
    #[primary_key]
    pub identity: Identity,
    /// Which party.
    #[index(btree)]
    pub party_id: u64,
    /// Their account id.
    pub account_id: String,
    /// Their display name.
    pub display_name: String,
    /// The character they brought.
    pub character_id: String,
    /// Its name.
    pub character_name: String,
    /// Its catalog definition.
    pub definition_id: String,
    /// Its role.
    pub role: RoleTag,
    /// Its level.
    pub level: u32,
    /// Its gear score.
    pub gear_score: u32,
    /// Their ready-check answer.
    pub ready: ReadyTag,
    /// Connected or not.
    pub presence: PresenceTag,
    /// When they disconnected, while disconnected.
    pub disconnected_at: Option<Timestamp>,
    /// When they originally queued, so a cancelled check re-queues them in place.
    pub enqueued_at: Timestamp,
    /// Party order: decides the spawn point.
    pub slot: u8,
}

/// A dungeon instance. One per party, never shared.
#[spacetimedb::table(accessor = dungeon_instance)]
#[derive(Debug, Clone)]
pub struct DungeonInstance {
    /// Instance id.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// Its party.
    #[unique]
    pub party_id: u64,
    /// Which dungeon.
    pub dungeon_id: String,
    /// Where it is in its life.
    pub state: InstanceStateTag,
    /// Whether it is running (countdown or active): the physics tick's index.
    #[index(btree)]
    pub live: bool,
    /// When the countdown ends.
    pub countdown_ends_at: Option<Timestamp>,
    /// Whether the starting barrier is open. Derived from `state`; replicated for clients.
    pub barrier_open: bool,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it completed or failed.
    pub finished_at: Option<Timestamp>,
}

/// A player inside an instance: the runtime state the server owns.
#[spacetimedb::table(accessor = instance_player)]
#[derive(Debug, Clone)]
pub struct InstancePlayer {
    /// The player.
    #[primary_key]
    pub identity: Identity,
    /// Which instance.
    #[index(btree)]
    pub instance_id: u64,
    /// Their account id.
    pub account_id: String,
    /// The character's name.
    pub name: String,
    /// Its catalog definition (decides the model).
    pub definition_id: String,
    /// Its role.
    pub role: RoleTag,
    /// Feet position.
    pub position: V3,
    /// Velocity.
    pub velocity: V3,
    /// Surface normal under the feet.
    pub ground_normal: V3,
    /// Facing, radians.
    pub yaw: f32,
    /// On the ground.
    pub grounded: bool,
    /// The last input frame applied.
    pub last_seq: u32,
    /// When the last input arrived.
    pub last_input_at: Timestamp,
    /// Movement budget: frames banked.
    pub budget_tokens: f32,
    /// Movement budget: last refill.
    pub budget_refilled_at: Timestamp,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Connected or not.
    pub presence: PresenceTag,
}

/// A monster inside an instance.
#[spacetimedb::table(accessor = instance_monster)]
#[derive(Debug, Clone)]
pub struct InstanceMonster {
    /// Monster id.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// Which instance.
    #[index(btree)]
    pub instance_id: u64,
    /// The archetype key (`skeleton_warrior`).
    pub archetype: String,
    /// Display name.
    pub name: String,
    /// The pack it belongs to.
    pub pack: String,
    /// Feet position.
    pub position: V3,
    /// Facing.
    pub yaw: f32,
    /// Size multiplier.
    pub scale: f32,
    /// Level.
    pub level: u32,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
}

/// One bucket of the rate limiter, keyed `"<identity hex>:<operation>"`.
#[spacetimedb::table(accessor = rate_limit)]
#[derive(Debug, Clone)]
pub struct RateLimit {
    /// The bucket's key.
    #[primary_key]
    pub key: String,
    /// Calls that may still be made.
    pub tokens: f32,
    /// Last refill.
    pub refilled_at: Timestamp,
}

/// A message for one player: "Dungeon found", "You were removed from the party".
#[spacetimedb::table(accessor = player_notice)]
#[derive(Debug, Clone)]
pub struct PlayerNotice {
    /// Notice id.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// For whom.
    #[index(btree)]
    pub identity: Identity,
    /// Stable code the client can branch on (`party_formed`, `kicked` …).
    pub code: String,
    /// How to show it.
    pub level: NoticeLevel,
    /// What to show.
    pub message: String,
    /// When.
    pub created_at: Timestamp,
}

/// A dungeon result waiting for the persistence bridge to collect it.
#[spacetimedb::table(accessor = pending_result)]
#[derive(Debug, Clone)]
pub struct PendingResult {
    /// Idempotency key: `"<instance id>:<account id>"`. The API ignores a key it has seen.
    #[primary_key]
    pub result_id: String,
    /// Whose.
    pub account_id: String,
    /// Which character.
    pub character_id: String,
    /// Which dungeon.
    pub dungeon_id: String,
    /// Which instance.
    pub instance_id: u64,
    /// Completed or failed.
    pub success: bool,
    /// How long the run took, microseconds.
    pub duration_us: i64,
    /// Experience the server awarded.
    pub experience: u32,
    /// When it was recorded.
    pub created_at: Timestamp,
}
