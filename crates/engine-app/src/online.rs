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
//! * monsters are the server's rows, placed, posed and interpolated, with no client AI;
//! * the fight is the server's: the simulation runs under a remote authority
//!   (`Simulation::set_remote_authority`), so an ability press becomes a `combat_act`
//!   request, and health, casts, cooldowns, resources and statuses are laid into the world
//!   from the server's rows, its events injected for effects, numbers and sound;
//! * nothing here can change HP, cooldowns, rewards or the party: those are requests.
//!
//! Outside an instance the player stands in the **hub**: the dungeon level's start area with
//! training dummies the local simulation spawns, under *local* authority (the offline combat
//! code, nothing sent to the server). The hub is torn down before an instance is entered and
//! rebuilt when it is left; see [`OnlineSession::sync_hub`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use engine_collision::world::{QueryBuffers, StaticId};
use engine_core::math::Vec3;
use engine_game::arena::{ArenaConfig, spawn_group, spawn_healing_dummy};
use engine_game::character::{ItemIcon, ItemQuality};
use engine_game::combat::Faction;
use engine_game::enemy::{EnemyAbility, EnemyKind};
use engine_game::event::CombatEvent;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::movement::{Body, MovementState};
use engine_game::sim::dungeon_exec::EnemySpawnOptions;
use engine_game::sim::remote::{PresentedResources, PresentedStatus};
use engine_game::status::StatusKind;
use engine_game::world::{EntityDesc, EntityKind};
use engine_game::{EntityId, Simulation};
use engine_net::clock::{ClockSync, local_now};
use engine_net::interpolation::SnapshotBuffer;
use engine_net::model::NoticeLevel;
use engine_net::model::RunSummaryInfo;
use engine_net::prediction::Prediction;
use engine_net::{
    Action, ConnectionStatus, GameSession, NetCommand, NetEvent, NetworkClient, RealmClient,
    RealmEndpoints, RealmMirror, StoredSession, TokenStore,
};
use engine_presentation::RemoteFrame;
use engine_presentation::ui::frontend::{
    BagRow, CharacterCard, CompletionCard, DungeonCard, ExchangeRow, FieldKeys, FrontEndAction,
    FrontEndUi, FrontEndView, FrontInput, HubCard, IntroCard, KeyCard, MemberRow, PartyCard,
    QueueCard, ReadyState, Screen, StatusLevel, Toast, WorkshopCard,
};
use engine_realm::api::{CharacterSummary, EconomyInfo};
use engine_realm::catalog::Catalog;
use engine_realm::combat::{AbilityRef, ActionKind, CombatAction, EntityRef, StatusState};
use engine_realm::instance::{Countdown, CountdownDisplay, InstanceState};
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
    key_sent: Option<u64>,
    started: Option<u64>,
    /// When the run in front of the player ended, to return to the hub a moment later.
    run_ended: Option<Instant>,
    /// When the hub was last found idle, to queue again a moment later.
    idle_since: Option<Instant>,
}

/// What the application should do this frame, beyond drawing.
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    /// Put this hero's kit and model on the local player.
    SetHero(Hero),
    /// Close the game.
    Quit,
    /// Apply the account's synced settings document over the local settings.
    ApplyAccountSettings(serde_json::Value),
    /// Dress this hero's sheet in the character's stored loadout.
    ApplyLoadout(Hero, engine_realm::loadout::Loadout),
    /// Open the character sheet (the hub's Equipment button).
    OpenCharacterMenu,
    /// Set this hero's talents to the persistent character's: its saved keys, and every point
    /// it has (starting plus earned).
    ApplyTalents(Hero, Vec<String>, u32),
}

/// One other player, as the world shows them.
struct RemoteEntity {
    entity: EntityId,
    buffer: SnapshotBuffer,
    last: Option<(Vec3, f32)>,
    definition: String,
}

/// One monster, as the world shows it.
struct MonsterEntity {
    entity: EntityId,
    buffer: SnapshotBuffer,
    last: Option<(Vec3, f32)>,
}

/// The realm's instance, mirrored into the local simulation.
#[derive(Default)]
struct WorldLink {
    instance: Option<u64>,
    remotes: BTreeMap<String, RemoteEntity>,
    monsters: BTreeMap<u64, MonsterEntity>,
    barrier: Option<StaticId>,
    prediction: Option<Prediction>,
    pending_jump: bool,
    last_reconciled_seq: u32,
    last_authoritative: Option<(Vec3, f32)>,
    last_send: Option<Instant>,
    buffers: Option<QueryBuffers>,
    /// The run was won while the player was in it, so unclaimed loot is mailed on leaving.
    completed: bool,
}

/// The hub's contents in the local simulation: what to take down when it is left.
struct HubWorld {
    /// The training dummies and the friendly dummy.
    entities: Vec<EntityId>,
    /// The dungeon's start barrier, standing so the hub is bounded.
    barrier: Option<StaticId>,
}

/// Which of the two levels the online client has loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnlineLevel {
    /// The hub scene, `assets/catalog/hub.json`.
    Hub,
    /// The dungeon the instance runs in.
    Dungeon,
}

/// Where the hub scene puts the player and the practice dummies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HubSpawns {
    /// Where the player arrives.
    pub player: Vec3,
    /// Which way they face, radians.
    pub yaw: f32,
    /// The lone dummy.
    pub featured: Vec3,
    /// Centre of the group of three.
    pub group_center: Vec3,
    /// Metres between the group's dummies.
    pub group_spacing: f32,
}

impl HubSpawns {
    /// Reads the spawns off a hub scene, each lifted onto the scene's ground so a terrain
    /// under the hub does not bury them.
    #[must_use]
    pub fn from_scene(scene: &engine_environment::EnvironmentScene) -> Self {
        let player = scene.player_spawn();
        let grounded = |position: Vec3| {
            Vec3::new(
                position.x,
                position.y.max(scene.ground_at(position)),
                position.z,
            )
        };
        Self {
            player: player.position,
            yaw: player.yaw,
            featured: grounded(scene.spawns.featured_dummy),
            group_center: grounded(scene.spawns.group_center),
            group_spacing: scene.spawns.group_spacing,
        }
    }
}

/// What a pending request is waiting for, so it can be cleared when the server has answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyKind {
    /// Loading the character list after signing in.
    Characters,
    /// Joining the queue: the API's answer.
    JoinQueue,
    /// Leaving the queue: the ticket going away.
    LeaveQueue,
    /// Inserting the key: the party leaving `ChoosingKey`.
    InsertKey,
    /// Withdrawing the key: the party leaving `Ready`.
    WithdrawKey,
    /// Starting the dungeon: the instance appearing.
    Start,
    /// Leaving the party: the party going away.
    LeaveParty,
    /// A salvage or an exchange: the API's answer.
    Workshop,
    /// Signing out.
    SignOut,
    /// Signing in.
    SignIn,
}

/// A request in flight, as the interface shows it.
struct Busy {
    kind: BusyKind,
    text: String,
    since: Instant,
}

/// What a character looked like when a run began, to tell when its result has been saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CharacterMark {
    level: u32,
    experience: u64,
    key_level: u32,
    resources: u64,
}

/// Waiting for the API to apply a finished run's result to the character.
struct SavingRewards {
    /// The character before the run.
    before: CharacterMark,
    /// Re-fetches sent so far.
    attempt: usize,
    /// When the next is due.
    next: Instant,
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
    busy: Option<Busy>,
    error: Option<String>,
    /// The title screen's words.
    intro: IntroCard,
    /// The economy catalog, fetched once after signing in.
    economy: Option<EconomyInfo>,
    /// The hub in the local simulation, while the player stands in it.
    hub: Option<HubWorld>,
    /// Where the hub's scene puts things, once the client has loaded it. Without it the hub
    /// is the dungeon's start area.
    hub_spawns: Option<HubSpawns>,
    /// The character as it was when the current run began.
    run_start: Option<CharacterMark>,
    /// The instance whose Return to Hub was pressed.
    returning: Option<u64>,
    /// A finished run's result, not yet seen on the stored character.
    saving: Option<SavingRewards>,
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
    /// The account settings: the stored version, what was last stored, what is in flight.
    settings_version: Option<i64>,
    settings_synced: Option<serde_json::Value>,
    settings_in_flight: Option<serde_json::Value>,
    /// The selected character's stored loadout, as the API last sent it.
    stored_loadout: Option<(Hero, engine_realm::loadout::Loadout)>,
    /// The bag cells put in the Workshop's salvage tray, in the order they were put there.
    salvage_tray: Vec<u32>,
    /// Chest loot (or its mail) the bridge may not have persisted yet.
    loot_watch: Option<LootWatch>,
}

/// Re-fetching the character after chest loot, until the item shows up in the bag or mailbox.
struct LootWatch {
    /// Bag stacks plus letters before the loot; more than this means the item arrived.
    held_before: usize,
    /// Fetches sent so far.
    attempt: usize,
    /// When the next fetch is due.
    next: Instant,
}

