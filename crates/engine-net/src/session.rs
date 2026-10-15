//! The network thread and the game thread's handle on it.
//!
//! ```text
//! game thread                         network thread
//! ───────────                         ──────────────
//! client.send(NetCommand) ──channel──► worker: API calls (blocking, here only)
//!                                        │      realm link (SDK thread → callbacks)
//! client.poll(&mut events) ◄─channel── NetEvent: auth, characters, RealmUpdate, failures
//! ```
//!
//! The game thread never waits: `send` enqueues, `poll` drains whatever has arrived. Login,
//! the persistence API, matchmaking and SpacetimeDB all happen behind the channel.
//!
//! The worker owns the session: tokens (the access token in memory only, the refresh token in
//! the [`TokenStore`] when "remember me" is on), the realm link, and the reconnect policy —
//! exponential back-off, a token refresh before every attempt, and the same account identity
//! every time because the identity is derived from the account, not the connection.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use engine_realm::api::{
    CharacterDetail, CharacterSummary, EconomyInfo, MoveItemRequest, QueueRequest,
    SettingsDocument, TokenPair,
};
use engine_realm::loadout::SlotRef;
use engine_realm::movement::InputFrame;
use engine_realm::{ErrorCategory, RealmError, Role};

use crate::api::ApiClient;
use crate::clock::ClockSync;
use crate::config::RealmEndpoints;
use crate::model::RealmUpdate;
use crate::realm::{LinkSignal, RealmLink};
use crate::token_store::{StoredSession, TokenStore};

/// Something the game asks the network layer to do.
#[derive(Debug, Clone)]
pub enum NetCommand {
    /// Sign in with credentials.
    Login {
        /// Email.
        email: String,
        /// Password. Dropped as soon as the request is sent.
        password: String,
        /// Keep the session across restarts.
        remember: bool,
    },
    /// Create an account, then sign in.
    Register {
        /// Email.
        email: String,
        /// Password.
        password: String,
        /// Display name.
        display_name: Option<String>,
        /// Keep the session across restarts.
        remember: bool,
    },
    /// Sign in with the stored session, if there is one.
    Resume,
    /// Sign out everywhere this session reaches.
    Logout,
    /// Reload the account's characters.
    FetchCharacters,
    /// Load one character in full.
    FetchCharacter(String),
    /// Move an item on a character.
    MoveItem {
        /// Which character.
        character_id: String,
        /// From.
        from: SlotRef,
        /// To.
        to: SlotRef,
    },
    /// Replace a character's talents.
    SaveTalents {
        /// Which character.
        character_id: String,
        /// Every talent it should have, by key.
        talents: Vec<String>,
    },
    /// Take a mail's item into the bag (`take`), or only mark the mail read.
    Mail {
        /// Which character.
        character_id: String,
        /// Which mail.
        mail_id: u64,
        /// Take the item, rather than only reading.
        take: bool,
    },
    /// Break the whole stacks in some bag cells down into a resource.
    Salvage {
        /// Which character.
        character_id: String,
        /// The bag cells, from the salvage tray.
        indices: Vec<u32>,
    },
    /// Exchange resources through a catalog exchange.
    Exchange {
        /// Which character.
        character_id: String,
        /// The catalog exchange id.
        exchange_id: String,
        /// How many times.
        times: u32,
    },
    /// Try to enhance the item in a slot; the server rolls.
    Enhance {
        /// Which character.
        character_id: String,
        /// The worn slot or bag cell.
        slot: SlotRef,
    },
    /// Load the economy catalog.
    FetchEconomy,
    /// Load the account's synced settings.
    FetchSettings,
    /// Save the account's synced settings.
    SaveSettings(SettingsDocument),
    /// Join the queue.
    JoinQueue {
        /// Which character.
        character_id: String,
        /// Which dungeon.
        dungeon_id: String,
        /// As which role.
        role: Role,
    },
    /// Leave the queue.
    LeaveQueue,
    /// Answer the ready check.
    RespondReadyCheck(bool),
    /// Leave the party.
    LeaveParty,
    /// Remove a member (leader).
    Kick(String),
    /// Hand over leadership (leader).
    TransferLeader(String),
    /// Start the dungeon (leader).
    StartDungeon,
    /// Insert the caller's own key into the party (while choosing the key).
    InsertKey,
    /// Take the caller's key back out (key owner, before the start).
    WithdrawKey,
    /// Movement input frames.
    Input(Vec<InputFrame>),
    /// A combat button press (or only a target change).
    Act(engine_realm::combat::CombatAction),
    /// Dismiss a notice.
    DismissNotice(u64),
    /// Say something. The server names the sender.
    SendChat {
        /// The channel.
        channel: engine_realm::chat::Channel,
        /// A whisper's character name; empty otherwise.
        target: String,
        /// The text.
        text: String,
    },
}

