//! The online game, as the application drives it.
//!
//! ```text
//!   OnlineSession                                          App
//!   ─────────────                                          ───
//!   RealmClient ── NetEvent ──► RealmMirror ──► FrontEndView ──► front-end screens
//!        ▲                          │
//!        │ NetCommand               └──► WorldLink ──► Simulation bodies, extractor poses
//!   FrontEndAction, input frames ◄──────────────────── player input, sim ticks
//! ```
//!
//! This module is the only branch point between the offline and online game. The rest of the
//! application does what it always did — simulate, extract, draw — and asks this module, once
//! a frame, what the realm says: which screen is up, where the other players are, whether the
//! barrier stands, how far the server has acknowledged the local player's input.
//!
//! Authority stays on the server:
//! * the local player is **predicted** by the ordinary simulation and **reconciled** to every
//!   authoritative row (`engine_net::prediction`), so input feels immediate and the server
//!   still decides where the player is;
//! * other players are drawn where the server said they were, interpolated (`engine_net::
//!   interpolation`), never simulated;
//! * monsters are the server's rows, placed and posed, with no client AI;
//! * nothing here can change HP, cooldowns, rewards or the party: those are requests.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use engine_collision::world::{QueryBuffers, StaticId};
use engine_core::math::Vec3;
use engine_game::combat::Faction;
use engine_game::enemy::EnemyKind;
use engine_game::hero::Hero;
use engine_game::movement::{Body, MovementState};
use engine_game::sim::dungeon_exec::EnemySpawnOptions;
use engine_game::world::{EntityDesc, EntityKind};
use engine_game::{EntityId, Simulation};
use engine_net::clock::{ClockSync, local_now};
use engine_net::interpolation::SnapshotBuffer;
use engine_net::model::NoticeLevel;
use engine_net::prediction::Prediction;
use engine_net::{
    Action, ConnectionStatus, GameSession, NetCommand, NetEvent, NetworkClient, RealmClient,
    RealmEndpoints, RealmMirror, StoredSession, TokenStore,
};
use engine_presentation::RemoteFrame;
use engine_presentation::ui::frontend::{
    CharacterCard, DungeonCard, FieldKeys, FrontEndAction, FrontEndUi, FrontEndView, FrontInput,
    MemberRow, PartyCard, QueueCard, ReadyState, Screen, StatusLevel, Toast,
};
use engine_realm::api::CharacterSummary;
use engine_realm::catalog::Catalog;
use engine_realm::instance::{Countdown, CountdownDisplay};
use engine_realm::matchmaking::MatchmakingConfig;
use engine_realm::movement::{BarrierKind, MovementRules, barriers_of};
use engine_realm::party::{PartyState, ReadyResponse};
use engine_realm::{ErrorCategory, Role};
use engine_render_data::ui::{UiDrawList, UiScale};

/// The remembered session, sealed with DPAPI for the current Windows user.
pub struct DpapiTokenStore {
    path: std::path::PathBuf,
}

impl DpapiTokenStore {
    /// A store at `%APPDATA%\mygame\session[-profile].bin`.
    #[must_use]
    pub fn for_profile(profile: Option<&str>) -> Self {
        let base = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("mygame");
        let name = match profile {
            Some(profile) => format!("session-{profile}.bin"),
            None => "session.bin".to_owned(),
        };
        Self {
            path: base.join(name),
        }
    }
}

impl TokenStore for DpapiTokenStore {
    fn load(&self) -> Option<StoredSession> {
        let sealed = std::fs::read(&self.path).ok()?;
        let plain = engine_platform::secure_store::unprotect(&sealed)?;
        let text = String::from_utf8(plain).ok()?;
        let (email, refresh) = text.split_once('\n')?;
        Some(StoredSession {
            refresh_token: refresh.to_owned(),
            email: email.to_owned(),
        })
    }

    fn save(&self, session: &StoredSession) {
        let plain = format!("{}\n{}", session.email, session.refresh_token);
        let Some(sealed) = engine_platform::secure_store::protect(plain.as_bytes()) else {
            return;
        };
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(error) = std::fs::write(&self.path, sealed) {
            log::warn!("online: could not remember the session: {error}");
        }
    }

    fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Development automation: sign in and play to the dungeon's start without input.
#[derive(Debug, Clone, Default)]
pub struct DevScript {
    /// Sign in as this account (registering it first if it is new).
    pub login: Option<(String, String)>,
    /// Then choose this character definition as this role, queue, accept and start.
    pub autoplay: Option<(String, Role)>,
}

#[derive(Debug, Default)]
struct DevProgress {
    registered: bool,
    logged_in_tried: bool,
    queued_for: Option<std::time::Instant>,
    answered: Option<u64>,
    started: Option<u64>,
}

/// What the application should do this frame, beyond drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Directive {
    /// Put this hero's kit and model on the local player.
    SetHero(Hero),
    /// Close the game.
    Quit,
}