/// Seconds after the loot event (or leaving a won run) of each re-fetch of the character. The
/// bridge collects loot every few seconds, so the item is usually stored by the second.
const LOOT_FETCH_DELAYS: [u64; 4] = [4, 6, 10, 15];

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
        let local = [
            &endpoints.spacetimedb_url,
            &endpoints.auth_api_url,
            &endpoints.persistence_api_url,
        ]
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
            // The title comes first, once per launch.
            stage: Screen::Title,
            // Resuming a remembered session is a sign-in in progress; a scripted login is not
            // yet.
            busy: dev.login.is_none().then(|| Busy {
                kind: BusyKind::SignIn,
                text: "Signing in...".to_owned(),
                since: Instant::now(),
            }),
            error: None,
            intro: parse_intro(INTRO_JSON),
            economy: None,
            hub: None,
            hub_spawns: None,
            run_start: None,
            returning: None,
            saving: None,
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
            settings_version: None,
            settings_synced: None,
            settings_in_flight: None,
            stored_loadout: None,
            salvage_tray: Vec::new(),
            loot_watch: None,
        })
    }

    /// Which level the client should have loaded: the dungeon while the realm has an instance
    /// for the player, the hub scene otherwise.
    #[must_use]
    pub fn wanted_level(&self) -> OnlineLevel {
        if self.mirror.instance.is_some() {
            OnlineLevel::Dungeon
        } else {
            OnlineLevel::Hub
        }
    }

    /// Tells the session where the hub scene puts the player and the dummies. Once set, the
    /// hub is built there and has no barrier of its own: the scene's boundary bounds it.
    pub fn set_hub_spawns(&mut self, spawns: HubSpawns) {
        self.hub_spawns = Some(spawns);
    }

    /// The level's collision world was rebuilt, which clears every static the session
    /// added. The handles it kept now name nothing (or, worse, a new shape in a recycled
    /// slot), so they are dropped rather than removed later.
    pub fn level_rebuilt(&mut self) {
        self.world.barrier = None;
        if let Some(hub) = self.hub.as_mut() {
            hub.barrier = None;
        }
    }

    /// Whether the full-screen front-end is up (no world input, no HUD). The hub and the
    /// instance are the playable world with overlays, not front-end pages.
    #[must_use]
    pub fn front_end_active(&self) -> bool {
        !self.view.screen.is_overlay()
    }

    /// Whether the player stands in the hub: the local simulation is the authority here, so
    /// the application lets every local command through.
    #[must_use]
    pub fn in_hub(&self) -> bool {
        self.view.screen == Screen::Hub
    }

    /// Whether the pointer is over online interface.
    #[must_use]
    pub fn wants_pointer(&self, viewport: (u32, u32), scale: UiScale) -> bool {
        self.ui.wants_pointer(&self.view, viewport, scale)
    }

    /// Whether the local player may move: in the hub, or inside an instance that accepts
    /// movement.
    #[must_use]
    pub fn may_move(&self) -> bool {
        match self.view.screen {
            Screen::Hub => self.hub.is_some(),
            Screen::InWorld => {
                self.mirror
                    .instance
                    .as_ref()
                    .is_some_and(|instance| instance.state.accepts_movement())
                    && self.mirror.me().is_some()
            }
            _ => false,
        }
    }

    /// Whether the local player stands somewhere: in the hub, or placed by the realm in an
    /// instance (until then the start area stands empty behind the front-end).
    #[must_use]
    pub fn player_placed(&self) -> bool {
        self.hub.is_some() || self.world.prediction.is_some()
    }

    /// Offers the current account settings document; uploads it when it differs from what
    /// the account has stored. Called every few seconds, not every frame.
    pub fn observe_settings(&mut self, document: serde_json::Value) {
        let Some(version) = self.settings_version else {
            return;
        };
        if self.session.is_none() || self.settings_in_flight.is_some() {
            return;
        }
        if self.settings_synced.as_ref() == Some(&document) {
            return;
        }
        self.settings_in_flight = Some(document.clone());
        self.client.send(NetCommand::SaveSettings(
            engine_realm::api::SettingsDocument {
                version,
                settings: document,
            },
        ));
    }

    /// Asks the API to move an item on the selected character. The server validates it and
    /// answers with the new loadout; nothing changes locally until it does.
    pub fn request_move(
        &mut self,
        from: engine_game::character::ItemSlot,
        to: engine_game::character::ItemSlot,
    ) {
        use engine_game::character::ItemSlot;
        use engine_realm::loadout::SlotRef;
        let to_ref = |slot: ItemSlot| match slot {
            ItemSlot::Equipment(equip) => SlotRef::Equipment {
                slot: equip.key().to_owned(),
            },
            ItemSlot::Inventory(index) => SlotRef::Inventory {
                index: u32::try_from(index).unwrap_or(u32::MAX),
            },
        };
        let Some(character_id) = self.selected_character_summary().map(|c| c.id.clone()) else {
            return;
        };
        self.client.send(NetCommand::MoveItem {
            character_id,
            from: to_ref(from),
            to: to_ref(to),
        });
    }

    /// Whether the realm has the account queued, in a party or in an instance. While it does,
    /// talents and equipment cannot change (the API refuses too).
    #[must_use]
    pub fn is_engaged(&self) -> bool {
        self.mirror.is_engaged()
    }

    /// Asks the API to replace the selected character's talents. The answer is the character,
    /// which re-applies them; a refusal re-fetches it so the menu shows what is stored.
    pub fn save_talents(&mut self, talents: Vec<String>) {
        if let Some(character_id) = self.selected_character_summary().map(|c| c.id.clone()) {
            self.client.send(NetCommand::SaveTalents {
                character_id,
                talents,
            });
        }
    }

    /// Takes mail `mail_id`'s item into the bag, on the persistent character. The API answers
    /// with the character, which dresses the local sheet; a full bag is refused there.
    pub fn take_mail(&mut self, mail_id: u64) {
        self.send_mail(mail_id, true);
    }

    /// Marks mail `mail_id` read, on the persistent character.
    pub fn read_mail(&mut self, mail_id: u64) {
        self.send_mail(mail_id, false);
    }

    fn send_mail(&mut self, mail_id: u64, take: bool) {
        if let Some(character_id) = self.selected_character_summary().map(|c| c.id.clone()) {
            self.client.send(NetCommand::Mail {
                character_id,
                mail_id,
                take,
            });
        }
    }

    /// Directives for the application, drained.
    pub fn take_directives(&mut self) -> Vec<Directive> {
        std::mem::take(&mut self.directives)
    }

    /// Drains network events and runs the front-end for one frame. `modal`: a window of the
    /// game (the character sheet, the pause menu) is over the interface and owns the mouse.
    pub fn update(
        &mut self,
        input: &engine_core::input::InputState,
        viewport: (u32, u32),
        scale: UiScale,
        delta_seconds: f32,
        modal: bool,
    ) {
        self.events.clear();
        let mut events = std::mem::take(&mut self.events);
        self.client.poll(&mut events);
        for event in events.drain(..) {
            self.on_event(event);
        }
        self.events = events;
        self.age_notices();
        self.poll_loot_watch();
        self.poll_saving();
        self.settle_busy();
        self.adopt_session_state();
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
            pressed: input.was_mouse_button_pressed(engine_core::input::MouseButton::Left)
                && !modal,
            typed: input.typed(),
            keys,
            escape: input.was_key_pressed(KeyCode::Escape) && !modal,
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
        // Nobody is at the keyboard to press Continue.
        if self.stage == Screen::Title && (self.dev.login.is_some() || self.dev.autoplay.is_some())
        {
            self.on_action(FrontEndAction::ContinueFromTitle);
        }
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

        // A finished run: look at it for a moment, then return to the hub.
        let run_over = self.mirror.instance.as_ref().is_some_and(|instance| {
            matches!(
                instance.state,
                InstanceState::Completed | InstanceState::Failed
            ) && self
                .mirror
                .run_summary
                .as_ref()
                .is_some_and(|summary| summary.instance_id == instance.id)
        });
        if run_over && self.returning.is_none() {
            let ended = *self.dev_progress.run_ended.get_or_insert_with(Instant::now);
            if ended.elapsed() > Duration::from_secs(3) {
                self.on_action(FrontEndAction::ReturnToHub);
            }
        } else if !run_over {
            self.dev_progress.run_ended = None;
        }

        // In the hub with nothing going on: queue again after a few seconds.
        let idle = self.mirror.ticket.is_none()
            && self.mirror.party.is_none()
            && self.mirror.instance.is_none()
            && self.saving.is_none()
            && self.busy.is_none()
            && self.stage != Screen::Title;
        if idle {
            let since = *self
                .dev_progress
                .idle_since
                .get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_secs(3)
                && self
                    .dev_progress
                    .queued_for
                    .is_none_or(|at| at.elapsed() > Duration::from_secs(5))
                && let Some(index) = self
                    .characters
                    .iter()
                    .position(|c| c.definition_id == definition)
            {
                if matches!(self.stage, Screen::Characters | Screen::Login) {
                    self.selected_character = Some(index);
                    self.on_action(FrontEndAction::ConfirmCharacter);
                }
                self.on_action(FrontEndAction::OpenDungeonFinder);
                self.selected_dungeon = Some(0);
                self.on_action(FrontEndAction::ConfirmDungeon);
                self.selected_role = Some(role);
                self.on_action(FrontEndAction::JoinQueue);
                self.dev_progress.queued_for = Some(Instant::now());
            }
        } else {
            self.dev_progress.idle_since = None;
        }

        if let Some(party) = self.mirror.party.clone() {
            if party.state == PartyState::ReadyCheck && self.dev_progress.answered != Some(party.id)
            {
                self.dev_progress.answered = Some(party.id);
                self.on_action(FrontEndAction::Accept);
            }
            // Every client offers its own key, once; the first to arrive wins and the others
            // are refused, which is expected.
            if party.state == PartyState::ChoosingKey
                && self.dev_progress.key_sent != Some(party.id)
            {
                self.dev_progress.key_sent = Some(party.id);
                self.on_action(FrontEndAction::InsertKey);
            }
            if party.state == PartyState::Ready
                && self.mirror.i_lead()
                && self.dev_progress.started != Some(party.id)
            {
                self.dev_progress.started = Some(party.id);
                self.on_action(FrontEndAction::StartDungeon);
            }
        }
    }

    /// Sets the request the interface waits on.
    fn set_busy(&mut self, kind: BusyKind, text: &str) {
        self.busy = Some(Busy {
            kind,
            text: text.to_owned(),
            since: Instant::now(),
        });
    }

    /// Clears a request the realm has visibly answered, or one nothing answered for too long.
    fn settle_busy(&mut self) {
        let Some(busy) = &self.busy else { return };
        let settled = busy_settled(busy.kind, &self.mirror);
        let stuck = busy.kind != BusyKind::SignIn && busy.since.elapsed() > BUSY_TIMEOUT;
        if settled || stuck {
            if stuck && !settled {
                self.toast(
                    StatusLevel::Warning,
                    "The server is taking a while to answer.",
                );
            }
            self.busy = None;
        }
    }

    /// A restored session may already hold a ticket or a party: land on that character's hub
    /// instead of the list.
    fn adopt_session_state(&mut self) {
        if self.stage != Screen::Characters || self.characters.is_empty() {
            return;
        }
        let name = self
            .mirror
            .ticket
            .as_ref()
            .map(|ticket| ticket.character_name.clone())
            .or_else(|| {
                self.mirror
                    .my_membership()
                    .map(|member| member.character_name.clone())
            });
        let Some(name) = name else { return };
        if let Some(index) = self.characters.iter().position(|c| c.name == name) {
            self.selected_character = Some(index);
            self.on_action(FrontEndAction::ConfirmCharacter);
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
                self.set_busy(BusyKind::SignIn, "Signing in...");
                self.error = None;
            }
            NetEvent::RememberedEmail(email) => self.ui.set_email(&email),
            NetEvent::SignedIn(session) => {
                self.ui.clear_password();
                self.error = None;
                // The title is shown once per launch, whenever the session turns up.
                if self.stage != Screen::Title {
                    self.stage = Screen::Characters;
                }
                self.session = Some(session);
                self.set_busy(BusyKind::Characters, "Loading characters...");
                self.client.send(NetCommand::FetchCharacters);
                self.client.send(NetCommand::FetchSettings);
                self.client.send(NetCommand::FetchEconomy);
            }
            NetEvent::SignedOut { reason } => {
                self.ui.clear_password();
                self.busy = None;
                self.session = None;
                self.characters.clear();
                self.selected_character = None;
                self.economy = None;
                self.saving = None;
                self.returning = None;
                self.run_start = None;
                self.stored_loadout = None;
                self.salvage_tray.clear();
                self.loot_watch = None;
                // Logging out lands on the sign-in form; only the first page is the title.
                if self.stage != Screen::Title {
                    self.stage = Screen::Login;
                }
                self.error = reason.map(|reason| reason.message);
                self.mirror = RealmMirror::default();
            }
            NetEvent::Characters(characters) => {
                if self.selected_character.is_none() && !characters.is_empty() {
                    self.selected_character = Some(0);
                }
                if self
                    .selected_character
                    .is_some_and(|index| index >= characters.len())
                {
                    self.selected_character = None;
                }
                self.characters = characters;
                self.clear_busy(BusyKind::Characters);
                self.check_saving();
            }
            NetEvent::Economy(economy) => self.economy = Some(*economy),
            NetEvent::Settings(document) => {
                self.settings_version = Some(document.version);
                if document.version > 0 && document.settings.is_object() {
                    log::info!("online: account settings v{} applied", document.version);
                    self.settings_synced = Some(document.settings.clone());
                    self.directives
                        .push(Directive::ApplyAccountSettings(document.settings));
                }
            }
            NetEvent::SettingsSaved(version) => {
                self.settings_version = Some(version);
                self.settings_synced = self.settings_in_flight.take();
            }
            NetEvent::Character(detail) => {
                let hero = self
                    .catalog
                    .character(&detail.summary.definition_id)
                    .map(|definition| definition.hero);
                if let Some(hero) = hero {
                    let loadout = engine_realm::loadout::Loadout {
                        equipment: detail.summary.equipment.clone(),
                        inventory: detail.inventory.clone(),
                        mail: detail.mail.clone(),
                        ..Default::default()
                    };
                    // The item has arrived once the bag and mailbox hold more than they did.
                    if self.loot_watch.as_ref().is_some_and(|watch| {
                        loadout.inventory.len() + loadout.mail.len() > watch.held_before
                    }) {
                        self.loot_watch = None;
                    }
                    // A salvage empties the cells it broke down; an exchange leaves the tray be.
                    self.salvage_tray.retain(|index| {
                        loadout
                            .inventory
                            .iter()
                            .any(|entry| entry.index == *index && entry.quantity > 0)
                    });
                    self.stored_loadout = Some((hero, loadout.clone()));
                    self.directives.push(Directive::ApplyLoadout(hero, loadout));
                    self.directives.push(Directive::ApplyTalents(
                        hero,
                        detail.talents.clone(),
                        detail.talent_points_total,
                    ));
                }
                // Keep the list's copy in step (gear score, level after a result).
                if let Some(entry) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.id == detail.summary.id)
                {
                    *entry = detail.summary.clone();
                }
                self.clear_busy(BusyKind::Workshop);
                self.check_saving();
            }
            NetEvent::QueueAccepted => self.clear_busy(BusyKind::JoinQueue),
            NetEvent::Realm(update) => self.mirror.apply(update),
            NetEvent::Clock(clock) => self.clock = clock,
            NetEvent::Failed {
                action: Action::Settings,
                error,
            } => {
                // Another machine saved first, or the API is down: start again from what is
                // stored rather than overwrite it.
                log::info!("online: settings not synced: {}", error.message);
                self.settings_in_flight = None;
                self.settings_version = None;
                self.client.send(NetCommand::FetchSettings);
            }
            // The catalog bundled with the game names the resources until the API answers.
            NetEvent::Failed {
                action: Action::Economy,
                error,
            } => log::info!("online: economy not loaded: {}", error.message),
            // A re-fetch while the result is being saved: the schedule tries again.
            NetEvent::Failed {
                action: Action::Characters,
                error,
            } if self.saving.is_some() => {
                log::info!("online: character not refreshed: {}", error.message);
            }
            NetEvent::Failed { action, error } => {
                self.busy = None;
                log::info!("online: {action:?} refused: {}", error.message);
                if action == Action::Talents
                    && let Some(id) = self.selected_character_summary().map(|c| c.id.clone())
                {
                    // Show what is stored, not what was optimistically bought.
                    self.client.send(NetCommand::FetchCharacter(id));
                }
                if action == Action::LeaveParty {
                    // Still in the run: the panel's button works again.
                    self.returning = None;
                    self.saving = None;
                }
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

    /// Clears the busy text if it is waiting for `kind`.
    fn clear_busy(&mut self, kind: BusyKind) {
        if self.busy.as_ref().is_some_and(|busy| busy.kind == kind) {
            self.busy = None;
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per action the interface offers"
    )]
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
            FrontEndAction::ContinueFromTitle => {
                if self.stage == Screen::Title {
                    self.stage = if self.session.is_some() {
                        Screen::Characters
                    } else {
                        Screen::Login
                    };
                }
            }
            FrontEndAction::Logout => {
                self.set_busy(BusyKind::SignOut, "Signing out...");
                self.client.send(NetCommand::Logout);
            }
            FrontEndAction::Quit => self.directives.push(Directive::Quit),
            FrontEndAction::SelectCharacter(index) => {
                self.selected_character = Some(index);
                self.selected_role = None;
            }
            FrontEndAction::ChangeCharacter => {
                if self.can_change_character() {
                    self.selected_role = None;
                    self.error = None;
                    self.stage = Screen::Characters;
                }
            }
            FrontEndAction::ConfirmCharacter => {
                if let Some(id) = self.selected_character_summary().map(|c| c.id.clone()) {
                    // The persistent character, once: its loadout dresses the local sheet.
                    self.stored_loadout = None;
                    self.client.send(NetCommand::FetchCharacter(id));
                }
                if let Some(hero) = self.selected_hero() {
                    self.directives.push(Directive::SetHero(hero));
                    self.hero = Some(hero);
                }
                self.stage = Screen::Hub;
                self.error = None;
            }
            FrontEndAction::OpenDungeonFinder => {
                if self.can_find_dungeon() {
                    self.stage = Screen::Dungeons;
                    self.error = None;
                }
            }
            FrontEndAction::OpenEquipment => self.directives.push(Directive::OpenCharacterMenu),
            FrontEndAction::OpenWorkshop => {
                self.stage = Screen::Workshop;
                self.error = None;
                self.salvage_tray.clear();
                // The bag as the server has it, and the catalog if it never arrived.
                if let Some(id) = self.selected_character_summary().map(|c| c.id.clone()) {
                    self.client.send(NetCommand::FetchCharacter(id));
                }
                if self.economy.is_none() {
                    self.client.send(NetCommand::FetchEconomy);
                }
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
                self.set_busy(BusyKind::JoinQueue, "Joining the queue...");
                // Back to the hub: the queue panel shows once the ticket exists.
                self.stage = Screen::Hub;
                self.client.send(NetCommand::JoinQueue {
                    character_id: character,
                    dungeon_id: dungeon,
                    role,
                });
            }
            FrontEndAction::Back => {
                self.error = None;
                self.salvage_tray.clear();
                self.stage = match self.stage {
                    Screen::Roles => Screen::Dungeons,
                    Screen::Dungeons | Screen::Workshop => Screen::Hub,
                    other => other,
                };
            }
            FrontEndAction::LeaveQueue => {
                self.set_busy(BusyKind::LeaveQueue, "Leaving the queue...");
                self.client.send(NetCommand::LeaveQueue);
            }
            FrontEndAction::Accept => {
                self.answered_party = self.mirror.party.as_ref().map(|p| p.id);
                self.client.send(NetCommand::RespondReadyCheck(true));
            }
            FrontEndAction::Decline => {
                self.answered_party = self.mirror.party.as_ref().map(|p| p.id);
                self.client.send(NetCommand::RespondReadyCheck(false));
            }
            FrontEndAction::InsertKey => {
                self.set_busy(BusyKind::InsertKey, "Inserting your key...");
                self.client.send(NetCommand::InsertKey);
            }
            FrontEndAction::WithdrawKey => {
                self.set_busy(BusyKind::WithdrawKey, "Withdrawing your key...");
                self.client.send(NetCommand::WithdrawKey);
            }
            FrontEndAction::StartDungeon => {
                self.set_busy(BusyKind::Start, "Starting dungeon...");
                self.client.send(NetCommand::StartDungeon);
            }
            FrontEndAction::Kick(account) => self.client.send(NetCommand::Kick(account)),
            FrontEndAction::LeaveParty => {
                self.set_busy(BusyKind::LeaveParty, "Leaving the party...");
                self.client.send(NetCommand::LeaveParty);
                self.stage = Screen::Hub;
            }
            FrontEndAction::ReturnToHub => self.return_to_hub(),
            FrontEndAction::StageSalvage(index) => {
                if !self.salvage_tray.contains(&index)
                    && self.salvage_tray.len() < self.salvage_slots()
                    && self.bag_has(index)
                {
                    self.salvage_tray.push(index);
                }
            }
            FrontEndAction::UnstageSalvage(index) => self.salvage_tray.retain(|i| *i != index),
            FrontEndAction::SalvageTray => {
                if self.salvage_tray.is_empty() {
                    return;
                }
                if let Some(character_id) = self.selected_character_summary().map(|c| c.id.clone())
                {
                    self.set_busy(BusyKind::Workshop, "Salvaging...");
                    self.client.send(NetCommand::Salvage {
                        character_id,
                        indices: self.salvage_tray.clone(),
                    });
                }
            }
            FrontEndAction::Exchange(exchange_id) => {
                if let Some(character_id) = self.selected_character_summary().map(|c| c.id.clone())
                {
                    self.set_busy(BusyKind::Workshop, "Exchanging...");
                    self.client.send(NetCommand::Exchange {
                        character_id,
                        exchange_id,
                        times: 1,
                    });
                }
            }
        }
    }

    /// Whether the Dungeon Finder may be opened: not while searching, grouped or saving.
    fn can_find_dungeon(&self) -> bool {
        self.mirror.ticket.is_none()
            && self.mirror.party.is_none()
            && self.saving.is_none()
            && self.busy.is_none()
    }

    /// Whether the character may be changed: not while searching or grouped.
    fn can_change_character(&self) -> bool {
        self.mirror.ticket.is_none() && self.mirror.party.is_none() && self.busy.is_none()
    }

    /// Return to Hub on the completion panel: leave the party, and wait for the run's result to
    /// show on the stored character. Never blocks: the wait ends with the fetch schedule.
    fn return_to_hub(&mut self) {
        let Some(instance) = self.mirror.instance.as_ref() else {
            return;
        };
        if !matches!(
            instance.state,
            InstanceState::Completed | InstanceState::Failed
        ) || self.returning.is_some()
        {
            return;
        }
        self.returning = Some(instance.id);
        let earned = self
            .mirror
            .run_summary
            .as_ref()
            .filter(|summary| summary.instance_id == instance.id)
            .is_some_and(expects_rewards);
        let now_mark = self.selected_character_summary().map(character_mark);
        // The character before the run, unless the result has already landed on it.
        let before = self.run_start.or(now_mark);
        if earned
            && let (Some(before), Some(now)) = (before, now_mark)
            && before == now
        {
            self.saving = Some(SavingRewards {
                before,
                attempt: 0,
                next: Instant::now() + Duration::from_secs(LOOT_FETCH_DELAYS[0]),
            });
            self.send_character_fetches();
        }
        self.client.send(NetCommand::LeaveParty);
    }

    /// Asks the API for the selected character and for the list.
    fn send_character_fetches(&self) {
        if let Some(id) = self.selected_character_summary().map(|c| c.id.clone()) {
            self.client.send(NetCommand::FetchCharacter(id));
        }
        self.client.send(NetCommand::FetchCharacters);
    }

    /// Re-fetches the character on the saving schedule, and gives up with a toast.
    fn poll_saving(&mut self) {
        let now = Instant::now();
        let Some(attempt) = self
            .saving
            .as_ref()
            .filter(|saving| now >= saving.next)
            .map(|saving| saving.attempt)
        else {
            return;
        };
        self.send_character_fetches();
        if let Some(delay) = LOOT_FETCH_DELAYS.get(attempt + 1) {
            if let Some(saving) = self.saving.as_mut() {
                saving.attempt += 1;
                saving.next = now + Duration::from_secs(*delay);
            }
        } else {
            self.saving = None;
            self.toast(
                StatusLevel::Warning,
                "Rewards are being saved; they will appear shortly.",
            );
        }
    }

    /// Ends the wait for a run's result once the stored character shows it.
    fn check_saving(&mut self) {
        let landed = self.saving.as_ref().is_some_and(|saving| {
            self.selected_character_summary()
                .map(character_mark)
                .is_some_and(|mark| mark != saving.before)
        });
        if landed {
            self.saving = None;
            self.toast(StatusLevel::Good, "Rewards saved.");
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
        route(self.session.is_some(), self.stage, &self.mirror)
    }

    /// A resource's display name: the economy catalog's, else the bundled one, else its id.
    fn resource_name(&self, id: &str) -> String {
        self.economy
            .as_ref()
            .and_then(|economy| economy.resources.iter().find(|r| r.id == id))
            .map(|resource| resource.name.clone())
            .or_else(|| {
                self.catalog
                    .economy()
                    .resource(id)
                    .map(|resource| resource.name.clone())
            })
            .unwrap_or_else(|| id.to_owned())
    }

    fn dungeon_name(&self, id: &str) -> String {
        self.catalog
            .dungeon(id)
            .map_or_else(|| id.to_owned(), |d| d.name.clone())
    }

    /// A character's resources with their display names.
    fn named_resources(&self, held: &[(String, u64)]) -> Vec<(String, u64)> {
        held.iter()
            .filter(|(_, amount)| *amount > 0)
            .map(|(id, amount)| (self.resource_name(id), *amount))
            .collect()
    }

    fn key_card(&self, character: &CharacterSummary) -> Option<KeyCard> {
        character.key.as_ref().map(|key| KeyCard {
            dungeon: if key.dungeon_name.is_empty() {
                self.dungeon_name(&key.dungeon_id)
            } else {
                key.dungeon_name.clone()
            },
            level: key.level,
        })
    }

    /// The hub panel for the selected character.
    fn hub_card(&self) -> Option<HubCard> {
        let character = self.selected_character_summary()?;
        Some(HubCard {
            name: character.name.clone(),
            class: character.class.clone(),
            level: character.level,
            experience: character.experience,
            next_level: engine_realm::progression::experience_to_next(character.level),
            key: self.key_card(character),
            resources: self.named_resources(&character.resources),
            can_find_dungeon: self.can_find_dungeon(),
            can_change_character: self.can_change_character(),
        })
    }

    /// How many cells the salvage tray has: the economy catalog's, else the bundled one's.
    fn salvage_slots(&self) -> usize {
        let slots = self.economy.as_ref().map_or_else(
            || self.catalog.economy().salvage.slots,
            |economy| economy.salvage.slots,
        );
        slots as usize
    }

    /// Whether bag cell `index` holds something.
    fn bag_has(&self, index: u32) -> bool {
        self.stored_loadout.as_ref().is_some_and(|(_, loadout)| {
            loadout
                .inventory
                .iter()
                .any(|entry| entry.index == index && entry.quantity > 0)
        })
    }

    /// The salvage and exchange page for the selected character.
    fn workshop_card(&self) -> WorkshopCard {
        let Some(character) = self.selected_character_summary() else {
            return WorkshopCard::default();
        };
        let held = |id: &str| {
            character
                .resources
                .iter()
                .find(|(resource, _)| resource == id)
                .map_or(0, |(_, amount)| *amount)
        };
        let Some(economy) = self.economy.as_ref() else {
            return WorkshopCard {
                resources: self.named_resources(&character.resources),
                ..WorkshopCard::default()
            };
        };
        let rule = &economy.salvage;
        let bag: Vec<BagRow> = self
            .stored_loadout
            .as_ref()
            .map(|(_, loadout)| {
                loadout
                    .inventory
                    .iter()
                    .filter(|entry| entry.quantity > 0)
                    .map(|entry| {
                        let item = engine_game::character::item_by_key(&entry.item);
                        let level = item.map_or(0, |def| def.item_level);
                        let amount = (u64::from(rule.base)
                            + u64::from(rule.per_item_level) * u64::from(level))
                            * u64::from(entry.quantity);
                        BagRow {
                            index: entry.index,
                            name: item
                                .map_or_else(|| entry.item.clone(), |def| def.name.to_owned()),
                            quantity: entry.quantity,
                            item_level: level,
                            icon: item.map_or(ItemIcon::Trinket, |def| def.icon),
                            quality: item.map_or(ItemQuality::Common, |def| def.quality),
                            yields: format!("+{amount} {}", self.resource_name(&rule.resource)),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Only cells that still hold something; the yield is the sum of their stacks.
        let tray: Vec<u32> = self
            .salvage_tray
            .iter()
            .copied()
            .filter(|index| bag.iter().any(|row| row.index == *index))
            .collect();
        let tray_total: u64 = tray
            .iter()
            .filter_map(|index| bag.iter().find(|row| row.index == *index))
            .map(|row| {
                let level = u64::from(row.item_level);
                (u64::from(rule.base) + u64::from(rule.per_item_level) * level)
                    * u64::from(row.quantity)
            })
            .sum();
        let tray_yield = if tray.is_empty() {
            String::new()
        } else {
            format!("+{tray_total} {}", self.resource_name(&rule.resource))
        };
        let exchanges = economy
            .exchanges
            .iter()
            .map(|exchange| ExchangeRow {
                id: exchange.id.clone(),
                label: format!(
                    "{} {} for {} {}",
                    exchange.from_amount,
                    self.resource_name(&exchange.from),
                    exchange.to_amount,
                    self.resource_name(&exchange.to)
                ),
                affordable: held(&exchange.from) >= u64::from(exchange.from_amount),
            })
            .collect();
        WorkshopCard {
            bag,
            tray,
            tray_slots: self.salvage_slots(),
            tray_yield,
            resources: self.named_resources(&character.resources),
            exchanges,
        }
    }

    /// The loot line of the completion panel.
    fn loot_line(&self, summary: &RunSummaryInfo, state: InstanceState) -> Option<String> {
        if !summary.success {
            return None;
        }
        if !summary.loot_item.is_empty() {
            return Some(
                engine_game::character::item_by_key(&summary.loot_item)
                    .map_or_else(|| summary.loot_item.clone(), |def| def.name.to_owned()),
            );
        }
        // The chest can be opened while the instance lingers.
        (state == InstanceState::Completed).then(|| "Open the chest to claim your loot".to_owned())
    }

    /// The text of the request in flight, or of the wait the realm itself shows.
    fn busy_text(&self, screen: Screen) -> Option<String> {
        if let Some(busy) = &self.busy {
            return Some(busy.text.clone());
        }
        // Between the start and the instance appearing; once it has, the world shows it.
        if let Some(party) = &self.mirror.party
            && self.mirror.instance.is_none()
        {
            match party.state {
                PartyState::Starting => return Some("Starting dungeon...".to_owned()),
                PartyState::InDungeon => return Some("Entering the dungeon...".to_owned()),
                _ => {}
            }
        }
        (screen == Screen::Hub && self.saving.is_some()).then(|| "Saving rewards...".to_owned())
    }

    fn build_view(&self) -> FrontEndView {
        let screen = self.screen();
        let connection = match self.status {
            ConnectionStatus::Connected => {
                Some((StatusLevel::Good, "Connected to the realm".to_owned()))
            }
            ConnectionStatus::Connecting => {
                Some((StatusLevel::Busy, "Connecting to the realm...".to_owned()))
            }
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
                key: self.key_card(c),
                resources: c.resources.iter().map(|(_, amount)| *amount).sum(),
            })
            .collect();
        let dungeons = self
            .catalog
            .dungeons()
            .iter()
            .map(|d| {
                let population = self
                    .mirror
                    .population
                    .get(&d.id)
                    .copied()
                    .unwrap_or_default();
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
        let now = self.server_now();
        let queue = self.mirror.ticket.as_ref().map(|ticket| {
            // Real time in the queue: the server's clock now, less when the ticket was made.
            let waited = (now - ticket.enqueued_at).max(0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a percentage"
            )]
            let tolerance = (MatchmakingConfig::default().tolerance(waited) * 100.0).round() as u32;
            QueueCard {
                dungeon: self.dungeon_name(&ticket.dungeon_id),
                role: ticket.role,
                character: ticket.character_name.clone(),
                waited_seconds: u64::try_from(waited / engine_realm::SECOND).unwrap_or(0),
                tolerance_percent: tolerance,
                matched: ticket.matched,
            }
        });
        let me = self.mirror.my_account_id().map(str::to_owned);
        let party = self.mirror.party.as_ref().map(|party| {
            let owner = party.key_owner.as_deref();
            let members = self
                .mirror
                .members
                .iter()
                .map(|m| MemberRow {
                    account_id: m.account_id.clone(),
                    character: m.character_name.clone(),
                    player: m.display_name.clone(),
                    class: self
                        .catalog
                        .character(&m.definition_id)
                        .map(|definition| definition.class.clone())
                        .unwrap_or_default(),
                    role: m.role,
                    level: m.level,
                    gear_score: m.gear_score,
                    key_level: (m.key_dungeon_id == party.dungeon_id && m.key_level >= 1)
                        .then_some(m.key_level),
                    ready: match m.ready {
                        ReadyResponse::Pending => ReadyState::Waiting,
                        ReadyResponse::Accepted => ReadyState::Ready,
                        ReadyResponse::Declined => ReadyState::Declined,
                    },
                    present: m.present,
                    leader: party.leader.as_deref() == Some(m.account_id.as_str()),
                    owner: owner == Some(m.account_id.as_str()),
                    me: me.as_deref() == Some(m.account_id.as_str()),
                })
                .collect();
            let left = (party.ready_deadline - now).max(0);
            let timeout = engine_realm::party::PartyConfig::default().ready_check_timeout;
            #[allow(clippy::cast_precision_loss, reason = "microsecond spans")]
            let fraction = left as f32 / timeout.max(1) as f32;
            PartyCard {
                dungeon: self.dungeon_name(&party.dungeon_id),
                description: self
                    .catalog
                    .dungeon(&party.dungeon_id)
                    .map(|d| d.description.clone())
                    .unwrap_or_default(),
                key_level: party.key_level,
                key_owner: owner.and_then(|account| {
                    self.mirror
                        .members
                        .iter()
                        .find(|m| m.account_id == account)
                        .map(|m| m.character_name.clone())
                }),
                i_own_key: owner.is_some() && owner == me.as_deref(),
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
        let completion = self
            .mirror
            .instance
            .as_ref()
            .filter(|instance| {
                matches!(
                    instance.state,
                    InstanceState::Completed | InstanceState::Failed
                )
            })
            .and_then(|instance| {
                let summary = self
                    .mirror
                    .run_summary
                    .as_ref()
                    .filter(|summary| summary.instance_id == instance.id)?;
                Some(CompletionCard {
                    success: summary.success,
                    dungeon: summary.dungeon_name.clone(),
                    key_level: summary.key_level,
                    key_owner: summary.key_owner,
                    key_level_after: summary.key_level_after,
                    duration_seconds: u64::try_from(summary.duration_us / engine_realm::SECOND)
                        .unwrap_or(0),
                    experience: summary.experience,
                    resources: summary
                        .resources
                        .iter()
                        .map(|(id, amount)| (self.resource_name(id), *amount))
                        .collect(),
                    loot: self.loot_line(summary, instance.state),
                    returning: self.returning == Some(instance.id),
                })
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
            let fresh = self.seen_notices.get(&notice.id).is_none_or(|at| {
                now_instant.saturating_duration_since(*at) < Duration::from_secs(TOAST_SECONDS)
            });
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
            busy: self.busy_text(screen),
            error: self.error.clone(),
            characters,
            selected_character: self.selected_character,
            intro: self.intro.clone(),
            hub: (screen == Screen::Hub).then(|| self.hub_card()).flatten(),
            workshop: (screen == Screen::Workshop).then(|| self.workshop_card()),
            completion,
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

    /// Re-fetches the character while a run's item is still on its way to the stored bag.
    fn poll_loot_watch(&mut self) {
        let Some(watch) = self.loot_watch.as_mut() else {
            return;
        };
        let now = Instant::now();
        if now < watch.next {
            return;
        }
        watch.attempt += 1;
        let last = watch.attempt >= LOOT_FETCH_DELAYS.len();
        if !last {
            watch.next = now + Duration::from_secs(LOOT_FETCH_DELAYS[watch.attempt]);
        }
        if last {
            self.loot_watch = None;
        }
        if let Some(id) = self.selected_character_summary().map(|c| c.id.clone()) {
            self.client.send(NetCommand::FetchCharacter(id));
        }
    }

    /// Starts re-fetching the character until `held_before` is exceeded. The bridge collects
    /// loot every few seconds, so the item is usually stored by the second fetch.
    fn watch_loot(&mut self, held_before: usize) {
        self.loot_watch = Some(LootWatch {
            held_before,
            attempt: 0,
            next: Instant::now() + Duration::from_secs(LOOT_FETCH_DELAYS[0]),
        });
    }

    fn held_now(&self) -> usize {
        self.stored_loadout.as_ref().map_or(0, |(_, loadout)| {
            loadout.inventory.len() + loadout.mail.len()
        })
    }

    /// The server gave the local player `key` (`mailed`: by mail, with `reason`, rather than
    /// into the bag). The stored character does not have it yet, so it is put in the local one
    /// at once; the re-fetch then replaces the lot with what is stored.
    fn on_loot(&mut self, key: &str, mailed: Option<&str>) {
        let held_before = self.held_now();
        if let Some((hero, mut loadout)) = self.stored_loadout.clone() {
            let placed = match mailed {
                Some(reason) => {
                    engine_realm::loadout::add_to_mail(&mut loadout, key, reason, unix_seconds())
                        .is_ok()
                }
                None => engine_realm::loadout::add_to_bag(&mut loadout, key).is_ok(),
            };
            if placed {
                self.stored_loadout = Some((hero, loadout.clone()));
                self.directives.push(Directive::ApplyLoadout(hero, loadout));
            }
        }
        self.watch_loot(held_before);
    }

    // --- the world -------------------------------------------------------------------

    /// Mirrors the realm's instance into the local simulation: the local player placed and
    /// reconciled, other players and monsters spawned and posed, the barrier standing or not.
    #[allow(
        clippy::too_many_lines,
        reason = "one pass over the instance, in order"
    )]
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
            // Out of an instance the player stands in the hub.
            self.sync_hub(simulation);
            return;
        };

        // Entering.
        if self.world.instance != Some(instance.id) {
            // The hub's dummies and barrier are gone before the server's world is laid in.
            self.leave_hub(simulation);
            self.leave_world(simulation, extractor);
            self.world.instance = Some(instance.id);
            self.run_start = self.selected_character_summary().map(character_mark);
            log::info!("online: entering instance {}", instance.id);
            // The server runs this fight; the local simulation predicts movement and
            // presents what the server decides.
            simulation.set_remote_authority(true);
            // The offline level's training dummies are not part of a realm dungeon: every
            // entity here besides the local player comes from the server.
            let player = simulation.player().entity;
            let strays: Vec<EntityId> = (0..simulation.world().slot_count())
                .filter_map(|slot| simulation.world().id_at(slot))
                .filter(|id| *id != player)
                .collect();
            for id in strays {
                simulation.party_mut().leave(id);
                simulation.despawn_enemy(id);
            }
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
        self.world.completed |= instance.state == InstanceState::Completed;
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
                    self.world.barrier =
                        Some(world.add_box(barrier.min, barrier.max, "start barrier"));
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
        let others: Vec<engine_net::model::RemotePlayerState> =
            self.mirror.others().cloned().collect();
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
                simulation.party_mut().leave(remote.entity);
                simulation.world_mut().despawn(remote.entity);
            }
        }
        for other in &others {
            if Some(&other.account_id) == me.as_ref() {
                continue;
            }
            let remote = self
                .world
                .remotes
                .entry(other.account_id.clone())
                .or_insert_with(|| {
                    let entity = simulation.world_mut().spawn(EntityDesc {
                        kind: EntityKind::HealingDummy,
                        faction: Faction::Player,
                        position: other.position,
                        max_health: other.max_health.max(1.0),
                        radius: engine_core::tuning::player::RADIUS,
                        height: engine_core::tuning::player::HEIGHT,
                    });
                    simulation.world_mut().set_label(entity, &other.name);
                    simulation.party_mut().join(entity);
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
            simulation.present_health(remote.entity, other.health, other.max_health);
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

        // Monsters: the server's rows, interpolated. No client AI decides anything about them.
        let monsters = self.mirror.monsters.clone();
        let gone: Vec<u64> = self
            .world
            .monsters
            .keys()
            .filter(|key| !monsters.contains_key(key))
            .copied()
            .collect();
        for key in gone {
            if let Some(monster) = self.world.monsters.remove(&key) {
                simulation.despawn_enemy(monster.entity);
            }
        }
        for (key, monster) in &monsters {
            if !self.world.monsters.contains_key(key) {
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
                let entity =
                    simulation.spawn_enemy_with(kind, monster.position, monster.yaw, &options);
                self.world.monsters.insert(
                    *key,
                    MonsterEntity {
                        entity,
                        buffer: SnapshotBuffer::default(),
                        last: None,
                    },
                );
            }
            let Some(entry) = self.world.monsters.get_mut(key) else {
                continue;
            };
            if entry.last != Some((monster.position, monster.yaw)) {
                entry.last = Some((monster.position, monster.yaw));
                entry
                    .buffer
                    .push(now_seconds, monster.position, monster.yaw);
            }
            let (position, yaw) = entry
                .buffer
                .sample(now_seconds)
                .unwrap_or((monster.position, monster.yaw));
            let entity = entry.entity;
            if let Some(body) = simulation.world_mut().body_mut(entity) {
                body.position = position;
                body.velocity = Vec3::ZERO;
                body.facing_yaw = yaw;
            }
            simulation.present_health(entity, monster.health, monster.max_health);
        }
        self.present_combat(simulation);

        // The local player: reconcile to the latest authoritative row.
        // On a new acknowledgement, and also when the server moved the player itself — a
        // respawn, a Blink, a charge, a knock-back — which changes the row but not the
        // sequence number. Replaying the unacknowledged frames from there is right in both.
        if let Some(me) = self.mirror.me().cloned()
            && (me.last_seq != self.world.last_reconciled_seq
                || self.world.last_authoritative != Some((me.position, me.yaw)))
            && let Some(prediction) = self.world.prediction.as_mut()
        {
            self.world.last_reconciled_seq = me.last_seq;
            self.world.last_authoritative = Some((me.position, me.yaw));
            let rules = MovementRules {
                movement: simulation.config().movement,
                character: simulation.config().character,
            };
            let current = simulation
                .player_body()
                .copied()
                .unwrap_or(Body::at(me.position));
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

    /// The entity the wire's `reference` names in this world.
    fn resolve(&self, simulation: &Simulation, reference: &EntityRef) -> Option<EntityId> {
        match reference {
            EntityRef::None => None,
            EntityRef::Player(account) if Some(account.as_str()) == self.mirror.my_account_id() => {
                Some(simulation.player().entity)
            }
            EntityRef::Player(account) => self.world.remotes.get(account).map(|r| r.entity),
            EntityRef::Monster(key) => self.world.monsters.get(key).map(|m| m.entity),
        }
    }

    /// How the wire names `entity`.
    fn name_of(&self, simulation: &Simulation, entity: EntityId) -> EntityRef {
        if entity == simulation.player().entity {
            return self
                .mirror
                .my_account_id()
                .map_or(EntityRef::None, |me| EntityRef::Player(me.to_owned()));
        }
        if let Some((account, _)) = self.world.remotes.iter().find(|(_, r)| r.entity == entity) {
            return EntityRef::Player(account.clone());
        }
        self.world
            .monsters
            .iter()
            .find(|(_, m)| m.entity == entity)
            .map_or(EntityRef::None, |(key, _)| EntityRef::Monster(*key))
    }

    fn presented_statuses(
        &self,
        simulation: &Simulation,
        statuses: &[StatusState],
        age: f32,
    ) -> Vec<PresentedStatus> {
        statuses
            .iter()
            .filter_map(|status| {
                Some(PresentedStatus {
                    kind: *StatusKind::ALL.get(usize::from(status.kind))?,
                    source: self
                        .resolve(simulation, &status.source)
                        .unwrap_or(EntityId::INVALID),
                    remaining: (status.remaining - age).max(0.0),
                    duration: status.duration,
                    stacks: status.stacks,
                })
            })
            .collect()
    }

    /// Lays the server's fight into the world: health and statuses on every body, what each
    /// monster is doing, the local player's cast, cooldowns and resources; then this pass's
    /// events, and the ability presses the player made, sent as requests.
    fn present_combat(&mut self, simulation: &mut Simulation) {
        // Players.
        let mine = self.mirror.my_account_id().map(str::to_owned);
        if let Some(me) = self.mirror.me() {
            let entity = simulation.player().entity;
            simulation.present_health(entity, me.health, me.max_health);
        }
        let combat: Vec<_> = self.mirror.combat.values().cloned().collect();
        for row in &combat {
            let reference = EntityRef::Player(row.account_id.clone());
            let Some(entity) = self.resolve(simulation, &reference) else {
                continue;
            };
            let age = row.age();
            let statuses = self.presented_statuses(simulation, &row.statuses, age);
            simulation.present_statuses(entity, &statuses);
            // Every player's shields, not only ours: party and raid frames draw them too.
            simulation.present_absorb(entity, row.resources.absorb);
            if Some(&row.account_id) != mine.as_ref() {
                continue;
            }
            let cast = row.cast.and_then(|cast| match cast.ability {
                AbilityRef::Player(index) => FireAbility::ALL
                    .get(usize::from(index))
                    .map(|ability| (*ability, cast.elapsed + age, cast.duration)),
                _ => None,
            });
            simulation.present_cast(cast);
            let cooldowns: Vec<(FireAbility, u8, f32)> = row
                .cooldowns
                .iter()
                .filter_map(|c| {
                    let ability = *FireAbility::ALL.get(usize::from(c.ability))?;
                    Some((ability, c.charges, (c.recharge - age).max(0.0)))
                })
                .collect();
            simulation.present_class_state(
                &cooldowns,
                PresentedResources {
                    heat: row.resources.heat,
                    energy: row.resources.energy,
                    mana: row.resources.mana,
                    deflection: row.resources.deflection,
                    astral: row.resources.astral,
                    global_cooldown: (row.resources.global_cooldown - age).max(0.0),
                },
            );
        }

        // Monsters.
        let monsters: Vec<_> = self.mirror.monsters.values().cloned().collect();
        for monster in &monsters {
            let Some(entity) = self.world.monsters.get(&monster.key).map(|m| m.entity) else {
                continue;
            };
            let age = monster.received.elapsed().as_secs_f32();
            let statuses = self.presented_statuses(simulation, &monster.statuses, age);
            simulation.present_statuses(entity, &statuses);
            let target = self.resolve(simulation, &monster.target);
            let cast = monster.cast.and_then(|cast| match cast.ability {
                AbilityRef::Monster(index) => EnemyAbility::ALL
                    .get(usize::from(index))
                    .map(|ability| (*ability, cast.elapsed + age, cast.duration)),
                _ => None,
            });
            let interruptible =
                cast.is_some_and(|(ability, _, _)| ability.definition().interruptible);
            simulation.present_enemy(entity, target, cast, interruptible);
        }

        // Events, as if they had happened here this frame.
        let tick = simulation.tick_count();
        for wire in self.mirror.take_events() {
            let is_mine = |player: &EntityRef| {
                mine.as_ref().is_some_and(
                    |account| matches!(player, EntityRef::Player(who) if who == account),
                )
            };
            match &wire {
                engine_realm::combat::WireEvent::LootAwarded { player, item }
                    if is_mine(player) =>
                {
                    self.on_loot(item, None);
                }
                engine_realm::combat::WireEvent::LootMailed {
                    player,
                    item,
                    reason,
                } if is_mine(player) => {
                    let reason = if *reason == 0 {
                        engine_realm::loadout::MAIL_BAG_FULL
                    } else {
                        engine_realm::loadout::MAIL_UNCLAIMED
                    };
                    self.on_loot(item, Some(reason));
                }
                _ => {}
            }
            let resolve = |reference: &EntityRef| self.resolve(simulation, reference);
            if let Some(event) = engine_realm::combat::wire::decode(&wire, &resolve, tick) {
                simulation.inject_event(event);
            }
        }

        // Presses, with the selection they were made against.
        let requests = simulation.take_remote_requests();
        let active = self.may_act();
        let completed = self.run_completed();
        if requests.is_empty() || !(active || completed) {
            return;
        }
        let target = simulation
            .player()
            .target
            .map_or(EntityRef::None, |entity| self.name_of(simulation, entity));
        let slot = |s: usize| u8::try_from(s).unwrap_or(u8::MAX);
        let at = |p: Vec3| [p.x, p.y, p.z];
        for request in requests {
            let kind = match request {
                engine_game::PlayerCommand::UseAbility(s) => ActionKind::Ability(slot(s)),
                engine_game::PlayerCommand::UseAbilityAt { slot: s, position } => {
                    ActionKind::AbilityAt(slot(s), at(position))
                }
                engine_game::PlayerCommand::UseUltimate { position } => {
                    ActionKind::Ultimate(at(position))
                }
                engine_game::PlayerCommand::UseExtra(index) => ActionKind::Extra(slot(index)),
                engine_game::PlayerCommand::OpenChest => ActionKind::OpenChest,
                _ => continue,
            };
            // After the run only the chest can be opened.
            if !active && kind != ActionKind::OpenChest {
                continue;
            }
            self.client.send(NetCommand::Act(CombatAction {
                target: target.clone(),
                kind,
            }));
        }
    }

    /// Whether the run is under way, so a press can mean something.
    fn may_act(&self) -> bool {
        self.mirror
            .instance
            .as_ref()
            .is_some_and(|instance| instance.state == engine_realm::instance::InstanceState::Active)
    }

    /// Whether the run is won and the instance lingers, so the chest can be opened.
    fn run_completed(&self) -> bool {
        self.mirror.instance.as_ref().is_some_and(|instance| {
            instance.state == engine_realm::instance::InstanceState::Completed
        })
    }

    fn leave_world(
        &mut self,
        simulation: &mut Simulation,
        extractor: &mut engine_presentation::RenderExtractor,
    ) {
        if self.world.instance.is_some() && self.world.completed && self.loot_watch.is_none() {
            // Whatever was never taken from the chest is mailed when the instance goes.
            let held = self.held_now();
            self.watch_loot(held);
        }
        for (_, remote) in std::mem::take(&mut self.world.remotes) {
            extractor.remove_remote_character(remote.entity);
            simulation.party_mut().leave(remote.entity);
            simulation.world_mut().despawn(remote.entity);
        }
        for (_, monster) in std::mem::take(&mut self.world.monsters) {
            simulation.despawn_enemy(monster.entity);
        }
        if simulation.is_remote_authority() {
            simulation.set_remote_authority(false);
            let player = simulation.player().entity;
            simulation.world_mut().restore(player);
            simulation.present_cast(None);
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
        self.returning = None;
    }

    // --- the hub ---------------------------------------------------------------------

    /// Stands the hub up or takes it down so it matches the screen: it exists on every page
    /// from the hub through the party screens (the world shows behind them), and not on the
    /// title, sign-in or character pages, nor in an instance.
    fn sync_hub(&mut self, simulation: &mut Simulation) {
        match (hub_screen(self.view.screen), self.hub.is_some()) {
            (true, false) => self.enter_hub(simulation),
            (false, true) => self.leave_hub(simulation),
            _ => {}
        }
    }

    /// The hub as a place to practise: the player at the hub's spawn with full health, a lone
    /// dummy ahead, a group of three nearby and a friendly dummy to heal, all under local
    /// authority.
    ///
    /// With the hub scene loaded ([`Self::set_hub_spawns`]) everything stands where the scene
    /// says and the scene's own boundary bounds the area. Without it the hub is the
    /// dungeon's start area, laid out from its first spawn point, with the start barrier
    /// standing so the area is bounded.
    fn enter_hub(&mut self, simulation: &mut Simulation) {
        let (spawn, yaw, layout, barrier_box) = if let Some(spawns) = self.hub_spawns {
            let mut layout = hub_layout(spawns.player, spawns.yaw);
            layout.single = spawns.featured;
            layout.group_center = spawns.group_center;
            (spawns.player, spawns.yaw, layout, None)
        } else {
            let Some(dungeon) = self.catalog.dungeons().first().cloned() else {
                return;
            };
            let point = dungeon.spawn_point(0);
            let spawn = Vec3::new(point.position[0], point.position[1], point.position[2]);
            let [min, max] = [dungeon.start_barrier.min(), dungeon.start_barrier.max()]
                .map(|corner| Vec3::new(corner[0], corner[1], corner[2]));
            (
                spawn,
                point.yaw,
                hub_layout(spawn, point.yaw),
                Some((min, max)),
            )
        };
        // Whatever the level was built with, and whatever was left from before, gives way.
        let player = simulation.player().entity;
        let strays: Vec<EntityId> = (0..simulation.world().slot_count())
            .filter_map(|slot| simulation.world().id_at(slot))
            .filter(|id| *id != player)
            .collect();
        for id in strays {
            simulation.party_mut().leave(id);
            simulation.despawn_enemy(id);
        }
        let mut entities = vec![
            simulation
                .world_mut()
                .spawn(EntityDesc::dummy(layout.single, HUB_DUMMY_HEALTH)),
        ];
        let config = ArenaConfig {
            group_count: 3,
            group_center: layout.group_center,
            dummy_health: HUB_DUMMY_HEALTH,
            group_spacing: self.hub_spawns.map_or_else(
                || ArenaConfig::default().group_spacing,
                |spawns| spawns.group_spacing,
            ),
            ..ArenaConfig::default()
        };
        spawn_group(simulation.world_mut(), &config, &mut entities);
        let healing = spawn_healing_dummy(
            simulation.world_mut(),
            layout.healing,
            HUB_HEALING_DUMMY_HEALTH,
        );
        simulation.party_mut().join(healing);
        entities.push(healing);

        let barrier = barrier_box.map(|(min, max)| {
            let world = simulation.collision_world_mut();
            let id = world.add_box(min, max, "hub barrier");
            world.rebuild_broad_phase();
            id
        });

        // Full health first (a restore also sends a body back to where it was created), then
        // arrive at the spawn point: a teleport the camera snaps to, not a run it follows.
        simulation.world_mut().restore(player);
        let from = simulation.player_position();
        place_player(simulation, spawn, yaw);
        simulation.world_mut().reset_history(player);
        let tick = simulation.tick_count();
        simulation.inject_event(CombatEvent::Teleported {
            entity: player,
            from,
            to: spawn,
            tick,
        });
        simulation.set_player_spawn(spawn, yaw);
        simulation.cancel_cast();
        simulation.select_nearest_target();
        self.hub = Some(HubWorld { entities, barrier });
        log::info!("online: the hub stands");
    }

    /// Takes the hub's dummies and barrier out of the simulation.
    fn leave_hub(&mut self, simulation: &mut Simulation) {
        let Some(hub) = self.hub.take() else {
            return;
        };
        for id in hub.entities {
            simulation.party_mut().leave(id);
            simulation.world_mut().despawn(id);
        }
        if let Some(barrier) = hub.barrier {
            let world = simulation.collision_world_mut();
            world.remove_static(barrier);
            world.rebuild_broad_phase();
        }
        log::info!("online: the hub is taken down");
    }
}

/// The title screen's words, bundled with the game.
const INTRO_JSON: &str = include_str!("../../../assets/catalog/intro.json");

/// Which level file the hub is, bundled with the game.
const HUB_JSON: &str = include_str!("../../../assets/catalog/hub.json");

/// The hub scene's path relative to the asset root, from `assets/catalog/hub.json`.
///
/// `None` when the file is not JSON or names no scene, which keeps the hub in the dungeon's
/// start area.
#[must_use]
pub fn hub_scene_file() -> Option<String> {
    parse_hub_scene(HUB_JSON)
}

/// Reads the hub scene's path out of the catalog file's text.
fn parse_hub_scene(text: &str) -> Option<String> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(text) else {
        log::warn!("online: the hub catalog is not valid JSON; the hub stays in the dungeon");
        return None;
    };
    document
        .get("scene")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

/// How long a request may wait for its answer before the interface stops showing it.
const BUSY_TIMEOUT: Duration = Duration::from_secs(20);

/// Health of the hub's training dummies: a full rotation does not delete one.
const HUB_DUMMY_HEALTH: f32 = 200_000.0;

/// Health of the hub's friendly dummy.
const HUB_HEALING_DUMMY_HEALTH: f32 = 2_000_000.0;

/// Reads the title screen's words. Any field that is missing or empty keeps the default, and a
/// file that is not JSON at all is all defaults.
fn parse_intro(text: &str) -> IntroCard {
    let mut intro = IntroCard::default();
    let Ok(document) = serde_json::from_str::<serde_json::Value>(text) else {
        log::warn!("online: the intro text is not valid JSON; using the defaults");
        return intro;
    };
    let string = |key: &str| {
        document
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    if let Some(title) = string("title") {
        intro.title = title;
    }
    if let Some(subtitle) = string("subtitle") {
        intro.subtitle = subtitle;
    }
    if let Some(label) = string("continue_label") {
        intro.continue_label = label;
    }
    let lore: Vec<String> = document
        .get("lore")
        .and_then(serde_json::Value::as_array)
        .map(|paragraphs| {
            paragraphs
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|paragraph| !paragraph.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if !lore.is_empty() {
        intro.lore = lore;
    }
    intro
}

/// Which screen the server state and the player's own place put them on.
///
/// The server decides what it can (an instance, a party's stage), so a reconnect lands where
/// the game is; `stage` is only the player's place among the pages the server has no opinion
/// on. The title comes first and only once, so it outranks everything until it is left.
fn route(signed_in: bool, stage: Screen, mirror: &RealmMirror) -> Screen {
    if !signed_in {
        return if stage == Screen::Title {
            Screen::Title
        } else {
            Screen::Login
        };
    }
    if stage == Screen::Title {
        return Screen::Title;
    }
    if mirror.instance.is_some() {
        return Screen::InWorld;
    }
    if let Some(party) = &mirror.party {
        match party.state {
            PartyState::ReadyCheck => return Screen::ReadyCheck,
            PartyState::ChoosingKey => return Screen::KeySelect,
            // Between the start and the instance appearing the party page waits.
            PartyState::Ready | PartyState::Starting | PartyState::InDungeon => {
                return Screen::Party;
            }
            PartyState::Completed | PartyState::Failed | PartyState::Disbanded => {}
        }
    }
    // A ticket is the hub with the queue panel; the pages opened from the hub stay.
    if mirror.ticket.is_some()
        && !matches!(stage, Screen::Dungeons | Screen::Roles | Screen::Workshop)
    {
        return Screen::Hub;
    }
    match stage {
        Screen::Login | Screen::Title => Screen::Characters,
        other => other,
    }
}

/// Whether the local simulation holds the hub on `screen`: from the hub through the party
/// pages, the world stands behind them.
fn hub_screen(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::Hub
            | Screen::Dungeons
            | Screen::Roles
            | Screen::Workshop
            | Screen::ReadyCheck
            | Screen::KeySelect
            | Screen::Party
    )
}

/// Whether the realm has visibly answered the request `kind` is waiting on.
fn busy_settled(kind: BusyKind, mirror: &RealmMirror) -> bool {
    let state = mirror.party.as_ref().map(|party| party.state);
    match kind {
        BusyKind::LeaveQueue => mirror.ticket.is_none(),
        BusyKind::JoinQueue => mirror.ticket.is_some() || mirror.party.is_some(),
        BusyKind::InsertKey => state != Some(PartyState::ChoosingKey),
        BusyKind::WithdrawKey => state != Some(PartyState::Ready),
        BusyKind::Start => mirror.instance.is_some() || state != Some(PartyState::Ready),
        BusyKind::LeaveParty => mirror.party.is_none(),
        // Answered by their own events.
        BusyKind::Characters | BusyKind::Workshop | BusyKind::SignOut | BusyKind::SignIn => false,
    }
}

/// Whether a run's result changes the character (so there is something to wait for).
fn expects_rewards(summary: &RunSummaryInfo) -> bool {
    summary.experience > 0
        || !summary.resources.is_empty()
        || summary.key_level_after != summary.key_level
}

/// What a result changes on a character.
fn character_mark(character: &CharacterSummary) -> CharacterMark {
    CharacterMark {
        level: character.level,
        experience: character.experience,
        key_level: character.key.as_ref().map_or(0, |key| key.level),
        resources: character.resources.iter().map(|(_, amount)| *amount).sum(),
    }
}

/// Where the hub's dummies stand relative to the spawn point.
struct HubLayout {
    single: Vec3,
    group_center: Vec3,
    healing: Vec3,
}

/// The hub's dummies: one six metres ahead, a group off to the right, a friendly one to the
/// left. "Ahead" is where the spawn point faces.
fn hub_layout(spawn: Vec3, yaw: f32) -> HubLayout {
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    HubLayout {
        single: spawn + forward * 6.0,
        group_center: spawn + forward * 12.0 + right * 7.0,
        healing: spawn + forward * 3.0 - right * 4.0,
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

/// Unix time in whole seconds, for the stamp on mail shown before the server's copy arrives.
fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use engine_net::MemoryTokenStore;
    use engine_net::model::{InstanceInfo, PartyInfo, TicketInfo};

    fn party(state: PartyState) -> PartyInfo {
        PartyInfo {
            id: 1,
            dungeon_id: "graveyard_crypt".into(),
            state,
            leader: None,
            key_owner: None,
            key_level: 0,
            ready_deadline: 0,
            instance_id: None,
        }
    }

    fn instance(state: InstanceState) -> InstanceInfo {
        InstanceInfo {
            id: 7,
            dungeon_id: "graveyard_crypt".into(),
            state,
            countdown_ends_at: None,
            barrier_open: false,
        }
    }

    fn ticket() -> TicketInfo {
        TicketInfo {
            dungeon_id: "graveyard_crypt".into(),
            role: Role::Dps,
            character_name: "Ember".into(),
            gear_score: 40,
            enqueued_at: 0,
            matched: false,
        }
    }

    fn mirror_with(party_state: Option<PartyState>) -> RealmMirror {
        RealmMirror {
            party: party_state.map(party),
            ..RealmMirror::default()
        }
    }

    #[test]
    fn a_fresh_launch_is_the_title_then_the_sign_in_form() {
        let mirror = RealmMirror::default();
        assert_eq!(route(false, Screen::Title, &mirror), Screen::Title);
        // Signed in by a remembered session, the title still comes first.
        assert_eq!(route(true, Screen::Title, &mirror), Screen::Title);
        // Past the title, nobody signed in is on the form; logging out never returns to it.
        assert_eq!(route(false, Screen::Login, &mirror), Screen::Login);
        assert_eq!(route(false, Screen::Hub, &mirror), Screen::Login);
        assert_eq!(route(true, Screen::Login, &mirror), Screen::Characters);
    }

    #[test]
    fn the_players_own_place_is_kept_when_the_server_has_no_opinion() {
        let mirror = RealmMirror::default();
        for stage in [
            Screen::Characters,
            Screen::Hub,
            Screen::Dungeons,
            Screen::Roles,
            Screen::Workshop,
        ] {
            assert_eq!(route(true, stage, &mirror), stage);
        }
    }

    #[test]
    fn the_party_decides_the_screen_by_its_state() {
        for (state, screen) in [
            (PartyState::ReadyCheck, Screen::ReadyCheck),
            (PartyState::ChoosingKey, Screen::KeySelect),
            (PartyState::Ready, Screen::Party),
            (PartyState::Starting, Screen::Party),
            (PartyState::InDungeon, Screen::Party),
        ] {
            let mirror = mirror_with(Some(state));
            assert_eq!(route(true, Screen::Hub, &mirror), screen, "{state:?}");
            // Wherever the player was looking, a reconnect lands on the party page.
            assert_eq!(route(true, Screen::Dungeons, &mirror), screen, "{state:?}");
        }
        // A party that is over leaves the player where they were.
        for state in [
            PartyState::Completed,
            PartyState::Failed,
            PartyState::Disbanded,
        ] {
            assert_eq!(
                route(true, Screen::Hub, &mirror_with(Some(state))),
                Screen::Hub
            );
        }
    }

    #[test]
    fn a_ticket_is_the_hub_and_an_instance_is_the_world() {
        let queued = RealmMirror {
            ticket: Some(ticket()),
            ..RealmMirror::default()
        };
        // A reconnect with a ticket lands in the hub, whatever page the player was on...
        assert_eq!(route(true, Screen::Characters, &queued), Screen::Hub);
        assert_eq!(route(true, Screen::Hub, &queued), Screen::Hub);
        // ...but a page opened from the hub (the workshop) is not taken from them.
        assert_eq!(route(true, Screen::Workshop, &queued), Screen::Workshop);
        // An instance outranks everything, including a lingering ticket and the party.
        let running = RealmMirror {
            ticket: Some(ticket()),
            party: Some(party(PartyState::InDungeon)),
            instance: Some(instance(InstanceState::Active)),
            ..RealmMirror::default()
        };
        assert_eq!(route(true, Screen::Hub, &running), Screen::InWorld);
        for state in [InstanceState::Completed, InstanceState::Failed] {
            let over = RealmMirror {
                instance: Some(instance(state)),
                ..RealmMirror::default()
            };
            assert_eq!(route(true, Screen::Hub, &over), Screen::InWorld);
        }
        // Signed out, nothing the realm says matters.
        assert_eq!(route(false, Screen::Hub, &running), Screen::Login);
    }

    #[test]
    fn the_hub_stands_behind_the_pages_opened_from_it() {
        for screen in [
            Screen::Hub,
            Screen::Dungeons,
            Screen::Roles,
            Screen::Workshop,
            Screen::ReadyCheck,
            Screen::KeySelect,
            Screen::Party,
        ] {
            assert!(hub_screen(screen), "{screen:?}");
        }
        for screen in [
            Screen::Title,
            Screen::Login,
            Screen::Characters,
            Screen::InWorld,
        ] {
            assert!(!hub_screen(screen), "{screen:?}");
        }
    }

    #[test]
    fn a_request_is_settled_by_what_the_realm_shows() {
        let none = RealmMirror::default();
        assert!(busy_settled(BusyKind::LeaveQueue, &none));
        let queued = RealmMirror {
            ticket: Some(ticket()),
            ..RealmMirror::default()
        };
        assert!(!busy_settled(BusyKind::LeaveQueue, &queued));
        assert!(busy_settled(BusyKind::JoinQueue, &queued));
        assert!(!busy_settled(BusyKind::JoinQueue, &none));

        let choosing = mirror_with(Some(PartyState::ChoosingKey));
        assert!(!busy_settled(BusyKind::InsertKey, &choosing));
        let ready = mirror_with(Some(PartyState::Ready));
        assert!(busy_settled(BusyKind::InsertKey, &ready));
        assert!(!busy_settled(BusyKind::WithdrawKey, &ready));
        assert!(busy_settled(BusyKind::WithdrawKey, &choosing));
        assert!(!busy_settled(BusyKind::Start, &ready));
        assert!(busy_settled(
            BusyKind::Start,
            &mirror_with(Some(PartyState::Starting))
        ));
        assert!(!busy_settled(BusyKind::LeaveParty, &ready));
        assert!(busy_settled(BusyKind::LeaveParty, &none));
        // Answered by their own events, never by the mirror.
        assert!(!busy_settled(BusyKind::Workshop, &none));
        assert!(!busy_settled(BusyKind::SignIn, &none));
    }

    #[test]
    fn the_intro_falls_back_field_by_field() {
        let default = IntroCard::default();
        // The file the game ships with parses.
        let shipped = parse_intro(INTRO_JSON);
        assert!(!shipped.title.is_empty() && !shipped.lore.is_empty());
        // Not JSON at all: all defaults.
        assert_eq!(parse_intro("{ nope"), default);
        // Wrong types and empty strings keep the default for that field only.
        let partial = parse_intro(
            r#"{"title":"  MY GAME ","subtitle":"","lore":[1,"  ","One."],"continue_label":7}"#,
        );
        assert_eq!(partial.title, "MY GAME");
        assert_eq!(partial.subtitle, default.subtitle);
        assert_eq!(partial.lore, vec!["One.".to_owned()]);
        assert_eq!(partial.continue_label, default.continue_label);
    }

    #[test]
    fn a_result_is_waited_for_only_when_it_changes_the_character() {
        let summary = |experience, resources: Vec<(String, u32)>, after| RunSummaryInfo {
            instance_id: 7,
            dungeon_id: "graveyard_crypt".into(),
            dungeon_name: "Graveyard Crypt".into(),
            success: experience > 0,
            key_level: 3,
            key_owner: true,
            key_level_after: after,
            duration_us: 0,
            experience,
            resources,
            loot_item: String::new(),
        };
        assert!(expects_rewards(&summary(300, vec![], 3)));
        assert!(expects_rewards(&summary(
            0,
            vec![("crypt_dust".into(), 4)],
            3
        )));
        assert!(expects_rewards(&summary(0, vec![], 4)));
        // A failure changes nothing: there is nothing to wait for.
        assert!(!expects_rewards(&summary(0, vec![], 3)));
    }

    #[test]
    fn the_hub_dummies_stand_ahead_of_the_spawn() {
        let spawn = Vec3::new(0.0, 0.0, -8.0);
        // Yaw zero faces -Z.
        let layout = hub_layout(spawn, 0.0);
        assert!((layout.single - Vec3::new(0.0, 0.0, -14.0)).length() < 1e-4);
        assert!(layout.group_center.z < layout.single.z);
        assert!(layout.group_center.x > 0.0, "the group is to the right");
        assert!(layout.healing.x < 0.0, "the friendly dummy is to the left");
        // Turning the spawn turns the layout with it, and keeps the distances.
        let turned = hub_layout(spawn, std::f32::consts::FRAC_PI_2);
        assert!(((turned.single - spawn).length() - 6.0).abs() < 1e-4);
        assert!(turned.single.x < spawn.x - 5.0);
    }

    fn session() -> OnlineSession {
        let endpoints = RealmEndpoints::resolve(None).expect("the default endpoints are valid");
        OnlineSession::start(
            endpoints,
            Box::new(MemoryTokenStore::default()),
            DevScript::default(),
        )
        .expect("the session starts")
    }

    #[test]
    fn the_hub_catalog_names_a_scene_and_tolerates_a_bad_file() {
        assert_eq!(
            hub_scene_file().as_deref(),
            Some("scenes/my_level.json"),
            "the shipped catalog names the hub scene"
        );
        assert_eq!(parse_hub_scene("{ nope"), None);
        assert_eq!(parse_hub_scene(r#"{ "version": 1 }"#), None);
        assert_eq!(parse_hub_scene(r#"{ "scene": "  " }"#), None);
        assert_eq!(
            parse_hub_scene(r#"{ "scene": " scenes/x.json " }"#).as_deref(),
            Some("scenes/x.json")
        );
    }

    #[test]
    fn the_level_follows_the_instance() {
        let mut session = session();
        assert_eq!(session.wanted_level(), OnlineLevel::Hub);
        session.mirror.instance = Some(instance(InstanceState::Active));
        assert_eq!(session.wanted_level(), OnlineLevel::Dungeon);
        session.mirror.instance = None;
        assert_eq!(session.wanted_level(), OnlineLevel::Hub);
    }

    #[test]
    fn a_hub_scene_places_the_hub_without_a_barrier() {
        let mut session = session();
        let spawns = HubSpawns {
            player: Vec3::new(1.0, 0.0, 2.0),
            yaw: 0.0,
            featured: Vec3::new(1.0, 0.0, -12.0),
            group_center: Vec3::new(14.0, 0.0, -18.0),
            group_spacing: 3.0,
        };
        session.set_hub_spawns(spawns);
        let mut simulation = engine_game::arena::training_arena(&ArenaConfig::default());
        let statics = simulation.collision_world().len();

        session.enter_hub(&mut simulation);
        assert_eq!(simulation.world().len(), 6);
        assert_eq!(
            simulation.collision_world().len(),
            statics,
            "the scene's own boundary bounds the hub"
        );
        let at = simulation.player_position();
        assert!((at - spawns.player).length() < 1e-4);
        let hub = session.hub.as_ref().expect("the hub stands");
        let single = hub.entities[0];
        let position = simulation.world().body(single).expect("a body").position;
        assert!((position - spawns.featured).length() < 1e-4);

        // A rebuilt collision world drops the handles; leaving then touches nothing.
        session.level_rebuilt();
        session.leave_hub(&mut simulation);
        assert_eq!(simulation.world().len(), 1);
        assert_eq!(simulation.collision_world().len(), statics);
    }

    #[test]
    fn the_hub_is_stood_up_and_taken_down_without_leaving_anything_behind() {
        let endpoints = RealmEndpoints::resolve(None).expect("the default endpoints are valid");
        let mut session = OnlineSession::start(
            endpoints,
            Box::new(MemoryTokenStore::default()),
            DevScript::default(),
        )
        .expect("the session starts");
        // The level the client builds online: a player and the scene's own dummies.
        let mut simulation = engine_game::arena::training_arena(&ArenaConfig::default());
        let statics = simulation.collision_world().len();
        // A monster left from an instance: its runtime goes with it, or the dummy that takes
        // its slot would inherit its name, rank and mind.
        simulation.spawn_enemy_with(
            EnemyKind::Melee,
            Vec3::new(3.0, 0.0, -3.0),
            0.0,
            &EnemySpawnOptions {
                name: Some("Lord Ashgrave".into()),
                ..EnemySpawnOptions::default()
            },
        );

        session.enter_hub(&mut simulation);
        assert_eq!(simulation.enemies().count(), 0, "no enemy runtime survives");
        // The player, a lone dummy, a group of three and a friendly dummy; the level's own
        // dummies are gone.
        assert_eq!(simulation.world().len(), 6);
        assert_eq!(simulation.collision_world().len(), statics + 1);
        assert!(!simulation.is_remote_authority(), "the hub is local");
        let spawn = session
            .catalog
            .dungeons()
            .first()
            .expect("a dungeon")
            .spawn_point(0);
        let at = simulation.player_position();
        assert!((at.x - spawn.position[0]).abs() < 1e-4 && (at.z - spawn.position[2]).abs() < 1e-4);

        // Entering again changes nothing, and leaving removes exactly what was added.
        session.leave_hub(&mut simulation);
        assert_eq!(simulation.world().len(), 1, "only the player is left");
        assert_eq!(simulation.collision_world().len(), statics);
        session.leave_hub(&mut simulation);
        assert_eq!(simulation.world().len(), 1);
    }
}
