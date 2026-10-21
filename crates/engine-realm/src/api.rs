//! The realm API's request and response bodies, shared by the game client and the server.
//!
//! One definition for both ends of every HTTP call, so a renamed field is a compile error in
//! both programs rather than a silent `null` in one. Routes are listed with each type.

use serde::{Deserialize, Serialize};

use crate::loadout::{BagItem, EquippedItem, SlotRef};
use crate::modifiers::KeyModifier;
use crate::outcome::RunOutcome;
use crate::role::Role;

/// `POST /api/v1/auth/login`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    /// Account email.
    pub email: String,
    /// Password. Sent over TLS, never logged, never stored by the client.
    pub password: String,
}

/// `POST /api/v1/auth/register`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    /// Account email.
    pub email: String,
    /// Password, at least eight characters.
    pub password: String,
    /// What other players see.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

/// `POST /api/v1/auth/refresh` and `POST /api/v1/auth/logout`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshRequest {
    /// The refresh token. Single use: a refresh returns a new one.
    pub refresh_token: String,
}

/// What login, registration and refresh return.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenPair {
    /// The access token: an ES256 OIDC token the API and the realm both accept.
    pub access_token: String,
    /// The refresh token.
    pub refresh_token: String,
    /// Seconds until the access token expires.
    pub access_expires_in: i64,
}

impl std::fmt::Debug for TokenPair {
    /// Never prints a token: a debug log line must not be a credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenPair")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("access_expires_in", &self.access_expires_in)
            .finish()
    }
}

/// `GET /api/v1/users/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// The account id (a UUID).
    pub id: String,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Display name.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Whether the email was verified.
    #[serde(default)]
    pub email_verified: bool,
}

/// One persistent character, as the selection screen lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterSummary {
    /// The character's id (a UUID).
    pub id: String,
    /// Its catalog definition.
    pub definition_id: String,
    /// Its name.
    pub name: String,
    /// Its class, from the catalog.
    pub class: String,
    /// The roles it may queue as.
    pub roles: Vec<Role>,
    /// Level.
    pub level: u32,
    /// Experience into the current level.
    pub experience: u64,
    /// Gear score, computed by the server from the loadout it owns.
    pub gear_score: u32,
    /// What it wears.
    pub equipment: Vec<EquippedItem>,
    /// The character's dungeon key, if it has one.
    #[serde(default)]
    pub key: Option<KeySummary>,
    /// Resources held, `(resource id, amount)`, sorted by id.
    #[serde(default)]
    pub resources: Vec<(String, u64)>,
}

/// A character's dungeon key, as the API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySummary {
    /// The dungeon it opens.
    pub dungeon_id: String,
    /// That dungeon's display name.
    pub dungeon_name: String,
    /// Key level.
    pub level: u32,
    /// The modifiers the key carries, with their reroll magnitudes. Rolled by the API when the
    /// key is created or its level changes.
    #[serde(default)]
    pub modifiers: Vec<KeyModifier>,
}

/// `GET /api/v1/characters/{id}`: everything about one character.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterDetail {
    /// The summary.
    #[serde(flatten)]
    pub summary: CharacterSummary,
    /// What it carries.
    pub inventory: Vec<BagItem>,
    /// What waits in the mailbox.
    #[serde(default)]
    pub mail: Vec<crate::loadout::MailItem>,
    /// Talents bought, by key.
    pub talents: Vec<String>,
    /// Every talent point the character has: the starting points plus the ones it earned.
    #[serde(default)]
    pub talent_points_total: u32,
    /// Talent points not yet spent.
    #[serde(default)]
    pub talent_points_available: u32,
    /// Derived stats, `(key, value)`.
    pub stats: Vec<(String, f32)>,
    /// Dungeons completed, by id, with counts.
    pub completions: Vec<DungeonCompletion>,
}

/// How often a character finished a dungeon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DungeonCompletion {
    /// The dungeon.
    pub dungeon_id: String,
    /// Times completed.
    pub count: u32,
}