/// One other player, as the world shows them.
struct RemoteEntity {
    entity: EntityId,
    buffer: SnapshotBuffer,
    last: Option<(Vec3, f32)>,
    definition: String,
}

/// The realm's instance, mirrored into the local simulation.
#[derive(Default)]
struct WorldLink {
    instance: Option<u64>,
    remotes: BTreeMap<String, RemoteEntity>,
    monsters: BTreeMap<u64, EntityId>,
    barrier: Option<StaticId>,
    prediction: Option<Prediction>,
    pending_jump: bool,
    last_reconciled_seq: u32,
    last_send: Option<Instant>,
    buffers: Option<QueryBuffers>,
}

/// The online game's state.
pub struct OnlineSession {
    client: RealmClient,
    mirror: RealmMirror,
    ui: FrontEndUi,
    catalog: Catalog,
    session: Option<GameSession>,
    status: ConnectionStatus,
    characters: Vec<CharacterSummary>,
    selected_character: Option<usize>,
    selected_dungeon: Option<usize>,
    selected_role: Option<Role>,
    /// Where the player is in the screens the server does not decide.
    stage: Screen,
    busy: Option<String>,
    error: Option<String>,
    clock: ClockSync,
    toasts: VecDeque<(Toast, Instant)>,
    /// When each server notice was first shown; dismissed after [`TOAST_SECONDS`].
    seen_notices: BTreeMap<u64, Instant>,
    /// Notices already dismissed on the server, until their rows go.
    dismissed: BTreeSet<u64>,
    answered_party: Option<u64>,
    world: WorldLink,
    hero: Option<Hero>,
    directives: Vec<Directive>,
    view: FrontEndView,
    events: Vec<NetEvent>,
    /// The origin of the interpolation clock.
    epoch: Instant,
    dev: DevScript,
    dev_progress: DevProgress,
}

/// How long a toast stays.
const TOAST_SECONDS: u64 = 6;

