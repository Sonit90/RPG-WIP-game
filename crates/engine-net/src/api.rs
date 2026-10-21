//! The realm API over HTTPS: authentication, characters, settings, the queue.
//!
//! Blocking calls, made only on the network thread. Every failure becomes a [`RealmError`]
//! with a sentence a player can read: a refused login says the email or password is wrong, a
//! dead server says the server cannot be reached, and neither ever contains a token.

use std::time::Duration;

use engine_realm::api::{
    CharacterDetail, CharacterSummary, EconomyInfo, ErrorBody, ExchangeRequest, LoginRequest,
    GemSlotResponse, GemSocketRequest, GemUnsocketRequest, GemUpgradeRequest, GemUpgradeResponse,
    MoveItemRequest, QueueRequest, ReforgeRequest, ReforgeResponse, RefreshRequest,
    RegisterRequest, RequestIdBody, SalvageRequest, SetActiveBookRequest, SettingsDocument,
    SharpenRequest, SharpenResponse, TokenPair, UseBookRequest, UseBookResponse, UserProfile,
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
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.send(
            "PUT",
            &self.persistence,
            &format!("/api/v1/characters/{}/talents", path_segment(id)),
            Some(token),
            &engine_realm::api::TalentsRequest {
                talents: talents.to_vec(),
                request_id: Some(request_id.to_owned()),
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
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!(
                "/api/v1/characters/{}/mail/{mail_id}/{}",
                path_segment(id),
                if take { "take" } else { "read" }
            ),
            Some(token),
            &request_id_body(request_id),
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
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/salvage", path_segment(id)),
            Some(token),
            &SalvageRequest {
                indices: indices.to_vec(),
                request_id: Some(request_id.to_owned()),
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
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/exchange", path_segment(id)),
            Some(token),
            &ExchangeRequest {
                exchange_id: exchange_id.to_owned(),
                times,
                request_id: Some(request_id.to_owned()),
            },
            Category::Persistence,
        )
    }

    /// Tries to take the item in `slot` one sharpening level higher. The server rolls; the
    /// answer says how it went and carries the character as it is afterwards.
    ///
    /// # Errors
    ///
    /// The item is broken, fully sharpened or not equipment, the price cannot be paid, the
    /// character is engaged in a dungeon or queue, it is not the caller's, or the server is
    /// unreachable.
    pub fn sharpen(
        &self,
        token: &str,
        id: &str,
        slot: &SlotRef,
        request_id: &str,
    ) -> Result<SharpenResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/sharpen", path_segment(id)),
            Some(token),
            &SharpenRequest {
                slot: slot.clone(),
                request_id: Some(request_id.to_owned()),
            },
            Category::Persistence,
        )
    }

    /// Pays gold to re-roll the base quality of the item in `slot` from the published table.
    /// The server rolls; the new quality replaces the old one, better or worse. `request_id`
    /// is the one id of this user action: a retry carries the same id and gets the first
    /// answer back without a second roll or charge.
    ///
    /// # Errors
    ///
    /// The item is broken or not equipment, the gold is short, the character is engaged in a
    /// dungeon or queue, it is not the caller's, or the server is unreachable.
    pub fn reforge(
        &self,
        token: &str,
        id: &str,
        slot: &SlotRef,
        request_id: &str,
    ) -> Result<ReforgeResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/reforge", path_segment(id)),
            Some(token),
            &ReforgeRequest {
                slot: slot.clone(),
                request_id: request_id.to_owned(),
            },
            Category::Persistence,
        )
    }

    /// Reads the book in bag cell `slot`: the item is consumed and the book learned. `request_id`
    /// is the one id of this user action; a retry gets the first answer back and consumes nothing
    /// more.
    ///
    /// # Errors
    ///
    /// The cell holds no book, the book is another class's or already learned (a conflict, the
    /// item stays), the character is not the caller's, or the server is unreachable.
    pub fn use_book(
        &self,
        token: &str,
        id: &str,
        slot: &SlotRef,
        request_id: &str,
    ) -> Result<UseBookResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/books/use", path_segment(id)),
            Some(token),
            &UseBookRequest {
                slot: slot.clone(),
                request_id: request_id.to_owned(),
            },
            Category::Persistence,
        )
    }

    /// Puts one gem from bag cell `gem_slot` into an empty `socket` of the item at `item_slot`.
    /// `request_id` is the one id of this user action; a retry gets the first answer back.
    ///
    /// # Errors
    ///
    /// The socket is taken or missing, the cell holds no gem, the character is engaged, it is
    /// not the caller's, or the server is unreachable.
    pub fn socket_gem(
        &self,
        token: &str,
        id: &str,
        item_slot: &SlotRef,
        socket: u8,
        gem_slot: u32,
        request_id: &str,
    ) -> Result<GemSlotResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/gems/socket", path_segment(id)),
            Some(token),
            &GemSocketRequest {
                request_id: request_id.to_owned(),
                item_slot: item_slot.clone(),
                socket,
                gem_slot,
            },
            Category::Persistence,
        )
    }

    /// Chooses the option in force for an enhancement book.
    ///
    /// # Errors
    ///
    /// The book is not learned or has no such option, the character is engaged (queued, in a
    /// party or a dungeon: a conflict), it is not the caller's, or the server is unreachable.
    pub fn set_active_book(
        &self,
        token: &str,
        id: &str,
        book_id: &str,
        option_id: &str,
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.send(
            "PUT",
            &self.persistence,
            &format!("/api/v1/characters/{}/books/active", path_segment(id)),
            Some(token),
            &SetActiveBookRequest {
                book_id: book_id.to_owned(),
                option_id: option_id.to_owned(),
                request_id: request_id.to_owned(),
            },
            Category::Persistence,
        )
    }

    /// Takes the gem out of `socket` for gold; it returns to the bag.
    ///
    /// # Errors
    ///
    /// The socket is empty, the gold is short, the bag is full, the character is engaged, or
    /// the server is unreachable.
    pub fn unsocket_gem(
        &self,
        token: &str,
        id: &str,
        item_slot: &SlotRef,
        socket: u8,
        request_id: &str,
    ) -> Result<GemSlotResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/gems/unsocket", path_segment(id)),
            Some(token),
            &GemUnsocketRequest {
                request_id: request_id.to_owned(),
                item_slot: item_slot.clone(),
                socket,
            },
            Category::Persistence,
        )
    }

    /// Upgrades the gem stack in bag cell `gem_slot`: the server consumes gems and gold and rolls.
    ///
    /// # Errors
    ///
    /// Too few gems, short of gold, the gem is at the top level, the bag is full, the character
    /// is engaged, or the server is unreachable.
    pub fn upgrade_gem(
        &self,
        token: &str,
        id: &str,
        gem_slot: u32,
        request_id: &str,
    ) -> Result<GemUpgradeResponse, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/gems/upgrade", path_segment(id)),
            Some(token),
            &GemUpgradeRequest {
                request_id: request_id.to_owned(),
                gem_slot,
            },
            Category::Persistence,
        )
    }

    /// Lowers the character's key one level (free), dropping the modifiers the lower level has
    /// no slot for.
    ///
    /// # Errors
    ///
    /// The key is already at its lowest level, the character is engaged (queued, in a party or
    /// a dungeon), is not the caller's, or the server is unreachable.
    pub fn downgrade_key(
        &self,
        token: &str,
        id: &str,
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/key/downgrade", path_segment(id)),
            Some(token),
            &request_id_body(request_id),
            Category::Persistence,
        )
    }

    /// Strips the character's key of its modifiers, for gold.
    ///
    /// # Errors
    ///
    /// The key has no modifiers, there is too little gold, the character is engaged, is not the
    /// caller's, or the server is unreachable.
    pub fn cleanse_key(
        &self,
        token: &str,
        id: &str,
        request_id: &str,
    ) -> Result<CharacterDetail, RealmError> {
        self.post(
            &self.persistence,
            &format!("/api/v1/characters/{}/key/cleanse", path_segment(id)),
            Some(token),
            &request_id_body(request_id),
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

    /// One page of a character's run history, newest first. `before` is the previous page's
    /// `next_before`.
    ///
    /// # Errors
    ///
    /// Not the caller's character, or the server is unreachable.
    pub fn history(
        &self,
        token: &str,
        character_id: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<engine_realm::api::HistoryPage, RealmError> {
        let mut path = format!(
            "/api/v1/characters/{}/history?limit={limit}",
            path_segment(character_id)
        );
        if let Some(before) = before {
            path.push_str(&format!("&before={before}"));
        }
        self.get(&self.persistence, &path, token, Category::Persistence)
    }

    /// One run the caller took part in.
    ///
    /// # Errors
    ///
    /// Not a run of the caller's, or the server is unreachable.
    pub fn run(
        &self,
        token: &str,
        run_uid: &str,
    ) -> Result<engine_realm::api::RunDetail, RealmError> {
        self.get(
            &self.persistence,
            &format!("/api/v1/runs/{}", path_segment(run_uid)),
            token,
            Category::Persistence,
        )
    }

    /// One page of a leaderboard.
    ///
    /// # Errors
    ///
    /// The server is unreachable or refused the query.
    pub fn leaderboard(
        &self,
        token: &str,
        board: &str,
        dungeon_id: &str,
        key_level: Option<u32>,
        page: u32,
    ) -> Result<engine_realm::api::LeaderboardPage, RealmError> {
        let mut path = format!(
            "/api/v1/leaderboards?board={}&dungeon={}&page={page}",
            path_segment(board),
            path_segment(dungeon_id)
        );
        if let Some(key) = key_level {
            path.push_str(&format!("&key={key}"));
        }
        self.get(&self.persistence, &path, token, Category::Persistence)
    }

    /// One page of community maps.
    ///
    /// # Errors
    ///
    /// The server is unreachable or refused the query.
    pub fn community_list(
        &self,
        token: &str,
        query: &engine_realm::community::MapQuery,
    ) -> Result<engine_realm::community::MapPage, RealmError> {
        self.get(
            &self.persistence,
            &format!("/api/v1/community/maps{}", query.to_query()),
            token,
            Category::Persistence,
        )
    }

    /// One community map with its latest validation report.
    ///
    /// # Errors
    ///
    /// No such map (or not visible to the caller), or the server is unreachable.
    pub fn community_map(
        &self,
        token: &str,
        id: &str,
    ) -> Result<engine_realm::community::MapDetail, RealmError> {
        self.get(
            &self.persistence,
            &format!("/api/v1/community/maps/{}", path_segment(id)),
            token,
            Category::Persistence,
        )
    }

    /// The stored text of one version of a map (what the scene is built from).
    ///
    /// # Errors
    ///
    /// No such version, or the server is unreachable.
    pub fn community_version(
        &self,
        token: &str,
        id: &str,
        version: u32,
    ) -> Result<String, RealmError> {
        let response = self
            .agent
            .get(format!(
                "{}/api/v1/community/maps/{}/versions/{version}",
                self.persistence,
                path_segment(id)
            ))
            .header("Authorization", format!("Bearer {token}"))
            .call();
        let mut response = response.map_err(|error| {
            log::warn!("api: request failed: {error}");
            RealmError::new(
                ErrorCategory::Connection,
                "Cannot reach the game servers. Check your connection and try again.",
            )
        })?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return response
                .body_mut()
                .with_config()
                .limit(engine_realm::community::MAX_BODY_BYTES as u64 * 4)
                .read_to_string()
                .map_err(|_| RealmError::protocol("The server sent an unexpected reply."));
        }
        let message = response
            .body_mut()
            .read_json::<ErrorBody>()
            .ok()
            .map(|body| body.error.message);
        Err(classify(status, message, Category::Persistence))
    }

    /// Creates a draft (`map_id` `None`) or saves a new version of `map_id`. `text` is the map
    /// file. A text that is not a map at all comes back as `Ok` with `map: None` and the report.
    ///
    /// # Errors
    ///
    /// Not the author's map, a rejected map, a limit, or the server is unreachable.
    pub fn community_save(
        &self,
        token: &str,
        map_id: Option<&str>,
        text: &str,
        request_id: &str,
    ) -> Result<engine_realm::community::SaveMapResponse, RealmError> {
        let data: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| RealmError::persistence(format!("That file is not valid JSON: {e}")))?;
        let body = engine_realm::community::SaveMapRequest {
            request_id: Some(request_id.to_owned()),
            data,
        };
        let (method, path) = match map_id {
            Some(id) => (
                "PUT",
                format!("/api/v1/community/maps/{}", path_segment(id)),
            ),
            None => ("POST", "/api/v1/community/maps".to_owned()),
        };
        self.send_with_report(method, &path, token, &body)
    }

    /// A dry run of the validator.
    ///
    /// # Errors
    ///
    /// The server is unreachable or the file is not JSON.
    pub fn community_validate(
        &self,
        token: &str,
        text: &str,
    ) -> Result<engine_realm::community::SaveMapResponse, RealmError> {
        let data: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| RealmError::persistence(format!("That file is not valid JSON: {e}")))?;
        let body = engine_realm::community::SaveMapRequest {
            request_id: None,
            data,
        };
        self.send_with_report("POST", "/api/v1/community/validate", token, &body)
    }

    /// Publishes (`publish`) or unpublishes the author's map.
    ///
    /// # Errors
    ///
    /// Not the author's map, a moderated map, a map with validation errors (the message says
    /// so; read the report from the map), or the server is unreachable.
    pub fn community_publish(
        &self,
        token: &str,
        id: &str,
        publish: bool,
        request_id: &str,
    ) -> Result<engine_realm::community::MapDetail, RealmError> {
        let body = engine_realm::community::MapActionRequest {
            request_id: Some(request_id.to_owned()),
            reason: None,
        };
        let action = if publish { "publish" } else { "unpublish" };
        match self.send_with_report_raw(
            "POST",
            &format!("/api/v1/community/maps/{}/{action}", path_segment(id)),
            token,
            &body,
        )? {
            (status, text) if (200..300).contains(&status) => serde_json::from_str(&text)
                .map_err(|_| RealmError::protocol("The server sent an unexpected reply.")),
            (422, _) => Err(RealmError::persistence(
                "This map has validation errors. Fix them and save again before publishing.",
            )),
            (status, text) => Err(error_from(status, &text, Category::Persistence)),
        }
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

    /// A save or validate call: 200 and 422 both carry a `SaveMapResponse`.
    fn send_with_report<B: Serialize>(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: &B,
    ) -> Result<engine_realm::community::SaveMapResponse, RealmError> {
        let (status, text) = self.send_with_report_raw(method, path, token, body)?;
        if (200..300).contains(&status) || status == 422 {
            if let Ok(response) =
                serde_json::from_str::<engine_realm::community::SaveMapResponse>(&text)
            {
                return Ok(response);
            }
        }
        Err(error_from(status, &text, Category::Persistence))
    }

    /// Sends a JSON body and returns the status and the body text whatever the status.
    fn send_with_report_raw<B: Serialize>(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: &B,
    ) -> Result<(u16, String), RealmError> {
        let url = format!("{}{path}", self.persistence);
        let request = match method {
            "PUT" => self.agent.put(url),
            _ => self.agent.post(url),
        }
        .header("Authorization", format!("Bearer {token}"));
        let mut response = request.send_json(body).map_err(|error| {
            log::warn!("api: request failed: {error}");
            RealmError::new(
                ErrorCategory::Connection,
                "Cannot reach the game servers. Check your connection and try again.",
            )
        })?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|_| RealmError::protocol("The server sent an unexpected reply."))?;
        Ok((status, text))
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

/// The body of the hub routes that carry nothing but the idempotency key.
fn request_id_body(request_id: &str) -> RequestIdBody {
    RequestIdBody {
        request_id: Some(request_id.to_owned()),
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

/// The error for a failed response whose body text is `text`.
fn error_from(status: u16, text: &str, category: Category) -> RealmError {
    let message = serde_json::from_str::<ErrorBody>(text)
        .ok()
        .map(|body| body.error.message);
    classify(status, message, category)
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