/// `POST /api/v1/characters/{id}/equipment/move`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveItemRequest {
    /// Where the item is.
    pub from: SlotRef,
    /// Where it goes.
    pub to: SlotRef,
    /// Idempotency key: the client's id for this one user action, reused if it retries. A
    /// replay returns the first answer and changes nothing. Absent: no replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `POST /api/v1/characters/{id}/salvage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalvageRequest {
    /// The bag cells to break down, whole stacks at once.
    pub indices: Vec<u32>,
    /// Idempotency key: the client's id for this one user action, reused if it retries. A
    /// replay returns the first answer and changes nothing. Absent: no replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `POST /api/v1/characters/{id}/exchange`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRequest {
    /// Which exchange in the economy catalog.
    pub exchange_id: String,
    /// How many trades.
    pub times: u32,
    /// Idempotency key: the client's id for this one user action, reused if it retries. A
    /// replay returns the first answer and changes nothing. Absent: no replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// The body of the hub routes that take nothing else (`mail/{id}/take`, `key/downgrade`,
/// `key/cleanse`): only the idempotency key. The body may be omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestIdBody {
    /// Idempotency key: the client's id for this one user action, reused if it retries. A
    /// replay returns the first answer and changes nothing. Absent: no replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// One resource in [`EconomyInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceInfo {
    /// Stable id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One sentence for the tooltip.
    #[serde(default)]
    pub description: String,
}

/// What salvaging an item yields, in [`EconomyInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SalvageInfo {
    /// The resource id it yields.
    pub resource: String,
    /// Yielded for any item.
    pub base: u32,
    /// Added per item level.
    pub per_item_level: u32,
    /// How many bag cells one salvage may take (the tray's size).
    pub slots: u32,
}

/// One exchange in [`EconomyInfo`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeInfo {
    /// Stable id.
    pub id: String,
    /// Resource paid.
    pub from: String,
    /// Cost of one trade.
    pub from_amount: u32,
    /// Resource received.
    pub to: String,
    /// Yield of one trade.
    pub to_amount: u32,
}

/// `GET /api/v1/economy`: the economy catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EconomyInfo {
    /// Every resource.
    pub resources: Vec<ResourceInfo>,
    /// The salvage rule.
    pub salvage: SalvageInfo,
    /// Every exchange.
    pub exchanges: Vec<ExchangeInfo>,
}

/// `PUT /api/v1/characters/{id}/talents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TalentsRequest {
    /// Every talent the character should have, by key.
    pub talents: Vec<String>,
    /// Idempotency key: the client's id for this one user action, reused if it retries. A
    /// replay returns the first answer and changes nothing. Absent: no replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `GET` / `PUT /api/v1/player/settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsDocument {
    /// Optimistic concurrency: a `PUT` must name the version it replaces.
    pub version: i64,
    /// The account-scoped settings, as JSON (`engine_settings::AccountSettings`).
    pub settings: serde_json::Value,
}

/// `POST /api/v1/realm/queue`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueRequest {
    /// Which of the caller's characters.
    pub character_id: String,
    /// Which dungeon.
    pub dungeon_id: String,
    /// As which role.
    pub role: Role,
    /// For a community map (`cm:` dungeon): the difficulty to play at, 1 to
    /// [`crate::community::MAX_DIFFICULTY`]. Ignored for official dungeons, which use the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u32>,
}

/// The API's error body: `{"error":{"code":..,"message":..}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    /// The error.
    pub error: ErrorDetail,
}

/// One API error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// A stable code.
    pub code: String,
    /// A sentence a player can read.
    pub message: String,
}