impl OnlineSession {
    /// Starts the network layer and tries the remembered session.
    ///
    /// # Errors
    ///
    /// The endpoints are invalid or the network thread could not start.
    pub fn start(
        endpoints: RealmEndpoints,
        store: Box<dyn TokenStore>,
        dev: DevScript,
    ) -> Result<Self, String> {
        let catalog = Catalog::builtin().map_err(|error| error.to_string())?;
        let local = [&endpoints.spacetimedb_url, &endpoints.auth_api_url, &endpoints.persistence_api_url]
            .iter()
            .all(|url| url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost"));
        if dev.login.is_some() && !local {
            return Err("--dev-login is only accepted against a realm on this machine".into());
        }
        let client = RealmClient::start(endpoints, store).map_err(|error| error.message)?;
        if dev.login.is_none() {
            client.send(NetCommand::Resume);
        }
        log::info!("online: started");
        Ok(Self {
            client,
            mirror: RealmMirror::default(),
            ui: FrontEndUi::new(),
            catalog,
            session: None,
            status: ConnectionStatus::Offline,
            characters: Vec::new(),
            selected_character: None,
            selected_dungeon: Some(0),
            selected_role: None,
            stage: Screen::Login,
            // Resuming a remembered session is a sign-in in progress; a scripted login is not
            // yet.
            busy: dev.login.is_none().then(|| "Signing in...".to_owned()),
            error: None,
            clock: ClockSync::default(),
            toasts: VecDeque::new(),
            seen_notices: BTreeMap::new(),
            dismissed: BTreeSet::new(),
            answered_party: None,
            world: WorldLink::default(),
            hero: None,
            directives: Vec::new(),
            view: FrontEndView::default(),
            events: Vec::new(),
            epoch: Instant::now(),
            dev,
            dev_progress: DevProgress::default(),
        })
    }

    /// Whether the full-screen front-end is up (no world input, no HUD).
    #[must_use]
    pub fn front_end_active(&self) -> bool {
        self.view.screen != Screen::InWorld
    }

    /// Whether the pointer is over online interface.
    #[must_use]
    pub fn wants_pointer(&self, viewport: (u32, u32), scale: UiScale) -> bool {
        self.ui.wants_pointer(&self.view, viewport, scale)
    }

    /// Whether the local player may move: inside an instance that accepts movement.
    #[must_use]
    pub fn may_move(&self) -> bool {
        self.view.screen == Screen::InWorld
            && self
                .mirror
                .instance
                .as_ref()
                .is_some_and(|instance| instance.state.accepts_movement())
            && self.mirror.me().is_some()
    }

    /// Whether the realm has placed the local player in an instance (until then the start
    /// area stands empty behind the front-end).
    #[must_use]
    pub fn player_placed(&self) -> bool {
        self.world.prediction.is_some()
    }

    /// Directives for the application, drained.
    pub fn take_directives(&mut self) -> Vec<Directive> {
        std::mem::take(&mut self.directives)
    }

    /// Drains network events and runs the front-end for one frame.
    pub fn update(
        &mut self,
        input: &engine_core::input::InputState,
        viewport: (u32, u32),
        scale: UiScale,
        delta_seconds: f32,
    ) {
        self.events.clear();
        let mut events = std::mem::take(&mut self.events);
        self.client.poll(&mut events);
        for event in events.drain(..) {
            self.on_event(event);
        }
        self.events = events;
        self.age_notices();
        self.run_dev_script();
        let now = Instant::now();
        self.toasts
            .retain(|(_, at)| now.duration_since(*at) < Duration::from_secs(TOAST_SECONDS));

        self.view = self.build_view();
        use engine_core::input::KeyCode;
        let keys = FieldKeys {
            left: input.was_key_pressed(KeyCode::ArrowLeft),
            right: input.was_key_pressed(KeyCode::ArrowRight),
            home: input.was_key_pressed(KeyCode::Home),
            end: false,
            delete: input.was_key_pressed(KeyCode::Delete),
        };
        let pointer = input.mouse_position();
        #[allow(clippy::cast_precision_loss, reason = "client pixel coordinates")]
        let pointer = (pointer.0 as f32, pointer.1 as f32);
        let clipboard = engine_platform::secure_store::clipboard_text;
        let front_input = FrontInput {
            pointer,
            pressed: input.was_mouse_button_pressed(engine_core::input::MouseButton::Left),
            typed: input.typed(),
            keys,
            escape: input.was_key_pressed(KeyCode::Escape),
            clipboard: &clipboard,
        };
        let actions = self
            .ui
            .update(&front_input, &self.view, viewport, scale, delta_seconds);
        for action in actions {
            self.on_action(action);
        }
        self.view = self.build_view();
    }

    /// Draws the online interface into `list`.
    pub fn draw(&self, list: &mut UiDrawList, viewport: (u32, u32), scale: UiScale) {
        self.ui.draw(list, &self.view, viewport, scale);
    }

    /// Development automation, one step a frame.
    fn run_dev_script(&mut self) {
        if let Some((email, password)) = self.dev.login.clone()
            && self.session.is_none()
            && self.busy.is_none()
        {
            if !self.dev_progress.registered {
                self.dev_progress.registered = true;
                self.client.send(NetCommand::Register {
                    email,
                    password,
                    display_name: None,
                    remember: false,
                });
            } else if !self.dev_progress.logged_in_tried {
                self.dev_progress.logged_in_tried = true;
                self.client.send(NetCommand::Login {
                    email,
                    password,
                    remember: false,
                });
            }
            return;
        }
        let Some((definition, role)) = self.dev.autoplay.clone() else {
            return;
        };
        if self.status != ConnectionStatus::Connected || self.session.is_none() {
            return;
        }
        if self.mirror.ticket.is_none()
            && self.mirror.party.is_none()
            && self.mirror.instance.is_none()
            && self.dev_progress.queued_for.is_none_or(|at| at.elapsed() > Duration::from_secs(5))
            && let Some(index) = self.characters.iter().position(|c| c.definition_id == definition)
        {
            self.selected_character = Some(index);
            self.on_action(FrontEndAction::ConfirmCharacter);
            self.selected_dungeon = Some(0);
            self.on_action(FrontEndAction::ConfirmDungeon);
            self.selected_role = Some(role);
            self.on_action(FrontEndAction::JoinQueue);
            self.dev_progress.queued_for = Some(Instant::now());
        }
        if let Some(party) = self.mirror.party.clone() {
            if party.state == PartyState::ReadyCheck && self.dev_progress.answered != Some(party.id) {
                self.dev_progress.answered = Some(party.id);
                self.on_action(FrontEndAction::Accept);
            }
            if party.state == PartyState::Ready && self.mirror.i_lead() && self.dev_progress.started != Some(party.id) {
                self.dev_progress.started = Some(party.id);
                self.on_action(FrontEndAction::StartDungeon);
            }
        }
    }

    fn toast(&mut self, level: StatusLevel, message: impl Into<String>) {
        self.toasts.push_back((
            Toast {
                level,
                message: message.into(),
            },
            Instant::now(),
        ));
        while self.toasts.len() > 6 {
            self.toasts.pop_front();
        }
    }

    fn on_event(&mut self, event: NetEvent) {
        match event {
            NetEvent::Status(status) => {
                if status == ConnectionStatus::Connected
                    && matches!(self.status, ConnectionStatus::Reconnecting { .. })
                {
                    self.toast(StatusLevel::Good, "Reconnected to the realm.");
                }
                self.status = status;
            }
            NetEvent::SigningIn => {
                self.busy = Some("Signing in...".into());
                self.error = None;
            }
            NetEvent::RememberedEmail(email) => self.ui.set_email(&email),
            NetEvent::SignedIn(session) => {
                self.ui.clear_password();
                self.busy = None;
                self.error = None;
                self.stage = Screen::Characters;
                self.session = Some(session);
                self.client.send(NetCommand::FetchCharacters);
            }
            NetEvent::SignedOut { reason } => {
                self.ui.clear_password();
                self.busy = None;
                self.session = None;
                self.characters.clear();
                self.selected_character = None;
                self.stage = Screen::Login;
                self.error = reason.map(|reason| reason.message);
                self.mirror = RealmMirror::default();
            }
            NetEvent::Characters(characters) => {
                if self.selected_character.is_none() && !characters.is_empty() {
                    self.selected_character = Some(0);
                }
                self.characters = characters;
                self.busy = None;
            }
            NetEvent::Character(_) | NetEvent::Settings(_) | NetEvent::SettingsSaved(_) => {}
            NetEvent::QueueAccepted => self.busy = None,
            NetEvent::Realm(update) => self.mirror.apply(update),
            NetEvent::Clock(clock) => self.clock = clock,
            NetEvent::Failed { action, error } => {
                self.busy = None;
                log::info!("online: {action:?} refused: {}", error.message);
                match action {
                    Action::SignIn => self.error = Some(error.message),
                    _ if error.category == ErrorCategory::Connection => {
                        self.toast(StatusLevel::Warning, error.message);
                    }
                    _ => {
                        self.error = Some(error.message.clone());
                        self.toast(StatusLevel::Bad, error.message);
                    }
                }
            }
        }
    }

    fn on_action(&mut self, action: FrontEndAction) {
        match action {
            FrontEndAction::Login {
                email,
                password,
                remember,
            } => {
                self.error = None;
                self.client.send(NetCommand::Login {
                    email,
                    password,
                    remember,
                });
            }
            FrontEndAction::Register {
                email,
                password,
                display_name,
                remember,
            } => {
                self.error = None;
                self.client.send(NetCommand::Register {
                    email,
                    password,
                    display_name: Some(display_name).filter(|n| !n.is_empty()),
                    remember,
                });
            }
            FrontEndAction::Logout => self.client.send(NetCommand::Logout),
            FrontEndAction::Quit => self.directives.push(Directive::Quit),
            FrontEndAction::SelectCharacter(index) => {
                self.selected_character = Some(index);
                self.selected_role = None;
            }
            FrontEndAction::ConfirmCharacter => {
                if let Some(hero) = self.selected_hero() {
                    self.directives.push(Directive::SetHero(hero));
                    self.hero = Some(hero);
                }
                self.stage = Screen::Dungeons;
                self.error = None;
            }
            FrontEndAction::SelectDungeon(index) => self.selected_dungeon = Some(index),
            FrontEndAction::ConfirmDungeon => {
                self.stage = Screen::Roles;
                self.error = None;
                // One role possible: choose it.
                if let Some(character) = self.selected_character_summary()
                    && character.roles.len() == 1
                {
                    self.selected_role = character.roles.first().copied();
                }
            }
            FrontEndAction::SelectRole(role) => self.selected_role = Some(role),
            FrontEndAction::JoinQueue => {
                let (Some(character), Some(dungeon), Some(role)) = (
                    self.selected_character_summary().map(|c| c.id.clone()),
                    self.selected_dungeon
                        .and_then(|i| self.catalog.dungeons().get(i))
                        .map(|d| d.id.clone()),
                    self.selected_role,
                ) else {
                    return;
                };
                self.error = None;
                self.busy = Some("Joining the queue...".into());
                self.client.send(NetCommand::JoinQueue {
                    character_id: character,
                    dungeon_id: dungeon,
                    role,
                });
            }
            FrontEndAction::Back => {
                self.error = None;
                self.stage = match self.stage {
                    Screen::Roles => Screen::Dungeons,
                    _ => Screen::Characters,
                };
            }
            FrontEndAction::LeaveQueue => self.client.send(NetCommand::LeaveQueue),
            FrontEndAction::Accept => {
                self.answered_party = self.mirror.party.as_ref().map(|p| p.id);
                self.client.send(NetCommand::RespondReadyCheck(true));
            }
            FrontEndAction::Decline => {
                self.answered_party = self.mirror.party.as_ref().map(|p| p.id);
                self.client.send(NetCommand::RespondReadyCheck(false));
            }
            FrontEndAction::StartDungeon => {
                self.busy = Some("Starting...".into());
                self.client.send(NetCommand::StartDungeon);
            }
            FrontEndAction::Kick(account) => self.client.send(NetCommand::Kick(account)),
            FrontEndAction::MakeLeader(account) => {
                self.client.send(NetCommand::TransferLeader(account));
            }
            FrontEndAction::LeaveParty => {
                self.client.send(NetCommand::LeaveParty);
                self.stage = Screen::Dungeons;
            }
        }
    }

    fn selected_character_summary(&self) -> Option<&CharacterSummary> {
        self.selected_character.and_then(|i| self.characters.get(i))
    }

    fn selected_hero(&self) -> Option<Hero> {
        let summary = self.selected_character_summary()?;
        self.catalog
            .character(&summary.definition_id)
            .map(|definition| definition.hero)
    }

    fn server_now(&self) -> i64 {
        self.clock.server_now(local_now())
    }

    /// Which screen the realm's state puts the player on.
    fn screen(&self) -> Screen {
        if self.session.is_none() {
            return Screen::Login;
        }
        if self.mirror.instance.is_some() {
            return Screen::InWorld;
        }
        if let Some(party) = &self.mirror.party {
            return match party.state {
                PartyState::ReadyCheck => Screen::ReadyCheck,
                PartyState::Ready => Screen::Party,
                _ => Screen::InWorld,
            };
        }
        if self.mirror.ticket.is_some() {
            return Screen::Queue;
        }
        match self.stage {
            Screen::Login => Screen::Characters,
            other => other,
        }
    }

    fn build_view(&self) -> FrontEndView {
        let screen = self.screen();
        let connection = match self.status {
            ConnectionStatus::Connected => Some((StatusLevel::Good, "Connected to the realm".to_owned())),
            ConnectionStatus::Connecting => Some((StatusLevel::Busy, "Connecting to the realm...".to_owned())),
            ConnectionStatus::Reconnecting { attempt, .. } => Some((
                StatusLevel::Warning,
                format!("Reconnecting (attempt {attempt})"),
            )),
            ConnectionStatus::Offline => Some((StatusLevel::Bad, "Not connected".to_owned())),
        };
        let characters = self
            .characters
            .iter()
            .map(|c| CharacterCard {
                name: c.name.clone(),
                class: c.class.clone(),
                roles: c.roles.clone(),
                level: c.level,
                experience: c.experience,
                next_level: engine_realm::progression::experience_to_next(c.level),
                gear_score: c.gear_score,
                equipped: c.equipment.len(),
            })
            .collect();
        let dungeons = self
            .catalog
            .dungeons()
            .iter()
            .map(|d| {
                let population = self.mirror.population.get(&d.id).copied().unwrap_or_default();
                DungeonCard {
                    name: d.name.clone(),
                    description: d.description.clone(),
                    difficulty: d.difficulty.name().to_owned(),
                    levels: (d.min_level, d.max_level),
                    recommended_gear_score: d.recommended_gear_score,
                    min_gear_score: d.min_gear_score,
                    rewards: d.rewards.clone(),
                    population: [population.tanks, population.healers, population.dps],
                }
            })
            .collect();
        let dungeon_name = |id: &str| {
            self.catalog
                .dungeon(id)
                .map_or_else(|| id.to_owned(), |d| d.name.clone())
        };
        let now = self.server_now();
        let queue = self.mirror.ticket.as_ref().map(|ticket| {
            let waited = (now - ticket.enqueued_at).max(0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a percentage")]
            let tolerance = (MatchmakingConfig::default().tolerance(waited) * 100.0).round() as u32;
            QueueCard {
                dungeon: dungeon_name(&ticket.dungeon_id),
                role: ticket.role,
                character: ticket.character_name.clone(),
                waited_seconds: u64::try_from(waited / engine_realm::SECOND).unwrap_or(0),
                tolerance_percent: tolerance,
            }
        });
        let me = self.mirror.my_account_id().map(str::to_owned);
        let party = self.mirror.party.as_ref().map(|party| {
            let members = self
                .mirror
                .members
                .iter()
                .map(|m| MemberRow {
                    account_id: m.account_id.clone(),
                    character: m.character_name.clone(),
                    player: m.display_name.clone(),
                    role: m.role,
                    level: m.level,
                    gear_score: m.gear_score,
                    ready: match m.ready {
                        ReadyResponse::Pending => ReadyState::Waiting,
                        ReadyResponse::Accepted => ReadyState::Ready,
                        ReadyResponse::Declined => ReadyState::Declined,
                    },
                    present: m.present,
                    leader: party.leader.as_deref() == Some(m.account_id.as_str()),
                    me: me.as_deref() == Some(m.account_id.as_str()),
                })
                .collect();
            let left = (party.ready_deadline - now).max(0);
            let timeout = engine_realm::party::PartyConfig::default().ready_check_timeout;
            #[allow(clippy::cast_precision_loss, reason = "microsecond spans")]
            let fraction = left as f32 / timeout.max(1) as f32;
            PartyCard {
                dungeon: dungeon_name(&party.dungeon_id),
                members,
                i_lead: self.mirror.i_lead(),
                i_answered: self.answered_party == Some(party.id)
                    || self
                        .mirror
                        .my_membership()
                        .is_some_and(|m| m.ready != ReadyResponse::Pending),
                seconds_left: u64::try_from(left / engine_realm::SECOND).unwrap_or(0),
                fraction_left: fraction,
            }
        });
        let countdown = self.mirror.instance.as_ref().and_then(|instance| {
            let ends = instance.countdown_ends_at?;
            match (Countdown { ends_at: ends }).display(now) {
                CountdownDisplay::Hidden => None,
                shown => Some(shown),
            }
        });
        let reconnecting = match self.status {
            ConnectionStatus::Reconnecting { .. } if self.session.is_some() => {
                Some("Reconnecting to the game server".to_owned())
            }
            _ => None,
        };
        let mut toasts: Vec<Toast> = self.toasts.iter().map(|(t, _)| t.clone()).collect();
        let now_instant = Instant::now();
        for notice in &self.mirror.notices {
            let fresh = self
                .seen_notices
                .get(&notice.id)
                .is_none_or(|at| now_instant.saturating_duration_since(*at) < Duration::from_secs(TOAST_SECONDS));
            if !fresh || self.dismissed.contains(&notice.id) {
                continue;
            }
            toasts.push(Toast {
                level: match notice.level {
                    NoticeLevel::Info => StatusLevel::Good,
                    NoticeLevel::Warning => StatusLevel::Warning,
                    NoticeLevel::Error => StatusLevel::Bad,
                },
                message: notice.message.clone(),
            });
        }
        let toasts = toasts.into_iter().rev().take(4).rev().collect();
        FrontEndView {
            screen,
            connection,
            account: self.session.as_ref().map(|s| s.display_name.clone()),
            busy: self.busy.clone(),
            error: self.error.clone(),
            characters,
            selected_character: self.selected_character,
            dungeons,
            selected_dungeon: self.selected_dungeon,
            selected_role: self.selected_role,
            queue,
            party,
            countdown,
            reconnecting,
            toasts,
        }
    }

    /// Shows each server notice for a few seconds, then dismisses it on the server.
    fn age_notices(&mut self) {
        let now = Instant::now();
        let live: BTreeSet<u64> = self.mirror.notices.iter().map(|n| n.id).collect();
        for id in &live {
            self.seen_notices.entry(*id).or_insert(now);
        }
        let expired: Vec<u64> = self
            .seen_notices
            .iter()
            .filter(|(id, at)| {
                live.contains(id)
                    && !self.dismissed.contains(id)
                    && now.duration_since(**at) >= Duration::from_secs(TOAST_SECONDS)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            self.client.send(NetCommand::DismissNotice(id));
            self.dismissed.insert(id);
        }
        self.seen_notices.retain(|id, _| live.contains(id));
        self.dismissed.retain(|id| live.contains(id));
    }

    // --- the world -------------------------------------------------------------------

    /// Mirrors the realm's instance into the local simulation: the local player placed and
    /// reconciled, other players and monsters spawned and posed, the barrier standing or not.
    #[allow(clippy::too_many_lines, reason = "one pass over the instance, in order")]
    pub fn sync_world(
        &mut self,
        simulation: &mut Simulation,
        extractor: &mut engine_presentation::RenderExtractor,
        visual_for: &dyn Fn(Hero) -> Option<engine_presentation::character::CharacterVisual>,
    ) {
        let now_seconds = self.epoch.elapsed().as_secs_f64();
        let instance = self.mirror.instance.clone();
        let Some(instance) = instance else {
            if self.world.instance.is_some() {
                self.leave_world(simulation, extractor);
            }
            return;
        };

        // Entering.
        if self.world.instance != Some(instance.id) {
            self.leave_world(simulation, extractor);
            self.world.instance = Some(instance.id);
            log::info!("online: entering instance {}", instance.id);
            if let Some(me) = self.mirror.me() {
                place_player(simulation, me.position, me.yaw);
                self.world.prediction = Some(Prediction::new(me.last_seq));
                self.world.last_reconciled_seq = me.last_seq;
            }
            let hero = self
                .mirror
                .me()
                .and_then(|me| self.catalog.character(&me.definition_id))
                .map(|d| d.hero);
            if let Some(hero) = hero
                && self.hero != Some(hero)
            {
                self.hero = Some(hero);
                self.directives.push(Directive::SetHero(hero));
            }
        }
        if self.world.prediction.is_none()
            && let Some(me) = self.mirror.me()
        {
            place_player(simulation, me.position, me.yaw);
            self.world.prediction = Some(Prediction::new(me.last_seq));
            self.world.last_reconciled_seq = me.last_seq;
        }

        // The barrier.
        let closed = !instance.barrier_open;
        if closed && self.world.barrier.is_none() {
            if let Some(dungeon) = self.catalog.dungeon(&instance.dungeon_id).cloned() {
                let scene = engine_environment::EnvironmentScene::empty("barrier");
                let start = barriers_of(&scene, &dungeon)
                    .into_iter()
                    .find(|b| b.kind == BarrierKind::Start);
                if let Some(barrier) = start {
                    let world = simulation.collision_world_mut();
                    self.world.barrier = Some(world.add_box(barrier.min, barrier.max, "start barrier"));
                    world.rebuild_broad_phase();
                }
            }
        } else if !closed && let Some(id) = self.world.barrier.take() {
            let world = simulation.collision_world_mut();
            world.remove_static(id);
            world.rebuild_broad_phase();
        }

        // Other players.
        let me = self.mirror.my_account_id().map(str::to_owned);
        let others: Vec<engine_net::model::RemotePlayerState> = self.mirror.others().cloned().collect();
        let present: BTreeSet<String> = others.iter().map(|p| p.account_id.clone()).collect();
        let gone: Vec<String> = self
            .world
            .remotes
            .keys()
            .filter(|account| !present.contains(*account))
            .cloned()
            .collect();
        for account in gone {
            if let Some(remote) = self.world.remotes.remove(&account) {
                extractor.remove_remote_character(remote.entity);
                simulation.world_mut().despawn(remote.entity);
            }
        }
        for other in &others {
            if Some(&other.account_id) == me.as_ref() {
                continue;
            }
            let remote = self.world.remotes.entry(other.account_id.clone()).or_insert_with(|| {
                let entity = simulation.world_mut().spawn(EntityDesc {
                    kind: EntityKind::HealingDummy,
                    faction: Faction::Player,
                    position: other.position,
                    max_health: other.max_health.max(1.0),
                    radius: engine_core::tuning::player::RADIUS,
                    height: engine_core::tuning::player::HEIGHT,
                });
                RemoteEntity {
                    entity,
                    buffer: SnapshotBuffer::default(),
                    last: None,
                    definition: String::new(),
                }
            });
            if remote.definition != other.definition_id {
                remote.definition.clone_from(&other.definition_id);
                if let Some(hero) = self.catalog.character(&other.definition_id).map(|d| d.hero)
                    && let Some(visual) = visual_for(hero)
                {
                    extractor.add_remote_character(remote.entity, visual);
                }
            }
            if remote.last != Some((other.position, other.yaw)) {
                remote.last = Some((other.position, other.yaw));
                remote.buffer.push(now_seconds, other.position, other.yaw);
            }
            let (position, yaw) = remote
                .buffer
                .sample(now_seconds)
                .unwrap_or((other.position, other.yaw));
            if let Some(body) = simulation.world_mut().body_mut(remote.entity) {
                body.position = position;
                body.velocity = other.velocity;
                body.facing_yaw = yaw;
                body.grounded = other.grounded;
            }
            extractor.set_remote_frame(
                remote.entity,
                RemoteFrame {
                    position,
                    facing_yaw: yaw,
                    velocity: other.velocity,
                    grounded: other.grounded,
                    dead: other.health <= 0.0,
                    opacity: if other.present { 1.0 } else { 0.45 },
                },
            );
        }

        // Monsters: the server's rows, posed. No client AI decides anything about them.
        let monsters = self.mirror.monsters.clone();
        let gone: Vec<u64> = self
            .world
            .monsters
            .keys()
            .filter(|id| !monsters.contains_key(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(entity) = self.world.monsters.remove(&id) {
                simulation.world_mut().despawn(entity);
            }
        }
        for (id, monster) in &monsters {
            if !self.world.monsters.contains_key(id) {
                let Some(kind) = EnemyKind::from_id(&monster.archetype) else {
                    continue;
                };
                let options = EnemySpawnOptions {
                    level: u8::try_from(monster.level.clamp(1, 255)).unwrap_or(1),
                    aggro_radius: Some(0.0),
                    size: monster.scale.max(0.1),
                    name: Some(monster.name.clone()),
                    ..EnemySpawnOptions::default()
                };
                let entity = simulation.spawn_enemy_with(kind, monster.position, monster.yaw, &options);
                self.world.monsters.insert(*id, entity);
            }
            if let Some(entity) = self.world.monsters.get(id)
                && let Some(body) = simulation.world_mut().body_mut(*entity)
            {
                body.position = monster.position;
                body.velocity = Vec3::ZERO;
                body.facing_yaw = monster.yaw;
            }
        }

        // The local player: reconcile to the latest authoritative row.
        if let Some(me) = self.mirror.me().cloned()
            && me.last_seq != self.world.last_reconciled_seq
            && let Some(prediction) = self.world.prediction.as_mut()
        {
            self.world.last_reconciled_seq = me.last_seq;
            let rules = MovementRules {
                movement: simulation.config().movement,
                character: simulation.config().character,
            };
            let current = simulation.player_body().copied().unwrap_or(Body::at(me.position));
            let authoritative = Body {
                position: me.position,
                velocity: me.velocity,
                grounded: me.grounded,
                ground_normal: me.ground_normal,
                facing_yaw: me.yaw,
                state: MovementState::Idle,
                support_height: 0.0,
            };
            let buffers = self.world.buffers.get_or_insert_with(QueryBuffers::new);
            let corrected = prediction.reconcile(
                &current,
                authoritative,
                me.last_seq,
                &rules,
                simulation.collision_world(),
                buffers,
            );
            let error = (corrected.position - current.position).length();
            if error > 0.01 {
                log::debug!("online: reconciled {error:.3} m");
            }
            let entity = simulation.player().entity;
            if let Some(body) = simulation.world_mut().body_mut(entity) {
                body.position = corrected.position;
                body.velocity = corrected.velocity;
                body.grounded = corrected.grounded;
                body.ground_normal = corrected.ground_normal;
            }
        }
    }

    /// Records the movement the simulation just applied, one frame per tick, and sends what
    /// is due. `ticks` is what `Simulation::advance` returned; `intent` the frame's movement.
    pub fn record_ticks(&mut self, ticks: u32, intent: Option<(f32, f32, f32, bool)>, jump: bool) {
        if jump {
            self.world.pending_jump = true;
        }
        let Some(prediction) = self.world.prediction.as_mut() else {
            return;
        };
        let (forward, right, yaw, face) = intent.unwrap_or((0.0, 0.0, 0.0, false));
        for _ in 0..ticks {
            let jump = std::mem::take(&mut self.world.pending_jump);
            prediction.record(forward, right, yaw, face, jump);
        }
        let due = self
            .world
            .last_send
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(33));
        if due && prediction.outbox_len() > 0 {
            while prediction.outbox_len() > 0 {
                let frames = prediction.take_outbox();
                self.client.send(NetCommand::Input(frames));
            }
            self.world.last_send = Some(Instant::now());
        }
    }

    fn leave_world(&mut self, simulation: &mut Simulation, extractor: &mut engine_presentation::RenderExtractor) {
        for (_, remote) in std::mem::take(&mut self.world.remotes) {
            extractor.remove_remote_character(remote.entity);
            simulation.world_mut().despawn(remote.entity);
        }
        for (_, entity) in std::mem::take(&mut self.world.monsters) {
            simulation.world_mut().despawn(entity);
        }
        if let Some(id) = self.world.barrier.take() {
            let world = simulation.collision_world_mut();
            world.remove_static(id);
            world.rebuild_broad_phase();
        }
        if self.world.instance.is_some() {
            log::info!("online: left the instance");
        }
        self.world = WorldLink::default();
    }
}

/// Puts the local player at `position`, at rest.
fn place_player(simulation: &mut Simulation, position: Vec3, yaw: f32) {
    let entity = simulation.player().entity;
    if let Some(body) = simulation.world_mut().body_mut(entity) {
        *body = Body::at(position);
        body.facing_yaw = yaw;
    }
}
