//! The realm API over HTTPS: authentication, characters, settings, the queue.
//!
//! Blocking calls, made only on the network thread. Every failure becomes a [`RealmError`]
//! with a sentence a player can read: a refused login says the email or password is wrong, a
//! dead server says the server cannot be reached, and neither ever contains a token.

use std::time::Duration;

use engine_realm::api::{
    CharacterDetail, CharacterSummary, EconomyInfo, EnhanceRequest, EnhanceResponse, ErrorBody,
    ExchangeRequest, LoginRequest, MoveItemRequest, QueueRequest, RefreshRequest, RegisterRequest,
    SalvageRequest, SettingsDocument, TokenPair, UserProfile,
};
use engine_realm::loadout::SlotRef;
use engine_realm::{ErrorCategory, RealmError};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::RealmEndpoints;

/// A client for the realm API.
pub struct ApiClient {
    agent: ureq::Agent,
    auth: String,
    persistence: String,
}

impl ApiClient {
    /// A client for `endpoints`.
    #[must_use]
    pub fn new(endpoints: &RealmEndpoints) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(10)))
            .user_agent("mygame-client")
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            auth: endpoints.auth_api_url.clone(),
            persistence: endpoints.persistence_api_url.clone(),
        }
    }

    /// Signs in.
    ///
    /// # Errors
    ///
    /// Wrong credentials, a disabled account, a rate limit, or an unreachable server.
    pub fn login(&self, email: &str, password: &str) -> Result<TokenPair, RealmError> {
        let body = LoginRequest {
            email: email.to_owned(),
            password: password.to_owned(),
        };
        self.post(
            &self.auth,
            "/api/v1/auth/login",
            None,
            &body,
            Category::Auth,
        )
    }

    /// Creates an account and signs in.
    ///
    /// # Errors
    ///
    /// The email is taken or invalid, the password too short, or the server is unreachable.
    pub fn register(
        &self,
        email: &str,
        password: &str,
        display_name: Option<&str>,
    ) -> Result<TokenPair, RealmError> {
        let body = RegisterRequest {
            email: email.to_owned(),
            password: password.to_owned(),
            display_name: display_name.map(str::to_owned),
        };
        self.post(
            &self.auth,
            "/api/v1/auth/register",
            None,
            &body,
            Category::Auth,
        )
    }

    /// Exchanges a refresh token for a new pair. The old refresh token is spent.
    ///
    /// # Errors
    ///
    /// The refresh token expired, was revoked or reused, or the server is unreachable.
    pub fn refresh(&self, refresh_token: &str) -> Result<TokenPair, RealmError> {
        let body = RefreshRequest {
            refresh_token: refresh_token.to_owned(),
        };
        self.post(
            &self.auth,
            "/api/v1/auth/refresh",
            None,
            &body,
            Category::Auth,
        )
    }

    /// Revokes a refresh token. Failure is logged and otherwise ignored: the local session is
    /// forgotten either way.
    pub fn logout(&self, refresh_token: &str) {
        let body = RefreshRequest {
            refresh_token: refresh_token.to_owned(),
        };
        let result = self
            .agent
            .post(format!("{}/api/v1/auth/logout", self.auth))
            .send_json(&body);
        if let Err(error) = result {
            log::warn!("api: logout did not reach the server: {error}");
        }
    }

    /// Who the token belongs to.
    ///
    /// # Errors
    ///
    /// The token is invalid or expired, or the server is unreachable.
    pub fn me(&self, token: &str) -> Result<UserProfile, RealmError> {
        self.get(&self.auth, "/api/v1/users/me", token, Category::Auth)
    }

    /// The account's characters. The first call for a new account creates its starters.
    ///
    /// # Errors
    ///
    /// The token is invalid, or the server is unreachable.
    pub fn characters(&self, token: &str) -> Result<Vec<CharacterSummary>, RealmError> {
        self.get(
            &self.persistence,
            "/api/v1/characters",
            token,
            Category::Persistence,
        )
    }

    /// One character in full.
    ///
    /// # Errors
    ///
    /// Not the caller's character, or the server is unreachable.
    pub fn character(&self, token: &str, id: &str) -> Result<CharacterDetail, RealmError> {
        self.get(
            &self.persistence,
            &format!("/api/v1/characters/{}", path_segment(id)),
            token,
            Category::Persistence,
        )
    }

    /// Moves an item on a character, server-validated.
    ///
    /// # Errors
    ///
    /// The move is illegal, the character is not the caller's, or the server is unreachable.
    pub fn move_item(
        &self,
        token: &str,
        id: &str,
        request: &MoveItemRequest,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/equipment/move", path_segment(id)),
            Some(token),
            request,
            Category::Persistence,
        )
    }

    /// Replaces a character's talents, validated against its points and the engagement lock.
    ///
    /// # Errors
    ///
    /// An unknown talent, too few points, the character is engaged (a conflict), it is not the
    /// caller's, or the server is unreachable.
    pub fn save_talents(
        &self,
        token: &str,
        id: &str,
        talents: &[String],
    ) -> Result<CharacterDetail, RealmError> {
        self.send(
            "PUT",
            &self.persistence,
            &format!("/api/v1/characters/{}/talents", path_segment(id)),
            Some(token),
            &engine_realm::api::TalentsRequest {
                talents: talents.to_vec(),
            },
            Category::Persistence,
        )
    }

    /// Takes a mail's item into the bag, or marks the mail read (`take = false`).
    ///
    /// # Errors
    ///
    /// The bag is full (a conflict), there is no such mail, the character is not the caller's,
    /// or the server is unreachable.
    pub fn mail(
        &self,
        token: &str,
        id: &str,
        mail_id: u64,
        take: bool,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!(
                "/api/v1/characters/{}/mail/{mail_id}/{}",
                path_segment(id),
                if take { "take" } else { "read" }
            ),
            Some(token),
            &serde_json::json!({}),
            Category::Persistence,
        )
    }

    /// Breaks the whole stacks in the bag cells `indices` down into a resource.
    ///
    /// # Errors
    ///
    /// A cell is not a bag item, the tray is empty or too big, the character is not the
    /// caller's, or the server is unreachable.
    pub fn salvage(
        &self,
        token: &str,
        id: &str,
        indices: &[u32],
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/salvage", path_segment(id)),
            Some(token),
            &SalvageRequest {
                indices: indices.to_vec(),
            },
            Category::Persistence,
        )
    }

    /// Exchanges resources `times` over, by catalog exchange id.
    ///
    /// # Errors
    ///
    /// Unknown exchange, insufficient resources, the character is not the caller's, or the
    /// server is unreachable.
    pub fn exchange(
        &self,
        token: &str,
        id: &str,
        exchange_id: &str,
        times: u32,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/exchange", path_segment(id)),
            Some(token),
            &ExchangeRequest {
                exchange_id: exchange_id.to_owned(),
                times,
            },
            Category::Persistence,
        )
    }

    /// Tries to take the item in `slot` one enhancement level higher. The server rolls; the
    /// answer says how it went and carries the character as it is afterwards.
    ///
    /// # Errors
    ///
    /// The item is broken, fully enhanced or not equipment, the price cannot be paid, the
    /// character is engaged in a dungeon or queue, it is not the caller's, or the server is
    /// unreachable.
    pub fn enhance(
        &self,
        token: &str,
        id: &str,
        slot: &SlotRef,
    ) -> Result<EnhanceResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/enhance", path_segment(id)),
            Some(token),
            &EnhanceRequest { slot: slot.clone() },
            Category::Persistence,
        )
    }

    /// The economy catalog: resources, salvage yield, exchanges.
    ///
    /// # Errors
    ///
    /// The token is invalid, or the server is unreachable.
    pub fn economy(&self, token: &str) -> Result<EconomyInfo, RealmError> {
        self.get(
            &self.persistence,
            "/api/v1/economy",
            token,
            Category::Persistence,
        )
    }

    /// The account's synced settings, if any were ever saved.
    ///
    /// # Errors
    ///
    /// The token is invalid, or the server is unreachable.
    pub fn settings(&self, token: &str) -> Result<SettingsDocument, RealmError> {
        self.get(
            &self.persistence,
            "/api/v1/player/settings",
            token,
            Category::Persistence,
        )
    }

    /// Saves the account's synced settings, replacing `document.version`.
    ///
    /// # Errors
    ///
    /// Another machine saved first (a conflict), or the server is unreachable.
    pub fn save_settings(
        &self,
        token: &str,
        document: &SettingsDocument,
    ) -> Result<SettingsDocument, RealmError> {
        self.send(
            "PUT",
            &self.persistence,
            "/api/v1/player/settings",
            Some(token),
            document,
            Category::Persistence,
        )
    }

    /// Asks the API to queue one of the caller's characters.
    ///
    /// # Errors
    ///
    /// The character is not the caller's, cannot play the role, does not meet the dungeon's
    /// requirements, or the realm refused (already queued, not connected).
    pub fn queue(&self, token: &str, request: &QueueRequest) -> Result<(), RealmError> {
        let _: serde_json::Value = self.post(
            &self.persistence,
            "/api/v1/realm/queue",
            Some(token),
            request,
            Category::Matchmaking,
        )?;
        Ok(())
    }

    fn get<T: DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        token: &str,
        category: Category,
    ) -> Result<T, RealmError> {
        let response = self
            .agent
            .get(format!("{base}{path}"))
            .header("Authorization", format!("Bearer {token}"))
            .call();
        read(response, category)
    }

    fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        base: &str,
        path: &str,
        token: Option<&str>,
        body: &B,
        category: Category,
    ) -> Result<T, RealmError> {
        self.send("POST", base, path, token, body, category)
    }

    fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        base: &str,
        path: &str,
        token: Option<&str>,
        body: &B,
        category: Category,
    ) -> Result<T, RealmError> {
        let url = format!("{base}{path}");
        let mut request = match method {
            "PUT" => self.agent.put(url),
            _ => self.agent.post(url),
        };
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        read(request.send_json(body), category)
    }
}