/// One dungeon result, as the bridge hands it to the persistence store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DungeonResult {
    /// Idempotency key: `"<run uid>:<account id>"`.
    pub result_id: String,
    /// The run's uid (hex): groups the results of one run. Empty for a result that predates it.
    #[serde(default)]
    pub run_uid: String,
    /// Whose.
    pub account_id: String,
    /// Which character.
    pub character_id: String,
    /// Which dungeon.
    pub dungeon_id: String,
    /// Completed or failed.
    pub success: bool,
    /// How long it took, microseconds.
    pub duration_us: i64,
    /// Experience awarded by the realm.
    pub experience: u32,
    /// The key level the run was played at (0: no key).
    #[serde(default)]
    pub key_level: u32,
    /// Whether this character's key opened the run.
    #[serde(default)]
    pub key_owner: bool,
    /// The level the key becomes on success (owner only; 0 otherwise).
    #[serde(default)]
    pub key_level_after: u32,
    /// Resources paid, `(resource id, amount)`.
    #[serde(default)]
    pub resources: Vec<(String, u32)>,
    /// The player's damage report, computed by the realm from its own combat events. `None`
    /// when the realm had none (an admin-finished run, an unreadable row).
    #[serde(default)]
    pub damage_report: Option<crate::meter::DamageReport>,
    /// Talent points granted. Applied once with the result, like everything else in it.
    #[serde(default)]
    pub talent_points: u32,
    /// What the player picked up from monsters during the run (gold, aether shards): paid
    /// whether the run succeeded or failed, in the same transaction as everything else here.
    #[serde(default)]
    pub looted: Vec<(String, u32)>,
    /// What the character spent during the run (key rerolls), `(resource id, amount)`. Debited
    /// from the loadout in the transaction that records the result, success or failure, and
    /// never below zero.
    #[serde(default)]
    pub currency_spent: Vec<(String, u32)>,
    /// A member who did not open the run: whether their own key rises (the realm worked out
    /// that it was lower than the used key and the run succeeded).
    #[serde(default)]
    pub participant_key_bump: bool,
    /// The level of the member's own key when they queued: the compare-and-set guard of their
    /// bump. Zero when there is none.
    #[serde(default)]
    pub own_key_level: u32,
    /// How this player's run ended. `None` for a result from before outcomes existed: read it
    /// with [`DungeonResult::outcome`].
    #[serde(default)]
    pub outcome: Option<RunOutcome>,
    /// The dungeon's timer for the run, microseconds; zero when it has none.
    #[serde(default)]
    pub timer_us: i64,
    /// Who played the run. Empty on a result that must not write the run's own row (a member who
    /// left before it ended); every other result of the run carries the whole party.
    #[serde(default)]
    pub party: Vec<RunMember>,
    /// The key's modifiers as the run had them (rerolls included).
    #[serde(default)]
    pub modifiers: Vec<KeyModifier>,
}

/// A change to a character's key a result asks for, applied only if the key still has
/// `expected_level` (so it can never rise twice for one run).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyUpdate {
    /// The level the key must still have.
    pub expected_level: u32,
    /// The level it becomes.
    pub new_level: u32,
}

impl DungeonResult {
    /// How the run ended for this player; a result from before outcomes existed is read as
    /// timed (success) or failed.
    #[must_use]
    pub fn outcome(&self) -> RunOutcome {
        self.outcome.unwrap_or_else(|| RunOutcome::legacy(self.success))
    }

    /// The key change this result asks for, if any. Only a success moves a key: the owner's
    /// key from the level used to `key_level_after`, a participant's from their own level
    /// (`own_key_level`) when the realm flagged the bump.
    #[must_use]
    pub fn key_update(&self) -> Option<KeyUpdate> {
        if !self.success || self.key_level_after == 0 {
            return None;
        }
        let expected_level = if self.key_owner {
            self.key_level
        } else if self.participant_key_bump {
            self.own_key_level
        } else {
            return None;
        };
        (expected_level >= 1 && self.key_level_after > expected_level).then_some(KeyUpdate {
            expected_level,
            new_level: self.key_level_after,
        })
    }

    /// What to debit from the loadout, as `(resource id, amount)` with repeats merged.
    #[must_use]
    pub fn debits(&self) -> Vec<(String, u64)> {
        let mut merged: Vec<(String, u64)> = Vec::new();
        for (id, amount) in &self.currency_spent {
            match merged.iter_mut().find(|(existing, _)| existing == id) {
                Some((_, total)) => *total = total.saturating_add(u64::from(*amount)),
                None => merged.push((id.clone(), u64::from(*amount))),
            }
        }
        merged
    }
}

