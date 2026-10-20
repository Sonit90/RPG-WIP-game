//! One scripted player over the real network layer.
//!
//! A bot is exactly what the game client is minus the window: a `RealmClient`, a
//! `RealmMirror`, and decisions taken from what the server says. It signs in (registering
//! first if the account is new), queues its character, accepts the ready check, inserts its
//! key if it is the designated lead, starts the dungeon if it leads, and walks forward once the
//! barrier lifts.

use std::time::{Duration, Instant};

use engine_core::math::Vec3;
use engine_net::api::ApiClient;
use engine_net::{
    Action, ConnectionStatus, GameSession, MemoryTokenStore, NetCommand, NetEvent, NetworkClient,
    RealmClient, RealmEndpoints, RealmMirror,
};
use engine_realm::api::{CharacterDetail, CharacterSummary};
use engine_realm::combat::{ActionKind, CombatAction, EntityRef};
use engine_realm::instance::InstanceState;
use engine_realm::movement::InputFrame;
use engine_realm::party::{PartyState, ReadyResponse};
use engine_realm::{RealmError, Role};

/// What a bot does.
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Label for log lines.
    pub name: String,
    /// Account email.
    pub email: String,
    /// Account password.
    pub password: String,
    /// Catalog definition of the character to play.
    pub definition: String,
    /// Role to queue as.
    pub role: Role,
    /// Dungeon to queue for.
    pub dungeon: String,
    /// Accept the ready check (false: decline it).
    pub accept: bool,
    /// Insert the character's own key when the party is choosing one (the designated lead).
    pub insert_key: bool,
    /// Start the dungeon when leading.
    pub start_if_leader: bool,
    /// Walk forward once the run is active.
    pub walk: bool,
    /// Once the run is active, walk at the nearest living monster and press bar slot 0 on
    /// it when in range.
    pub hunt: bool,
    /// Queue automatically once connected.
    pub queue: bool,
}

/// A scripted player.
pub struct Bot {
    /// Its configuration.
    pub config: BotConfig,
    client: RealmClient,
    /// What the server has said.
    pub mirror: RealmMirror,
    /// The signed-in session.
    pub session: Option<GameSession>,
    /// The account's characters.
    pub characters: Vec<CharacterSummary>,
    /// The last character in full the API sent (bag and mail included).
    pub detail: Option<CharacterDetail>,
    /// The last history page the API sent.
    pub history: Option<engine_realm::api::HistoryPage>,
    /// The last run the API sent.
    pub run: Option<engine_realm::api::RunDetail>,
    /// The last leaderboard page the API sent.
    pub board: Option<engine_realm::api::LeaderboardPage>,
    /// The realm connection's state.
    pub status: ConnectionStatus,
    /// Every failure the server reported.
    pub failures: Vec<(Action, RealmError)>,
    tried_login: bool,
    queue_sent_at: Option<Instant>,
    answered_party: Option<u64>,
    start_sent_for: Option<u64>,
    key_sent_for: Option<u64>,
    /// Do not answer a ready check (a test holds the party in it).
    pub hold_ready: bool,
    next_seq: u32,
    last_input: Instant,
    last_act: Option<Instant>,
    /// Where it stood when the run went active.
    pub start_position: Option<Vec3>,
    /// The access token the bot's own API client holds (security probes only).
    pub probe_token: Option<String>,
}

impl Bot {
    /// Starts a bot: its network thread begins registering at once.
    ///
    /// # Errors
    ///
    /// The network thread could not start.
    pub fn start(config: BotConfig, endpoints: &RealmEndpoints) -> Result<Self, RealmError> {
        let client = RealmClient::start(endpoints.clone(), Box::new(MemoryTokenStore::default()))?;
        client.send(NetCommand::Register {
            email: config.email.clone(),
            password: config.password.clone(),
            display_name: Some(config.name.clone()),
            remember: false,
        });
        Ok(Self {
            config,
            client,
            mirror: RealmMirror::default(),
            session: None,
            characters: Vec::new(),
            detail: None,
            history: None,
            run: None,
            board: None,
            status: ConnectionStatus::Offline,
            failures: Vec::new(),
            tried_login: false,
            queue_sent_at: None,
            answered_party: None,
            start_sent_for: None,
            key_sent_for: None,
            hold_ready: false,
            next_seq: 1,
            last_input: Instant::now(),
            last_act: None,
            start_position: None,
            probe_token: None,
        })
    }