/// Which error category a non-auth failure falls into.
#[derive(Clone, Copy)]
enum Category {
    Auth,
    Persistence,
    Matchmaking,
}

/// Escapes one path segment (ids are UUIDs, but nothing here assumes it).
fn path_segment(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// A response's body as `T`, or the error it carried.
fn read<T: DeserializeOwned>(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    category: Category,
) -> Result<T, RealmError> {
    let mut response = response.map_err(|error| {
        log::warn!("api: request failed: {error}");
        RealmError::new(
            ErrorCategory::Connection,
            "Cannot reach the game servers. Check your connection and try again.",
        )
    })?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        if status == 204 {
            return serde_json::from_value(serde_json::Value::Null)
                .map_err(|_| RealmError::protocol("The server sent an unexpected reply."));
        }
        return response.body_mut().read_json::<T>().map_err(|error| {
            log::warn!("api: unreadable response: {error}");
            RealmError::protocol("The server sent an unexpected reply.")
        });
    }
    let message = response
        .body_mut()
        .read_json::<ErrorBody>()
        .ok()
        .map(|body| body.error.message);
    Err(classify(status, message, category))
}

/// Turns a status code into the error a player sees.
fn classify(status: u16, message: Option<String>, category: Category) -> RealmError {
    let base = match category {
        Category::Auth => ErrorCategory::Authentication,
        Category::Persistence => ErrorCategory::Persistence,
        Category::Matchmaking => ErrorCategory::Matchmaking,
    };
    match status {
        401 => RealmError::new(
            ErrorCategory::Authentication,
            match category {
                Category::Auth => message.unwrap_or_else(|| "Incorrect email or password.".into()),
                _ => "Your session has expired. Please sign in again.".into(),
            },
        ),
        403 => RealmError::new(
            ErrorCategory::Authorization,
            message.unwrap_or_else(|| "You are not allowed to do that.".into()),
        ),
        429 => RealmError::rate_limited(),
        500..=599 => RealmError::new(
            ErrorCategory::Connection,
            "The game servers are having trouble. Try again shortly.",
        ),
        _ => RealmError::new(
            base,
            message.unwrap_or_else(|| format!("The request failed ({status}).")),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_player_messages() {
        let error = classify(401, None, Category::Auth);
        assert_eq!(error.category, ErrorCategory::Authentication);
        assert_eq!(error.message, "Incorrect email or password.");
        let error = classify(401, None, Category::Persistence);
        assert!(error.message.contains("expired"));
        assert_eq!(
            classify(429, None, Category::Auth).category,
            ErrorCategory::RateLimited
        );
        assert_eq!(
            classify(503, None, Category::Auth).category,
            ErrorCategory::Connection
        );
        let error = classify(409, Some("Taken.".into()), Category::Matchmaking);
        assert_eq!(error.category, ErrorCategory::Matchmaking);
        assert_eq!(error.message, "Taken.");
    }

    #[test]
    fn path_segments_cannot_escape() {
        assert_eq!(path_segment("../../admin"), "admin");
        assert_eq!(
            path_segment("1b4e28ba-2fa1-11d2-883f-0016d3cca427"),
            "1b4e28ba-2fa1-11d2-883f-0016d3cca427"
        );
    }
}