/// Whether the realm has a character engaged: queued, in a party, or in an instance.
///
/// The answer to `svc_engagement`. While `engaged`, the API refuses to change the character's
/// talents or equipment: the realm holds a snapshot of them for the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Engagement {
    /// Whether the account is queued, in a party or in an instance.
    pub engaged: bool,
    /// Which: `"none"`, `"queued"`, `"party"` or `"instance"`.
    pub state: String,
}

/// The sentence a refused change shows.
pub const ENGAGED_MESSAGE: &str =
    "Your talents and equipment are locked while you are in a dungeon or queue.";

/// One chest item to deliver to a character, as the bridge hands it to the persistence store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LootDelivery {
    /// Idempotency key.
    pub loot_id: String,
    /// Whose.
    pub account_id: String,
    /// Which character.
    pub character_id: String,
    /// The item's stable key.
    pub item: String,
    /// `"bag"` (the player opened the chest: bag if there is room, else mail) or `"mail"`.
    pub kind: String,
    /// For mail, why: `"bag_full"`, `"unclaimed"` or `"dungeon_drop"`.
    pub reason: String,
    /// Rolled quality, per cent of the base stats.
    #[serde(default = "default_quality")]
    pub quality: u8,
    /// Sharpening level (zero for anything just rolled).
    #[serde(default)]
    #[serde(rename = "enhancement", alias = "sharpening")]
    pub sharpening: u8,
    /// Whether the item is broken.
    #[serde(default)]
    pub broken: bool,
}

fn default_quality() -> u8 {
    engine_game::character::QUALITY_DEFAULT
}

impl LootDelivery {
    /// The item's instance properties, clamped into the legal ranges: a forged row can be
    /// wrong but never stronger than the rules allow.
    #[must_use]
    pub fn props(&self) -> engine_game::character::ItemProps {
        engine_game::character::ItemProps::new(self.quality, self.sharpening, self.broken)
    }
}

/// One member of a finished run, as the history lists the party.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMember {
    /// The character's id.
    #[serde(default)]
    pub character_id: String,
    /// The character's name.
    pub name: String,
    /// Its class ("Warrior", "Fire Mage").
    pub hero: String,
    /// The role it queued as (`tank`, `healer`, `dps`).
    #[serde(default)]
    pub role: String,
}

/// One item a character received in a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLoot {
    /// The item's stable key.
    pub item: String,
    /// Rolled quality, per cent of the base stats.
    #[serde(default = "default_quality")]
    pub quality: u8,
    /// Sharpening level (zero for anything just rolled).
    #[serde(default)]
    pub sharpening: u8,
    /// Whether the item came broken.
    #[serde(default)]
    pub broken: bool,
}

/// The most history rows one request returns.
pub const HISTORY_MAX_LIMIT: u32 = 50;

/// One run in a character's history (`GET /api/v1/characters/{id}/history`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// The run (hex uid); empty for a result from before runs were recorded.
    #[serde(default)]
    pub run_uid: String,
    /// Which dungeon.
    pub dungeon_id: String,
    /// The key level.
    pub key_level: u32,
    /// How it ended for this character.
    pub outcome: RunOutcome,
    /// How long the run took, microseconds.
    pub duration_us: i64,
    /// The dungeon's timer, microseconds; zero when it had none.
    #[serde(default)]
    pub timer_us: i64,
    /// When the result was recorded, microseconds since the epoch: the paging cursor.
    pub applied_at_us: i64,
    /// Who played (empty when the run's own row was never written).
    #[serde(default)]
    pub party: Vec<RunMember>,
    /// The key's modifiers.
    #[serde(default)]
    pub modifiers: Vec<KeyModifier>,
    /// Times this character died.
    #[serde(default)]
    pub deaths: u32,
    /// Damage this character took.
    #[serde(default)]
    pub damage_taken: u64,
    /// Healing this character did.
    #[serde(default)]
    pub healing: u64,
    /// Damage per second over the run.
    #[serde(default)]
    pub dps: f32,
    /// What this character received.
    #[serde(default)]
    pub loot: Vec<RunLoot>,
}

