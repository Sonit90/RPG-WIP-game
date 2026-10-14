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
    /// Everyone accepted; waiting for a member to insert their key (appended last: the
    /// variant order is the wire format).
    ChoosingKey,
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
    /// Its loadout as JSON, from the persistence API: what it wears into the fight.
    pub loadout: String,
    /// Which dungeon.
    #[index(btree)]
    pub dungeon_id: String,
    /// As which role.
    pub role: RoleTag,
    /// The dungeon the character's key opens. Empty: no key.
    pub key_dungeon_id: String,
    /// The key's level, from the persistence API. Zero: no key.
    pub key_level: u32,
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
    /// The leader, once chosen: whoever inserts the key.
    pub leader: Option<Identity>,
    /// Whose key is inserted, while one is.
    pub key_owner: Option<Identity>,
    /// The inserted key's level. Zero: none.
    pub key_level: u32,
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
    /// Its loadout as JSON.
    pub loadout: String,
    /// The dungeon their key opens. Empty: no key.
    pub key_dungeon_id: String,
    /// Their key's level. Zero: no key.
    pub key_level: u32,
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
    /// The level of the key the run was started with.
    pub key_level: u32,
    /// Where it is in its life.
    pub state: InstanceStateTag,
    /// Whether its fight is ticking (countdown, active, or completed with a chest to loot):
    /// the combat tick's index.
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
    /// Its key in the fight: stable across a rebuild for every monster the level placed.
    #[index(btree)]
    pub key: u64,
    /// Alive.
    pub alive: bool,
    /// Whom it is fighting.
    pub target: NetEntity,
    /// What it is casting.
    pub cast: Option<NetCast>,
    /// Statuses on it.
    pub statuses: Vec<NetStatus>,
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

/// One chat line. Private: clients read `my_chat`, which is scoped to the caller.
///
/// The sender's name is the character they are playing, read server-side when the line is
/// stored; a client never supplies one.
#[spacetimedb::table(accessor = chat_message)]
#[derive(Debug, Clone)]
pub struct ChatMessage {
    /// Message id; larger is newer.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// `engine_realm::chat::Channel::as_u8`.
    #[index(btree)]
    pub channel: u8,
    /// Who sent it.
    #[index(btree)]
    pub sender: Identity,
    /// The sender's account id.
    pub sender_account: String,
    /// The sender's character name when it was sent.
    pub sender_name: String,
    /// A party message's party; zero for the other channels.
    #[index(btree)]
    pub party_id: u64,
    /// A whisper's target; `Identity::ZERO` for the other channels.
    #[index(btree)]
    pub recipient: Identity,
    /// A whisper's target account id; empty for the other channels.
    pub recipient_account: String,
    /// A whisper's target character name; empty for the other channels.
    pub recipient_name: String,
    /// The cleaned text.
    pub text: String,
    /// When it was sent.
    pub sent_at: Timestamp,
}

/// A dungeon result waiting for the persistence bridge to collect it.
///
/// The bridge decodes rows by position (`backend/crates/game/src/bridge.rs`): a field added
/// here is appended after the last one and added there, at the same position
/// (`key_level`, `key_owner`, `key_level_after`, `resources` follow `created_at`).
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
    /// The level of the key the run was played at.
    pub key_level: u32,
    /// Whether this player's key was the one used.
    pub key_owner: bool,
    /// The owner's key level after the run; zero when unchanged.
    pub key_level_after: u32,
    /// Resources awarded, as JSON text: `[["crypt_dust",9],...]`.
    pub resources: String,
}

/// What a finished run paid one player, for their results screen. Kept until the instance is
/// torn down. Same key as their `pending_result`.
#[spacetimedb::table(accessor = run_summary)]
#[derive(Debug, Clone)]
pub struct RunSummary {
    /// `"<instance id>:<account id>"`.
    #[primary_key]
    pub result_id: String,
    /// Whose.
    #[index(btree)]
    pub identity: Identity,
    /// Which instance.
    #[index(btree)]
    pub instance_id: u64,
    /// Which dungeon.
    pub dungeon_id: String,
    /// Its display name.
    pub dungeon_name: String,
    /// Completed or failed.
    pub success: bool,
    /// The key level the run was played at.
    pub key_level: u32,
    /// Whether this player's key was the one used.
    pub key_owner: bool,
    /// The owner's key level after the run; zero when unchanged.
    pub key_level_after: u32,
    /// How long the run took, microseconds.
    pub duration_us: i64,
    /// Experience awarded.
    pub experience: u32,
    /// Resources awarded, as JSON text: `[["crypt_dust",9],...]`.
    pub resources: String,
    /// The item rolled from the chest. Empty until known (or none).
    pub loot_item: String,
    /// When it was recorded.
    pub created_at: Timestamp,
}