    /// Sends a command on the bot's behalf.
    pub fn send(&self, command: NetCommand) {
        self.client.send(command);
    }

    /// Whether it has answered a ready check.
    #[must_use]
    pub fn answered(&self) -> bool {
        self.answered_party.is_some()
    }

    /// The bot's character for its definition.
    #[must_use]
    pub fn character(&self) -> Option<&CharacterSummary> {
        self.characters
            .iter()
            .find(|c| c.definition_id == self.config.definition)
    }

    /// Drains events and acts on the current state.
    pub fn tick(&mut self) {
        let mut events = Vec::new();
        self.client.poll(&mut events);
        for event in events {
            self.on_event(event);
        }
        self.act();
    }

    fn on_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Status(status) => {
                log::info!("[{}] connection {status:?}", self.config.name);
                self.status = status;
            }
            NetEvent::SignedIn(session) => {
                log::info!("[{}] signed in as {}", self.config.name, session.account_id);
                self.session = Some(session);
                self.client.send(NetCommand::FetchCharacters);
            }
            NetEvent::Characters(characters) => self.characters = characters,
            NetEvent::Character(detail) => self.detail = Some(*detail),
            NetEvent::History { page, .. } => self.history = Some(*page),
            NetEvent::Run(run) => self.run = Some(*run),
            NetEvent::Leaderboard(page) => self.board = Some(*page),
            NetEvent::Realm(update) => self.mirror.apply(update),
            NetEvent::Failed { action, error } => {
                if action == Action::SignIn && !self.tried_login && self.session.is_none() {
                    // Already registered: sign in instead.
                    self.tried_login = true;
                    self.client.send(NetCommand::Login {
                        email: self.config.email.clone(),
                        password: self.config.password.clone(),
                        remember: false,
                    });
                    return;
                }
                log::warn!(
                    "[{}] {action:?} failed: {}",
                    self.config.name,
                    error.message
                );
                self.failures.push((action, error));
            }
            NetEvent::SignedOut { reason } => {
                if let Some(reason) = reason
                    && self.tried_login
                {
                    log::warn!("[{}] signed out: {}", self.config.name, reason.message);
                }
            }
            _ => {}
        }
    }

    fn act(&mut self) {
        if self.status != ConnectionStatus::Connected {
            return;
        }
        // Queue once connected, idle and with a character.
        if self.config.queue
            && self.mirror.ticket.is_none()
            && self.mirror.party.is_none()
            && self
                .queue_sent_at
                .is_none_or(|at| at.elapsed() > Duration::from_secs(3))
            && let Some(character) = self.character().cloned()
        {
            log::info!(
                "[{}] queueing {} as {}",
                self.config.name,
                character.name,
                self.config.role.name()
            );
            self.client.send(NetCommand::JoinQueue {
                character_id: character.id,
                dungeon_id: self.config.dungeon.clone(),
                role: self.config.role,
            });
            self.queue_sent_at = Some(Instant::now());
        }
        let Some(party) = self.mirror.party.clone() else {
            return;
        };
        if party.state == PartyState::ReadyCheck
            && !self.hold_ready
            && self.answered_party != Some(party.id)
            && self
                .mirror
                .my_membership()
                .is_some_and(|me| me.ready == ReadyResponse::Pending)
        {
            log::info!(
                "[{}] ready check for party {}: {}",
                self.config.name,
                party.id,
                if self.config.accept {
                    "accept"
                } else {
                    "decline"
                }
            );
            self.client
                .send(NetCommand::RespondReadyCheck(self.config.accept));
            self.answered_party = Some(party.id);
        }
        if party.state == PartyState::ChoosingKey
            && self.config.insert_key
            && self.key_sent_for != Some(party.id)
        {
            log::info!(
                "[{}] party {} is choosing a key: inserting its own",
                self.config.name,
                party.id
            );
            self.client.send(NetCommand::InsertKey);
            self.key_sent_for = Some(party.id);
        }
        if party.state == PartyState::Ready
            && self.config.start_if_leader
            && self.mirror.i_lead()
            && self.start_sent_for != Some(party.id)
        {
            log::info!(
                "[{}] leading party {}: starting",
                self.config.name,
                party.id
            );
            self.client.send(NetCommand::StartDungeon);
            self.start_sent_for = Some(party.id);
        }
        let active = self
            .mirror
            .instance
            .as_ref()
            .is_some_and(|instance| instance.state == InstanceState::Active);
        if active && self.start_position.is_none() {
            self.start_position = self.mirror.me().map(|me| me.position);
            self.next_seq = self.mirror.me().map_or(0, |me| me.last_seq) + 1;
        }
        if active && self.config.hunt {
            self.hunt();
            return;
        }
        if active && self.config.walk && self.last_input.elapsed() >= Duration::from_millis(33) {
            self.last_input = Instant::now();
            let frames: Vec<InputFrame> = (0..2)
                .map(|_| {
                    let frame = InputFrame {
                        seq: self.next_seq,
                        forward: 1.0,
                        right: 0.0,
                        yaw: 0.0,
                        face_camera: false,
                        jump: false,
                    };
                    self.next_seq += 1;
                    frame
                })
                .collect();
            self.client.send(NetCommand::Input(frames));
        }
    }

    /// Walks at the nearest living monster, then presses slot 0 on it every second.
    fn hunt(&mut self) {
        const RANGE: f32 = 22.0;
        let Some(me) = self.mirror.me().map(|me| me.position) else {
            return;
        };
        let Some((key, at)) = self
            .mirror
            .monsters
            .values()
            .filter(|m| m.alive)
            .map(|m| (m.key, m.position))
            .min_by(|a, b| (a.1 - me).length().total_cmp(&(b.1 - me).length()))
        else {
            return;
        };
        let offset = at - me;
        let distance = Vec3::new(offset.x, 0.0, offset.z).length();
        if distance > RANGE {
            if self.last_input.elapsed() < Duration::from_millis(33) {
                return;
            }
            self.last_input = Instant::now();
            // Yaw zero faces −Z; the forward axis rotated by yaw is (−sin, −cos).
            let yaw = (-offset.x).atan2(-offset.z);
            let frames: Vec<InputFrame> = (0..2)
                .map(|_| {
                    let frame = InputFrame {
                        seq: self.next_seq,
                        forward: 1.0,
                        right: 0.0,
                        yaw,
                        face_camera: true,
                        jump: false,
                    };
                    self.next_seq += 1;
                    frame
                })
                .collect();
            self.client.send(NetCommand::Input(frames));
            return;
        }
        if self
            .last_act
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(1000))
        {
            self.last_act = Some(Instant::now());
            log::info!(
                "[{}] pressing slot 0 on monster {key} at {distance:.1} m",
                self.config.name
            );
            self.client.send(NetCommand::Act(CombatAction {
                target: EntityRef::Monster(key),
                kind: ActionKind::Ability(0),
            }));
        }
    }

    /// Signs in on a separate API client and keeps the access token, for probes that call
    /// the realm's HTTP API directly the way an attacker would.
    ///
    /// # Errors
    ///
    /// Login failed.
    pub fn acquire_probe_token(
        &mut self,
        endpoints: &RealmEndpoints,
    ) -> Result<String, RealmError> {
        let api = ApiClient::new(endpoints);
        let pair = api.login(&self.config.email, &self.config.password)?;
        self.probe_token = Some(pair.access_token.clone());
        Ok(pair.access_token)
    }
}
