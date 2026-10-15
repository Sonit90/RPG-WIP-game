//! The realm API's request and response bodies, shared by the game client and the server.
//!
//! One definition for both ends of every HTTP call, so a renamed field is a compile error in
//! both programs rather than a silent `null` in one. Routes are listed with each type.

use serde::{Deserialize, Serialize};

use crate::loadout::{BagItem, EquippedItem, SlotRef};
use crate::modifiers::KeyModifier;
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
}

/// `POST /api/v1/characters/{id}/salvage`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalvageRequest {
    /// The bag cells to break down, whole stacks at once.
    pub indices: Vec<u32>,
}

/// `POST /api/v1/characters/{id}/exchange`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRequest {
    /// Which exchange in the economy catalog.
    pub exchange_id: String,
    /// How many trades.
    pub times: u32,
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
    /// Idempotency key.
    pub result_id: String,
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
    /// For mail, why: `"bag_full"` or `"unclaimed"`.
    pub reason: String,
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
            currency_spent: vec![],
            participant_key_bump: false,
            own_key_level: 0,
        }
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