/// Which request a failure belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Signing in or registering.
    SignIn,
    /// Connecting to the realm.
    Connect,
    /// Loading characters.
    Characters,
    /// Moving an item.
    MoveItem,
    /// Saving talents.
    Talents,
    /// Taking or reading mail.
    Mail,
    /// Salvaging an item.
    Salvage,
    /// Exchanging resources.
    Exchange,
    /// Enhancing an item.
    Enhance,
    /// Loading the economy catalog.
    Economy,
    /// Loading or saving settings.
    Settings,
    /// Joining the queue.
    JoinQueue,
    /// Leaving the queue.
    LeaveQueue,
    /// Answering the ready check.
    ReadyCheck,
    /// Leaving the party.
    LeaveParty,
    /// Kicking.
    Kick,
    /// Transferring leadership.
    TransferLeader,
    /// Starting the dungeon.
    StartDungeon,
    /// Inserting the key.
    InsertKey,
    /// Withdrawing the key.
    WithdrawKey,
    /// Sending a chat message.
    Chat,
}

/// The realm connection's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Not signed in; nothing to connect.
    Offline,
    /// Opening the connection.
    Connecting,
    /// Connected and subscribed.
    Connected,
    /// Lost; trying again.
    Reconnecting {
        /// Which attempt is next.
        attempt: u32,
        /// Milliseconds until it.
        retry_in_ms: u64,
    },
}

/// The signed-in session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameSession {
    /// The account id.
    pub account_id: String,
    /// Its email.
    pub email: String,
    /// Its display name.
    pub display_name: String,
}

/// What the network layer reports.
#[derive(Debug, Clone)]
pub enum NetEvent {
    /// The realm connection changed state.
    Status(ConnectionStatus),
    /// A sign-in began.
    SigningIn,
    /// Signed in.
    SignedIn(GameSession),
    /// Signed out, by request or because the session could not be renewed.
    SignedOut {
        /// Why, if it was not the player's choice.
        reason: Option<RealmError>,
    },
    /// A stored session exists for this email (pre-fill the login form).
    RememberedEmail(String),
    /// The account's characters.
    Characters(Vec<CharacterSummary>),
    /// One character in full.
    Character(Box<CharacterDetail>),
    /// An enhancement attempt was made; the character is the server's copy afterwards.
    Enhanced(Box<engine_realm::api::EnhanceResponse>),
    /// The economy catalog.
    Economy(Box<EconomyInfo>),
    /// The account's synced settings.
    Settings(SettingsDocument),
    /// Settings were saved; this is the new version.
    SettingsSaved(i64),
    /// The API accepted a queue request; the ticket arrives through the realm.
    QueueAccepted,
    /// A change to the realm mirror.
    Realm(RealmUpdate),
    /// A new clock estimate.
    Clock(ClockSync),
    /// A request failed.
    Failed {
        /// Which.
        action: Action,
        /// Why, for the player.
        error: RealmError,
    },
}

/// The game's interface to the network layer.
pub trait NetworkClient {
    /// Asks for something. Never blocks.
    fn send(&self, command: NetCommand);
    /// Moves every event that has arrived into `out`. Never blocks.
    fn poll(&self, out: &mut Vec<NetEvent>);
}

/// Messages into the worker: the game's commands and the link's signals.
#[derive(Debug)]
pub enum WorkerMessage {
    /// From the game.
    Command(NetCommand),
    /// From the realm link of the given generation. Signals from an older link — one already
    /// replaced by a reconnect — are ignored, so a late "disconnected" from the old socket
    /// cannot tear down the new one.
    Link(u64, LinkSignal),
    /// Stop.
    Shutdown,
}

/// The network layer, as the game holds it.
pub struct RealmClient {
    commands: Sender<WorkerMessage>,
    events: Receiver<NetEvent>,
    thread: Option<JoinHandle<()>>,
}