/// `GET /api/v1/characters/{id}/history?before=&limit=`: newest first.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HistoryPage {
    /// The runs, newest first.
    pub runs: Vec<HistoryEntry>,
    /// Pass as `before` for the next (older) page; `None` at the end.
    #[serde(default)]
    pub next_before: Option<i64>,
}

/// `GET /api/v1/runs/{run_uid}`: one run, for a player who took part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunDetail {
    /// The run (hex uid).
    pub run_uid: String,
    /// The season it counted in.
    #[serde(default)]
    pub season_id: Option<String>,
    /// Which dungeon.
    pub dungeon_id: String,
    /// The key level.
    pub key_level: u32,
    /// How the run ended.
    pub outcome: RunOutcome,
    /// How long it took, microseconds.
    pub duration_us: i64,
    /// The timer, microseconds; zero when none.
    #[serde(default)]
    pub timer_us: i64,
    /// When it finished, microseconds since the epoch.
    pub finished_at_us: i64,
    /// Who played.
    pub party: Vec<RunMember>,
    /// The key's modifiers.
    #[serde(default)]
    pub modifiers: Vec<KeyModifier>,
    /// The asking player's own figures and loot.
    pub you: Option<HistoryEntry>,
}

/// One row of a leaderboard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderboardRow {
    /// Position on the board, from 1.
    pub rank: u32,
    /// The run (hex uid).
    pub run_uid: String,
    /// The key level.
    pub key_level: u32,
    /// Timed or overtime.
    pub outcome: RunOutcome,
    /// How long it took, microseconds.
    pub duration_us: i64,
    /// The timer, microseconds; zero when none.
    #[serde(default)]
    pub timer_us: i64,
    /// When it finished, microseconds since the epoch.
    pub finished_at_us: i64,
    /// Who played.
    pub party: Vec<RunMember>,
}

/// One player on the completions board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRow {
    /// Position, from 1.
    pub rank: u32,
    /// The character that completed.
    pub character_id: String,
    /// Their name.
    pub name: String,
    /// Their class.
    #[serde(default)]
    pub hero: String,
    /// Completed runs of the dungeon.
    pub completions: u32,
    /// The highest key level (difficulty) among them.
    pub best_key_level: u32,
    /// The fastest, microseconds.
    pub best_duration_us: i64,
}

/// `GET /api/v1/leaderboards?dungeon=&key=&season=&board=&page=`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderboardPage {
    /// The board (`fastest` or `highest`).
    pub board: String,
    /// The dungeon.
    pub dungeon_id: String,
    /// The key level asked for (`fastest`); `None` for `highest`.
    #[serde(default)]
    pub key_level: Option<u32>,
    /// The season.
    pub season_id: String,
    /// The page, from 0.
    pub page: u32,
    /// Rows per page.
    pub page_size: u32,
    /// The rows of this page, ranked.
    pub rows: Vec<LeaderboardRow>,
    /// Whether a later page has rows.
    #[serde(default)]
    pub has_more: bool,
    /// The rows of the `completions` board (which ranks players, so `rows` is empty then).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completions: Vec<CompletionRow>,
}

/// `POST /api/v1/characters/{id}/sharpen`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharpenRequest {
    /// The worn slot or bag cell holding the item. There is no roll in it: the server rolls.
    pub slot: SlotRef,
    /// Idempotency key: the client's id for this one user action, reused if it retries. A
    /// replay returns the first answer and changes nothing. Absent: no replay protection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// How a sharpening attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharpenOutcomeName {
    /// The item gained a level.
    Success,
    /// The item broke.
    Broken,
}

/// What `POST /api/v1/characters/{id}/sharpen` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharpenResponse {
    /// `"success"` or `"broken"`.
    pub outcome: SharpenOutcomeName,
    /// The item's sharpening level afterwards (zero when it broke).
    pub new_level: u8,
    /// The character in full, as every other change answers.
    pub character: CharacterDetail,
}