/// A player's roll from the boss's chest, on its way to their character.
///
/// Written at `finish` as `"rolled"` - durable, so a restart cannot lose it. Then exactly one
/// thing settles it: the sim reports the chest opened (`"bag"`), the sim reports it mailed
/// (`"mail"`), or the instance is torn down with it still rolled (`"mail"`, `"unclaimed"`).
/// The bridge collects settled rows and the API delivers each once, under `loot_id`.
///
/// The bridge decodes rows by position (`backend/crates/game/src/bridge.rs`): a field added
/// here is added there, at the same position.
#[spacetimedb::table(accessor = pending_loot)]
#[derive(Debug, Clone)]
pub struct PendingLoot {
    /// Idempotency key: `"<instance id>:<account id>:loot"`.
    #[primary_key]
    pub loot_id: String,
    /// Whose.
    pub account_id: String,
    /// Which character.
    pub character_id: String,
    /// Which instance.
    #[index(btree)]
    pub instance_id: u64,
    /// The item's stable key.
    pub item: String,
    /// `"rolled"` (not settled, never collected), `"bag"` or `"mail"`.
    pub state: String,
    /// For mail: `"bag_full"` or `"unclaimed"`. Empty otherwise.
    pub reason: String,
    /// When it was rolled.
    pub created_at: Timestamp,
}

/// An entity as both ends can name it: `engine_realm::combat::EntityRef`.
#[derive(SpacetimeType, Debug, Clone, PartialEq, Eq)]
pub enum NetEntity {
    /// Nobody.
    Nobody,
    /// A player, by account id.
    Player(String),
    /// A monster, by key.
    Monster(u64),
}

/// An ability: `engine_realm::combat::AbilityRef`.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetAbility {
    /// None.
    Nothing,
    /// A player ability, by index.
    Player(u16),
    /// A monster ability, by index.
    Monster(u16),
}

/// A cast in progress.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq)]
pub struct NetCast {
    /// What.
    pub ability: NetAbility,
    /// Seconds in, when the row was written.
    pub elapsed: f32,
    /// Seconds in total.
    pub duration: f32,
}

/// A status on an entity.
#[derive(SpacetimeType, Debug, Clone, PartialEq)]
pub struct NetStatus {
    /// `StatusKind` index.
    pub kind: u16,
    /// Who applied it.
    pub source: NetEntity,
    /// Seconds left, when the row was written.
    pub remaining: f32,
    /// Seconds in total.
    pub duration: f32,
    /// Stacks.
    pub stacks: u8,
}

/// One ability's cooldown.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq)]
pub struct NetCooldown {
    /// `FireAbility` index.
    pub ability: u16,
    /// Charges ready.
    pub charges: u8,
    /// Seconds to the next charge, when the row was written.
    pub recharge: f32,
}

/// A class's resources.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq, Default)]
pub struct NetResources {
    /// Heat.
    pub heat: f32,
    /// Ultimate energy.
    pub energy: f32,
    /// Mana.
    pub mana: f32,
    /// Mana maximum.
    pub max_mana: f32,
    /// Deflection.
    pub deflection: f32,
    /// Astral power.
    pub astral: f32,
    /// Global cooldown left.
    pub global_cooldown: f32,
    /// Damage absorbed before health moves.
    pub absorb: f32,
}

/// A bar slot aimed at a point.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq)]
pub struct NetAimed {
    /// The bar slot.
    pub slot: u8,
    /// Where.
    pub position: V3,
}

/// A button press: `engine_realm::combat::ActionKind`.
#[derive(SpacetimeType, Debug, Clone, Copy, PartialEq)]
pub enum NetAction {
    /// Only change target.
    Target,
    /// A bar slot.
    Ability(u8),
    /// A bar slot aimed at a point.
    AbilityAt(NetAimed),
    /// The ultimate, at a point.
    Ultimate(V3),
    /// An extra button.
    Extra(u8),
    /// Open the boss's chest (appended last: the variant order is the wire format).
    OpenChest,
}

/// A player's combat state beyond health and position (which live on `InstancePlayer`).
#[spacetimedb::table(accessor = player_combat)]
#[derive(Debug, Clone)]
pub struct PlayerCombat {
    /// The player.
    #[primary_key]
    pub identity: Identity,
    /// Which instance.
    #[index(btree)]
    pub instance_id: u64,
    /// Their account id.
    pub account_id: String,
    /// Alive.
    pub alive: bool,
    /// Their selection.
    pub target: NetEntity,
    /// Their cast.
    pub cast: Option<NetCast>,
    /// Their resources.
    pub resources: NetResources,
    /// Their class's cooldowns.
    pub cooldowns: Vec<NetCooldown>,
    /// Statuses on them.
    pub statuses: Vec<NetStatus>,
    /// The fight tick this was written at.
    pub tick: u64,
}

/// One instance's fight clock and what a rebuild needs that no other row holds.
#[spacetimedb::table(accessor = instance_combat)]
#[derive(Debug, Clone)]
pub struct InstanceCombat {
    /// The instance.
    #[primary_key]
    pub instance_id: u64,
    /// Ticks run.
    pub tick: u64,
    /// When tick zero was, in server time: tick `n` is due at `origin + n / 60 s`.
    pub origin: Timestamp,
    /// Completion credited.
    pub credited: f32,
    /// Boss gate open.
    pub unlocked: bool,
}

/// What happened in one instance during one combat pass: a JSON array of
/// `engine_realm::combat::WireEvent`. Kept for a few seconds; clients apply each once.
#[spacetimedb::table(accessor = combat_batch)]
#[derive(Debug, Clone)]
pub struct CombatBatch {
    /// Increasing: the order to apply batches in.
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// Which instance.
    #[index(btree)]
    pub instance_id: u64,
    /// The fight tick at the end of the pass.
    pub tick: u64,
    /// When it was written.
    pub created_at: Timestamp,
    /// The events.
    pub events: String,
}