impl RealmClient {
    /// Starts the network thread.
    ///
    /// # Errors
    ///
    /// The thread could not be spawned.
    pub fn start(
        endpoints: RealmEndpoints,
        store: Box<dyn TokenStore>,
    ) -> Result<Self, RealmError> {
        if endpoints.insecure_remote() {
            log::warn!(
                "network: a realm address uses plain http to a remote host; credentials will cross the network unencrypted"
            );
        }
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker_tx = command_tx.clone();
        let thread = std::thread::Builder::new()
            .name("network".into())
            .spawn(move || {
                Worker::new(endpoints, store, event_tx, worker_tx).run(&command_rx);
            })
            .map_err(|error| {
                RealmError::new(
                    ErrorCategory::Connection,
                    format!("The network thread could not start: {error}"),
                )
            })?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
        })
    }
}

impl NetworkClient for RealmClient {
    fn send(&self, command: NetCommand) {
        let _ = self.commands.send(WorkerMessage::Command(command));
    }

    fn poll(&self, out: &mut Vec<NetEvent>) {
        out.extend(self.events.try_iter());
    }
}

impl Drop for RealmClient {
    fn drop(&mut self) {
        let _ = self.commands.send(WorkerMessage::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Back-off between reconnect attempts, by attempt number.
#[must_use]
pub fn reconnect_delay(attempt: u32) -> Duration {
    const STEPS: [u64; 6] = [500, 1_000, 2_000, 4_000, 8_000, 15_000];
    let index = usize::try_from(attempt)
        .unwrap_or(usize::MAX)
        .min(STEPS.len() - 1);
    Duration::from_millis(STEPS[index])
}

/// The current tokens.
struct Tokens {
    access: String,
    refresh: String,
    expires_at: Instant,
}

impl Tokens {
    fn from(pair: TokenPair) -> Self {
        let seconds = u64::try_from(pair.access_expires_in.max(30)).unwrap_or(30);
        Self {
            access: pair.access_token,
            refresh: pair.refresh_token,
            expires_at: Instant::now() + Duration::from_secs(seconds),
        }
    }

    /// Whether it should be renewed now: within two minutes of expiry.
    fn stale(&self) -> bool {
        self.expires_at
            .checked_duration_since(Instant::now())
            .is_none_or(|left| left < Duration::from_secs(120))
    }
}

struct Worker {
    endpoints: RealmEndpoints,
    api: ApiClient,
    store: Box<dyn TokenStore>,
    events: Sender<NetEvent>,
    myself: Sender<WorkerMessage>,
    tokens: Option<Tokens>,
    session: Option<GameSession>,
    remember: bool,
    link: Option<RealmLink>,
    status: ConnectionStatus,
    attempt: u32,
    reconnect_at: Option<Instant>,
    clock: ClockSync,
    /// Incremented for every link opened.
    generation: u64,
}

impl Worker {
    fn new(
        endpoints: RealmEndpoints,
        store: Box<dyn TokenStore>,
        events: Sender<NetEvent>,
        myself: Sender<WorkerMessage>,
    ) -> Self {
        let api = ApiClient::new(&endpoints);
        Self {
            endpoints,
            api,
            store,
            events,
            myself,
            tokens: None,
            session: None,
            remember: false,
            link: None,
            status: ConnectionStatus::Offline,
            attempt: 0,
            reconnect_at: None,
            clock: ClockSync::default(),
            generation: 0,
        }
    }

    fn emit(&self, event: NetEvent) {
        let _ = self.events.send(event);
    }

    fn fail(&self, action: Action, error: RealmError) {
        log::info!(
            "network: {action:?} failed ({:?}): {}",
            error.category,
            error.message
        );
        self.emit(NetEvent::Failed { action, error });
    }

    fn set_status(&mut self, status: ConnectionStatus) {
        if self.status != status {
            self.status = status;
            log::info!("network: realm connection {status:?}");
            self.emit(NetEvent::Status(status));
        }
    }

    fn run(mut self, commands: &Receiver<WorkerMessage>) {
        if let Some(stored) = self.store.load() {
            self.emit(NetEvent::RememberedEmail(stored.email));
        }
        loop {
            let wait = self.reconnect_at.map_or(Duration::from_millis(500), |at| {
                at.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(500))
            });
            match commands.recv_timeout(wait) {
                Ok(WorkerMessage::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(WorkerMessage::Command(command)) => self.handle(command),
                Ok(WorkerMessage::Link(generation, signal)) => {
                    if generation == self.generation {
                        self.on_link(signal);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.timers();
        }
        if let Some(link) = self.link.take() {
            link.close();
        }
    }

    fn timers(&mut self) {
        if self.tokens.as_ref().is_some_and(Tokens::stale) && self.session.is_some() {
            self.renew();
        }
        if let Some(at) = self.reconnect_at
            && Instant::now() >= at
        {
            self.reconnect_at = None;
            self.connect();
        }
    }

    fn handle(&mut self, command: NetCommand) {
        match command {
            NetCommand::Login {
                email,
                password,
                remember,
            } => {
                self.emit(NetEvent::SigningIn);
                let result = self.api.login(&email, &password);
                drop(password);
                self.signed_in(result, email, remember);
            }
            NetCommand::Register {
                email,
                password,
                display_name,
                remember,
            } => {
                self.emit(NetEvent::SigningIn);
                let result = self
                    .api
                    .register(&email, &password, display_name.as_deref());
                drop(password);
                self.signed_in(result, email, remember);
            }
            NetCommand::Resume => {
                let Some(stored) = self.store.load() else {
                    self.emit(NetEvent::SignedOut { reason: None });
                    return;
                };
                self.emit(NetEvent::SigningIn);
                let result = self.api.refresh(&stored.refresh_token);
                if let Err(error) = &result
                    && error.category == ErrorCategory::Authentication
                {
                    self.store.clear();
                }
                self.signed_in(result, stored.email, true);
            }
            NetCommand::Logout => self.sign_out(None),
            NetCommand::FetchCharacters => {
                let Some(token) = self.access() else { return };
                match self.api.characters(&token) {
                    Ok(characters) => self.emit(NetEvent::Characters(characters)),
                    Err(error) => self.fail(Action::Characters, error),
                }
            }
            NetCommand::FetchCharacter(id) => {
                let Some(token) = self.access() else { return };
                match self.api.character(&token, &id) {
                    Ok(detail) => self.emit(NetEvent::Character(Box::new(detail))),
                    Err(error) => self.fail(Action::Characters, error),
                }
            }
            NetCommand::MoveItem {
                character_id,
                from,
                to,
            } => {
                let Some(token) = self.access() else { return };
                match self
                    .api
                    .move_item(&token, &character_id, &MoveItemRequest { from, to })
                {
                    Ok(detail) => self.emit(NetEvent::Character(Box::new(detail))),
                    Err(error) => self.fail(Action::MoveItem, error),
                }
            }
            NetCommand::SaveTalents {
                character_id,
                talents,
            } => {
                let Some(token) = self.access() else { return };
                match self.api.save_talents(&token, &character_id, &talents) {
                    Ok(detail) => self.emit(NetEvent::Character(Box::new(detail))),
                    Err(error) => self.fail(Action::Talents, error),
                }
            }
            NetCommand::Mail {
                character_id,
                mail_id,
                take,
            } => {
                let Some(token) = self.access() else { return };
                match self.api.mail(&token, &character_id, mail_id, take) {
                    Ok(detail) => self.emit(NetEvent::Character(Box::new(detail))),
                    Err(error) => self.fail(Action::Mail, error),
                }
            }
            NetCommand::Salvage {
                character_id,
                indices,
            } => {
                let Some(token) = self.access() else { return };
                match self.api.salvage(&token, &character_id, &indices) {
                    Ok(detail) => self.emit(NetEvent::Character(Box::new(detail))),
                    Err(error) => self.fail(Action::Salvage, error),
                }
            }
            NetCommand::Exchange {
                character_id,
                exchange_id,
                times,
            } => {
                let Some(token) = self.access() else { return };
                match self
                    .api
                    .exchange(&token, &character_id, &exchange_id, times)
                {
                    Ok(detail) => self.emit(NetEvent::Character(Box::new(detail))),
                    Err(error) => self.fail(Action::Exchange, error),
                }
            }
            NetCommand::Enhance { character_id, slot } => {
                let Some(token) = self.access() else { return };
                match self.api.enhance(&token, &character_id, &slot) {
                    Ok(response) => self.emit(NetEvent::Enhanced(Box::new(response))),
                    Err(error) => self.fail(Action::Enhance, error),
                }
            }
            NetCommand::FetchEconomy => {
                let Some(token) = self.access() else { return };
                match self.api.economy(&token) {
                    Ok(economy) => self.emit(NetEvent::Economy(Box::new(economy))),
                    Err(error) => self.fail(Action::Economy, error),
                }
            }
            NetCommand::FetchSettings => {
                let Some(token) = self.access() else { return };
                match self.api.settings(&token) {
                    Ok(document) => self.emit(NetEvent::Settings(document)),
                    Err(error) => self.fail(Action::Settings, error),
                }
            }
            NetCommand::SaveSettings(document) => {
                let Some(token) = self.access() else { return };
                match self.api.save_settings(&token, &document) {
                    Ok(saved) => self.emit(NetEvent::SettingsSaved(saved.version)),
                    Err(error) => self.fail(Action::Settings, error),
                }
            }
            NetCommand::JoinQueue {
                character_id,
                dungeon_id,
                role,
            } => {
                let Some(token) = self.access() else { return };
                let request = QueueRequest {
                    character_id,
                    dungeon_id,
                    role,
                };
                match self.api.queue(&token, &request) {
                    Ok(()) => self.emit(NetEvent::QueueAccepted),
                    Err(error) => self.fail(Action::JoinQueue, error),
                }
            }
            NetCommand::LeaveQueue => self.with_link(Action::LeaveQueue, RealmLink::queue_leave),
            NetCommand::RespondReadyCheck(accept) => {
                self.with_link(Action::ReadyCheck, |link| link.ready_check_respond(accept));
            }
            NetCommand::LeaveParty => self.with_link(Action::LeaveParty, RealmLink::party_leave),
            NetCommand::Kick(account) => {
                self.with_link(Action::Kick, |link| link.party_kick(account));
            }
            NetCommand::TransferLeader(account) => {
                self.with_link(Action::TransferLeader, |link| {
                    link.party_transfer_leader(account);
                });
            }
            NetCommand::StartDungeon => {
                self.with_link(Action::StartDungeon, RealmLink::party_start);
            }
            NetCommand::InsertKey => {
                self.with_link(Action::InsertKey, RealmLink::party_insert_key);
            }
            NetCommand::WithdrawKey => {
                self.with_link(Action::WithdrawKey, RealmLink::party_withdraw_key);
            }
            NetCommand::Input(frames) => {
                if let Some(link) = &self.link
                    && self.status == ConnectionStatus::Connected
                {
                    link.send_input(&frames);
                }
            }
            NetCommand::Act(action) => {
                if let Some(link) = &self.link
                    && self.status == ConnectionStatus::Connected
                {
                    link.act(&action);
                }
            }
            NetCommand::SendChat {
                channel,
                target,
                text,
            } => self.with_link(Action::Chat, |link| link.send_chat(channel, target, text)),
            NetCommand::DismissNotice(id) => {
                if let Some(link) = &self.link {
                    link.dismiss_notice(id);
                }
            }
        }
    }

    fn with_link(&self, action: Action, body: impl FnOnce(&RealmLink)) {
        match &self.link {
            Some(link) if self.status == ConnectionStatus::Connected => body(link),
            _ => self.fail(
                action,
                RealmError::new(
                    ErrorCategory::Connection,
                    "Not connected to the game server. Reconnecting…",
                ),
            ),
        }
    }

    /// The access token, or a failure event and `None` if signed out.
    fn access(&self) -> Option<String> {
        let token = self.tokens.as_ref().map(|tokens| tokens.access.clone());
        if token.is_none() {
            self.fail(
                Action::SignIn,
                RealmError::new(ErrorCategory::Authentication, "Sign in first."),
            );
        }
        token
    }

    fn signed_in(&mut self, result: Result<TokenPair, RealmError>, email: String, remember: bool) {
        let pair = match result {
            Ok(pair) => pair,
            Err(error) => {
                self.fail(Action::SignIn, error.clone());
                self.emit(NetEvent::SignedOut {
                    reason: Some(error),
                });
                return;
            }
        };
        let tokens = Tokens::from(pair);
        let profile = match self.api.me(&tokens.access) {
            Ok(profile) => profile,
            Err(error) => {
                self.fail(Action::SignIn, error.clone());
                self.emit(NetEvent::SignedOut {
                    reason: Some(error),
                });
                return;
            }
        };
        self.remember = remember;
        if remember {
            self.store.save(&StoredSession {
                refresh_token: tokens.refresh.clone(),
                email: email.clone(),
            });
        } else {
            self.store.clear();
        }
        let session = GameSession {
            account_id: profile.id,
            display_name: profile
                .display_name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| email.split('@').next().unwrap_or("Adventurer").to_owned()),
            email,
        };
        log::info!("auth: signed in account_id={}", session.account_id);
        self.tokens = Some(tokens);
        self.session = Some(session.clone());
        self.emit(NetEvent::SignedIn(session));
        self.attempt = 0;
        self.connect();
    }

    fn sign_out(&mut self, reason: Option<RealmError>) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(link) = self.link.take() {
            link.close();
        }
        if let Some(tokens) = self.tokens.take() {
            if reason.is_none() {
                self.api.logout(&tokens.refresh);
            }
        }
        if reason.is_none() {
            self.store.clear();
        }
        if let Some(session) = self.session.take() {
            log::info!("auth: signed out account_id={}", session.account_id);
        }
        self.reconnect_at = None;
        self.clock.reset();
        self.set_status(ConnectionStatus::Offline);
        self.emit(NetEvent::SignedOut { reason });
    }

    /// Renews the access token. A refusal ends the session; a network failure keeps it and
    /// tries again on the next pass.
    fn renew(&mut self) {
        let Some(refresh) = self.tokens.as_ref().map(|tokens| tokens.refresh.clone()) else {
            return;
        };
        match self.api.refresh(&refresh) {
            Ok(pair) => {
                let tokens = Tokens::from(pair);
                if self.remember
                    && let Some(session) = &self.session
                {
                    self.store.save(&StoredSession {
                        refresh_token: tokens.refresh.clone(),
                        email: session.email.clone(),
                    });
                }
                self.tokens = Some(tokens);
                log::info!("auth: access token renewed");
            }
            Err(error) if error.category == ErrorCategory::Authentication => {
                self.store.clear();
                self.sign_out(Some(RealmError::new(
                    ErrorCategory::Authentication,
                    "Your session has expired. Please sign in again.",
                )));
            }
            Err(error) => {
                log::warn!("auth: renewal failed, will retry: {}", error.message);
                if let Some(tokens) = &mut self.tokens {
                    // Try again in thirty seconds rather than every pass.
                    tokens.expires_at = Instant::now() + Duration::from_secs(150);
                }
            }
        }
    }

    fn connect(&mut self) {
        if self.session.is_none() {
            return;
        }
        if self.tokens.as_ref().is_some_and(Tokens::stale) {
            self.renew();
        }
        let Some(token) = self.tokens.as_ref().map(|tokens| tokens.access.clone()) else {
            return;
        };
        if let Some(link) = self.link.take() {
            link.close();
        }
        self.set_status(if self.attempt == 0 {
            ConnectionStatus::Connecting
        } else {
            self.status
        });
        self.generation = self.generation.wrapping_add(1);
        match RealmLink::connect(
            &self.endpoints,
            &token,
            self.events.clone(),
            self.myself.clone(),
            self.generation,
        ) {
            Ok(link) => self.link = Some(link),
            Err(error) => {
                log::warn!("network: {}", error.message);
                self.schedule_reconnect();
            }
        }
    }

    fn schedule_reconnect(&mut self) {
        let delay = reconnect_delay(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        self.reconnect_at = Some(Instant::now() + delay);
        self.set_status(ConnectionStatus::Reconnecting {
            attempt: self.attempt,
            retry_in_ms: u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
        });
    }

    fn on_link(&mut self, signal: LinkSignal) {
        match signal {
            LinkSignal::Connected => {
                log::info!("network: realm connected");
            }
            LinkSignal::Subscribed => {
                self.attempt = 0;
                self.set_status(ConnectionStatus::Connected);
                self.clock.reset();
                if let Some(link) = &self.link {
                    for _ in 0..5 {
                        link.request_time(self.myself.clone());
                    }
                }
            }
            LinkSignal::ConnectFailed(reason) | LinkSignal::Disconnected(Some(reason)) => {
                log::warn!("network: realm connection lost: {reason}");
                if let Some(link) = self.link.take() {
                    link.close();
                }
                if self.session.is_some() {
                    self.schedule_reconnect();
                }
            }
            LinkSignal::Disconnected(None) => {
                if let Some(link) = self.link.take() {
                    link.close();
                }
                if self.session.is_some() {
                    self.schedule_reconnect();
                }
            }
            LinkSignal::ServerTime {
                sent,
                received,
                server,
            } => {
                self.clock.record(sent, received, server);
                self.emit(NetEvent::Clock(self.clock));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_off_grows_and_caps() {
        let delays: Vec<u128> = (0..10).map(|n| reconnect_delay(n).as_millis()).collect();
        assert!(delays.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(delays[0], 500);
        assert_eq!(*delays.last().unwrap_or(&0), 15_000);
    }
}
