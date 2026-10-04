//! The realm API's request and response bodies, shared by the game client and the server.
//!
//! One definition for both ends of every HTTP call, so a renamed field is a compile error in
//! both programs rather than a silent `null` in one. Routes are listed with each type.

use serde::{Deserialize, Serialize};

use crate::loadout::{BagItem, EquippedItem, SlotRef};
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
}

/// `GET /api/v1/characters/{id}`: everything about one character.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterDetail {
    /// The summary.
    #[serde(flatten)]
    pub summary: CharacterSummary,
    /// What it carries.
    pub inventory: Vec<BagItem>,
    /// Talents bought, by key.
    pub talents: Vec<String>,
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
            },
            inventory: vec![],
            talents: vec![],
            stats: vec![("str".into(), 1.0)],
            completions: vec![],
        };
        let json = serde_json::to_value(&detail).expect("serializes");
        assert_eq!(json["gear_score"], 40);
        let back: CharacterDetail = serde_json::from_value(json).expect("deserializes");
        assert_eq!(back, detail);
    }
}