/// `POST /api/v1/characters/{id}/reforge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReforgeRequest {
    /// The worn slot or bag cell holding the item. There is no roll in it: the server rolls.
    pub slot: SlotRef,
    /// Idempotency key (a UUID): the client's id for this one user action, reused if it
    /// retries. Required, because a gamble must never be repeated by a retry: a replay returns
    /// the first answer and changes nothing, re-rolls nothing, charges nothing.
    pub request_id: String,
}

/// What `POST /api/v1/characters/{id}/reforge` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReforgeResponse {
    /// The reforged item's stable key.
    pub item: String,
    /// Its base quality before, per cent.
    pub old_quality: u8,
    /// Its base quality now (it replaced the old one, better or worse).
    pub new_quality: u8,
    /// The gold paid.
    pub cost: u64,
    /// The gold the character holds afterwards.
    pub gold_left: u64,
    /// The character in full, as every other change answers.
    pub character: CharacterDetail,
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    #[test]
    fn token_pairs_never_print_their_tokens() {
        let pair = TokenPair {
            access_token: "secret-access".into(),
            refresh_token: "secret-refresh".into(),
            access_expires_in: 1800,
        };
        let printed = format!("{pair:?}");
        assert!(!printed.contains("secret"));
    }

    #[test]
    fn detail_flattens_its_summary() {
        let detail = CharacterDetail {
            summary: CharacterSummary {
                id: "c".into(),
                definition_id: "warrior".into(),
                name: "Warrior".into(),
                class: "Warrior".into(),
                roles: vec![Role::Tank],
                level: 1,
                experience: 0,
                gear_score: 40,
                equipment: vec![],
                key: None,
                resources: vec![],
            },
            inventory: vec![],
            mail: vec![],
            talents: vec![],
            talent_points_total: 5,
            talent_points_available: 5,
            stats: vec![("str".into(), 1.0)],
            completions: vec![],
        };
        let json = serde_json::to_value(&detail).expect("serializes");
        assert_eq!(json["gear_score"], 40);
        let back: CharacterDetail = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, detail);
    }

    fn winning_result() -> DungeonResult {
        DungeonResult {
            result_id: "1:a".into(),
            run_uid: String::new(),
            account_id: "a".into(),
            character_id: "c".into(),
            dungeon_id: "graveyard_crypt".into(),
            success: true,
            duration_us: 1,
            experience: 0,
            key_level: 5,
            key_owner: false,
            key_level_after: 0,
            resources: vec![],
            damage_report: None,
            talent_points: 0,
            looted: vec![],
            currency_spent: vec![],
            participant_key_bump: false,
            own_key_level: 0,
            outcome: None,
            timer_us: 0,
            party: vec![],
            modifiers: vec![],
        }
    }

    #[test]
    fn a_result_from_before_outcomes_reads_with_defaults() {
        let old = r#"{"result_id":"1:a","account_id":"a","character_id":"c","dungeon_id":"graveyard_crypt","success":true,"duration_us":5,"experience":1}"#;
        let result: DungeonResult = serde_json::from_str(old).expect("reads");
        assert_eq!(result.outcome, None);
        assert_eq!(result.outcome(), RunOutcome::Timed);
        assert_eq!((result.timer_us, result.party.len(), result.modifiers.len()), (0, 0, 0));
        let failed = DungeonResult {
            success: false,
            ..result
        };
        assert_eq!(failed.outcome(), RunOutcome::Failed);
    }

    #[test]
    fn a_result_round_trips_with_its_outcome_party_and_modifiers() {
        let result = DungeonResult {
            outcome: Some(RunOutcome::Overtime),
            timer_us: 1_500_000_000,
            party: vec![RunMember {
                character_id: "c".into(),
                name: "Ash".into(),
                hero: "Fire Mage".into(),
                role: "dps".into(),
            }],
            modifiers: vec![KeyModifier::base("rotating_beams")],
            ..winning_result()
        };
        let text = serde_json::to_string(&result).expect("writes");
        assert!(text.contains(r#""outcome":"overtime""#));
        let back: DungeonResult = serde_json::from_str(&text).expect("reads");
        assert_eq!(back, result);
        assert_eq!(back.outcome(), RunOutcome::Overtime);
    }

    #[test]
    fn history_and_board_bodies_round_trip_and_tolerate_missing_optional_fields() {
        let page = HistoryPage {
            runs: vec![HistoryEntry {
                run_uid: "ab".into(),
                dungeon_id: "graveyard_crypt".into(),
                key_level: 4,
                outcome: RunOutcome::Timed,
                duration_us: 9,
                timer_us: 10,
                applied_at_us: 1_700_000_000_000_000,
                party: vec![],
                modifiers: vec![],
                deaths: 2,
                damage_taken: 30,
                healing: 4,
                dps: 12.5,
                loot: vec![RunLoot {
                    item: "ashgraves_seal".into(),
                    quality: 103,
                    sharpening: 2,
                    broken: false,
                }],
            }],
            next_before: Some(1_700_000_000_000_000),
        };
        let text = serde_json::to_string(&page).expect("writes");
        assert_eq!(serde_json::from_str::<HistoryPage>(&text).expect("reads"), page);
        let minimal = r#"{"runs":[{"dungeon_id":"d","key_level":1,"outcome":"failed","duration_us":1,"applied_at_us":2}]}"#;
        let read: HistoryPage = serde_json::from_str(minimal).expect("reads");
        assert_eq!(read.next_before, None);
        assert_eq!(read.runs[0].loot.len(), 0);
        let loot: RunLoot = serde_json::from_str(r#"{"item":"x"}"#).expect("reads");
        assert_eq!(loot.quality, engine_game::character::QUALITY_DEFAULT);
        let board: LeaderboardPage = serde_json::from_str(
            r#"{"board":"highest","dungeon_id":"d","season_id":"season-0","page":0,"page_size":25,"rows":[]}"#,
        )
        .expect("reads");
        assert!(!board.has_more && board.key_level.is_none());
    }

    #[test]
    fn the_owners_key_is_guarded_by_the_level_used() {
        let result = DungeonResult {
            key_owner: true,
            key_level_after: 6,
            ..winning_result()
        };
        assert_eq!(
            result.key_update(),
            Some(KeyUpdate {
                expected_level: 5,
                new_level: 6
            })
        );
        // A failure or a claim that does not rise moves nothing.
        assert_eq!(
            DungeonResult {
                success: false,
                ..result.clone()
            }
            .key_update(),
            None
        );
        assert_eq!(
            DungeonResult {
                key_level_after: 5,
                ..result.clone()
            }
            .key_update(),
            None
        );
        assert_eq!(
            DungeonResult {
                key_level_after: 0,
                ..result
            }
            .key_update(),
            None
        );
    }

    #[test]
    fn a_participants_key_is_guarded_by_their_own_level() {
        let bump = DungeonResult {
            participant_key_bump: true,
            own_key_level: 2,
            key_level_after: 3,
            ..winning_result()
        };
        assert_eq!(
            bump.key_update(),
            Some(KeyUpdate {
                expected_level: 2,
                new_level: 3
            })
        );
        assert_eq!(
            DungeonResult {
                participant_key_bump: false,
                ..bump.clone()
            }
            .key_update(),
            None
        );
        assert_eq!(
            DungeonResult {
                success: false,
                ..bump.clone()
            }
            .key_update(),
            None
        );
        assert_eq!(
            DungeonResult {
                own_key_level: 0,
                ..bump.clone()
            }
            .key_update(),
            None
        );
        assert_eq!(
            DungeonResult {
                own_key_level: 3,
                ..bump
            }
            .key_update(),
            None
        );
    }

    #[test]
    fn debits_merge_repeats() {
        let result = DungeonResult {
            currency_spent: vec![("aether_shard".into(), 1), ("aether_shard".into(), 2)],
            ..winning_result()
        };
        assert_eq!(result.debits(), vec![("aether_shard".to_owned(), 3)]);
        assert!(winning_result().debits().is_empty());
    }
}
