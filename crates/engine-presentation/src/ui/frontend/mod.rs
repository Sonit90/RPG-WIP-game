//! The online front-end: every screen between launching the game and standing in a dungeon.
//!
//! ```text
//! FrontEndView (plain data the app builds from the realm mirror)
//!        │
//!        ▼
//! FrontEndUi::update(input) ──► Vec<FrontEndAction> ──► the app sends network commands
//! FrontEndUi::draw(list)    ──► UiDrawList (the same single UI batch as the HUD)
//! ```
//!
//! The screens never decide anything about the realm. Which screen shows is the view's
//! `screen`, chosen by the app from what the *server* says (a party in its ready check →
//! ReadyCheck, a party choosing its key → KeySelect ...), so a reconnect lands on the right
//! screen by construction. The only state kept here is the interface's own: what is typed in
//! the login fields, which field has focus, whether the sign-in form is in "create account"
//! mode.
//!
//! Two screens are not full-screen front-ends but overlays on the playable world: the
//! [`Screen::Hub`] (the player's panel, and the queue panel while a ticket exists) and
//! [`Screen::InWorld`] (the countdown, the leave control and, once a run ends, the completion
//! panel). Only the rectangles they draw take the pointer.
//!
//! Layout is a pure function of the view, the viewport and the UI scale; `update` and `draw`
//! both call it, so what is clicked is always what was drawn.

pub mod text_field;

use engine_game::character::equipment::{INVENTORY_COLUMNS, INVENTORY_SLOTS};
use engine_game::character::{ItemIcon, ItemQuality};
use engine_realm::Role;
use engine_realm::instance::CountdownDisplay;
use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, TextStyle, UiColor, UiDrawList, UiRect, UiScale};

pub use text_field::{FieldKeys, FieldOutcome, TextField};

/// Which screen is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// The game's name and a little lore, before anything else. Once per launch.
    #[default]
    Title,
    /// Sign in or create an account.
    Login,
    /// Choose a character.
    Characters,
    /// The hub: the playable start area with the player's panel (and the queue panel while
    /// searching) over it.
    Hub,
    /// Choose a dungeon.
    Dungeons,
    /// Choose a role.
    Roles,
    /// Salvage bag items and exchange resources.
    Workshop,
    /// "Dungeon found": accept or decline.
    ReadyCheck,
    /// The party is choosing whose key to use.
    KeySelect,
    /// The party, waiting for its leader (the key's owner).
    Party,
    /// In the dungeon: no full-screen interface, only overlays.
    InWorld,
}

impl Screen {
    /// Whether the screen is drawn over the playable world instead of covering it.
    #[must_use]
    pub fn is_overlay(self) -> bool {
        matches!(self, Self::Hub | Self::InWorld)
    }
}

/// How good some status is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    /// All well.
    Good,
    /// In progress.
    Busy,
    /// Worth noticing.
    Warning,
    /// Broken.
    Bad,
}

/// One character on the selection screen.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterCard {
    /// Name.
    pub name: String,
    /// Class.
    pub class: String,
    /// Roles it can queue as.
    pub roles: Vec<Role>,
    /// Level.
    pub level: u32,
    /// Experience into the level.
    pub experience: u64,
    /// Experience the level needs (0 at the cap).
    pub next_level: u64,
    /// Gear score, from the server.
    pub gear_score: u32,
    /// Items worn.
    pub equipped: usize,
    /// The dungeon key it holds.
    pub key: Option<KeyCard>,
    /// Resources held, all kinds together.
    pub resources: u64,
}

/// A dungeon key, as the cards show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCard {
    /// The dungeon it opens.
    pub dungeon: String,
    /// Its level.
    pub level: u32,
}

impl PartyCard {
    /// The level of the local player's key, when it opens this party's dungeon.
    #[must_use]
    pub fn my_key(&self) -> Option<u32> {
        self.members.iter().find(|m| m.me).and_then(|m| m.key_level)
    }
}

impl KeyCard {
    /// "Graveyard Crypt key . level 3".
    fn text(&self) -> String {
        format!("{} key \u{b7} level {}", self.dungeon, self.level)
    }
}

/// The title screen's words, from `assets/catalog/intro.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntroCard {
    /// The game's name.
    pub title: String,
    /// A line under it.
    pub subtitle: String,
    /// Paragraphs of lore.
    pub lore: Vec<String>,
    /// What the button says.
    pub continue_label: String,
}

impl Default for IntroCard {
    fn default() -> Self {
        Self {
            title: "SHADOWS OF THE CRYPT".into(),
            subtitle: "Online".into(),
            lore: vec![
                "The dead of the old churchyard no longer rest. Beneath the stones a crypt \
                 keeps opening, and every key that fits it leads a little deeper."
                    .into(),
            ],
            continue_label: "Continue".into(),
        }
    }
}

/// The hub's panel: who the player is, what they hold, what they may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubCard {
    /// Character name.
    pub name: String,
    /// Class.
    pub class: String,
    /// Level.
    pub level: u32,
    /// Experience into the level.
    pub experience: u64,
    /// Experience the level needs (0 at the cap).
    pub next_level: u64,
    /// The dungeon key.
    pub key: Option<KeyCard>,
    /// Resources held: display name and amount.
    pub resources: Vec<(String, u64)>,
    /// Whether the Dungeon Finder may be opened (not while searching or saving rewards).
    pub can_find_dungeon: bool,
    /// Whether the character may be changed (not while queued or in a party).
    pub can_change_character: bool,
}

/// One bag cell on the workshop screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BagRow {
    /// The bag cell, as the API addresses it.
    pub index: u32,
    /// Item name.
    pub name: String,
    /// How many.
    pub quantity: u16,
    /// Item level.
    pub item_level: u16,
    /// The picture the cell shows.
    pub icon: ItemIcon,
    /// The rank the cell's edge is coloured by.
    pub quality: ItemQuality,
    /// What salvaging the whole stack yields, as text ("+8 Crypt Dust").
    pub yields: String,
}

/// One exchange on the workshop screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeRow {
    /// The catalog id.
    pub id: String,
    /// "10 Crypt Dust for 1 Ember Shard".
    pub label: String,
    /// Whether the character can pay for one trade.
    pub affordable: bool,
}

/// The salvage and exchange screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkshopCard {
    /// The occupied bag cells.
    pub bag: Vec<BagRow>,
    /// The bag cells in the salvage tray, in the order they were put there.
    pub tray: Vec<u32>,
    /// How many cells the tray has.
    pub tray_slots: usize,
    /// What salvaging the tray yields, as text ("+34 Bone Shard"); empty for an empty tray.
    pub tray_yield: String,
    /// Resources held: display name and amount.
    pub resources: Vec<(String, u64)>,
    /// Available exchanges.
    pub exchanges: Vec<ExchangeRow>,
}

/// How a run ended, for the panel over the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCard {
    /// Whether the boss fell.
    pub success: bool,
    /// Dungeon name.
    pub dungeon: String,
    /// The key level the run used.
    pub key_level: u32,
    /// Whether the local player's key was the one used.
    pub key_owner: bool,
    /// The owner's key level after the run.
    pub key_level_after: u32,
    /// How long it took.
    pub duration_seconds: u64,
    /// Experience granted.
    pub experience: u32,
    /// Resources granted: display name and amount.
    pub resources: Vec<(String, u32)>,
    /// The loot line: the item's name, or a prompt to open the chest. `None`: no loot.
    pub loot: Option<String>,
    /// Whether Return to Hub was pressed and the rewards are being saved.
    pub returning: bool,
}

/// One dungeon on the selection screen.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonCard {
    /// Name.
    pub name: String,
    /// A paragraph.
    pub description: String,
    /// Difficulty name.
    pub difficulty: String,
    /// Level range.
    pub levels: (u32, u32),
    /// Recommended gear score.
    pub recommended_gear_score: u32,
    /// Required gear score.
    pub min_gear_score: u32,
    /// Rewards.
    pub rewards: Vec<String>,
    /// Players searching: tanks, healers, dps.
    pub population: [u32; 3],
}

/// The local player's queue ticket.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueCard {
    /// Dungeon name.
    pub dungeon: String,
    /// Role.
    pub role: Role,
    /// Character name.
    pub character: String,
    /// Seconds in the queue.
    pub waited_seconds: u64,
    /// Current matchmaking range, +/- percent.
    pub tolerance_percent: u32,
    /// A group was found and the ready check is opening.
    pub matched: bool,
}

/// A member's ready-check state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    /// Not answered.
    Waiting,
    /// Accepted.
    Ready,
    /// Declined.
    Declined,
}

/// One party member.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    /// Their account id (the target of kick / transfer).
    pub account_id: String,
    /// Character name.
    pub character: String,
    /// Player display name.
    pub player: String,
    /// Class.
    pub class: String,
    /// Role.
    pub role: Role,
    /// Level.
    pub level: u32,
    /// Gear score.
    pub gear_score: u32,
    /// The level of the key they brought, when it opens this party's dungeon.
    pub key_level: Option<u32>,
    /// Ready-check answer.
    pub ready: ReadyState,
    /// Connected.
    pub present: bool,
    /// Leads the party.
    pub leader: bool,
    /// Their key is the one in the lock.
    pub owner: bool,
    /// Is the local player.
    pub me: bool,
}

/// The local player's party.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PartyCard {
    /// Dungeon name.
    pub dungeon: String,
    /// A sentence about it.
    pub description: String,
    /// The key level in the lock (0 while none is).
    pub key_level: u32,
    /// Whose key it is, by character name.
    pub key_owner: Option<String>,
    /// Whether the local player's key is the one in the lock.
    pub i_own_key: bool,
    /// Members, in party order.
    pub members: Vec<MemberRow>,
    /// Whether the local player leads (the key's owner).
    pub i_lead: bool,
    /// Whether the local player has answered the ready check.
    pub i_answered: bool,
    /// Seconds left on the ready check.
    pub seconds_left: u64,
    /// Fraction of the ready check left, for the bar.
    pub fraction_left: f32,
}

/// A transient message.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// How it looks.
    pub level: StatusLevel,
    /// What it says.
    pub message: String,
}

/// Everything the front-end draws, built by the app from the realm mirror each frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrontEndView {
    /// Which screen.
    pub screen: Screen,
    /// The realm connection, as a short line.
    pub connection: Option<(StatusLevel, String)>,
    /// The signed-in account's name.
    pub account: Option<String>,
    /// Something in progress ("Signing in...").
    pub busy: Option<String>,
    /// The last refusal, for the current screen.
    pub error: Option<String>,
    /// The account's characters.
    pub characters: Vec<CharacterCard>,
    /// Which is selected.
    pub selected_character: Option<usize>,
    /// The title screen's words.
    pub intro: IntroCard,
    /// The hub panel, while a character is chosen.
    pub hub: Option<HubCard>,
    /// The salvage and exchange screen.
    pub workshop: Option<WorkshopCard>,
    /// The panel over the world once a run has ended.
    pub completion: Option<CompletionCard>,
    /// The dungeon catalog.
    pub dungeons: Vec<DungeonCard>,
    /// Which is selected.
    pub selected_dungeon: Option<usize>,
    /// The chosen role.
    pub selected_role: Option<Role>,
    /// The queue ticket.
    pub queue: Option<QueueCard>,
    /// The party.
    pub party: Option<PartyCard>,
    /// The countdown, while it runs.
    pub countdown: Option<CountdownDisplay>,
    /// Shown over everything while the connection is being re-established.
    pub reconnecting: Option<String>,
    /// Recent messages from the server.
    pub toasts: Vec<Toast>,
}

/// What the player asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontEndAction {
    /// Sign in.
    Login {
        /// Email.
        email: String,
        /// Password.
        password: String,
        /// Keep the session.
        remember: bool,
    },
    /// Create an account.
    Register {
        /// Email.
        email: String,
        /// Password.
        password: String,
        /// Display name.
        display_name: String,
        /// Keep the session.
        remember: bool,
    },
    /// Leave the title screen.
    ContinueFromTitle,
    /// Sign out.
    Logout,
    /// Leave the game.
    Quit,
    /// Open the Dungeon Finder from the hub.
    OpenDungeonFinder,
    /// Open the character sheet.
    OpenEquipment,
    /// Open the salvage and exchange screen.
    OpenWorkshop,
    /// Back to the character list.
    ChangeCharacter,
    /// Highlight a character.
    SelectCharacter(usize),
    /// Continue with the highlighted character.
    ConfirmCharacter,
    /// Highlight a dungeon.
    SelectDungeon(usize),
    /// Continue to role selection.
    ConfirmDungeon,
    /// Choose a role.
    SelectRole(Role),
    /// Join the queue.
    JoinQueue,
    /// One screen back.
    Back,
    /// Leave the queue.
    LeaveQueue,
    /// Accept the ready check.
    Accept,
    /// Decline the ready check.
    Decline,
    /// Put the local player's key in the lock.
    InsertKey,
    /// Take the key back out (its owner).
    WithdrawKey,
    /// Start the dungeon (the key's owner).
    StartDungeon,
    /// Remove a member (leader).
    Kick(String),
    /// Leave the party.
    LeaveParty,
    /// Leave a finished run and return to the hub.
    ReturnToHub,
    /// Put the bag cell's item in the salvage tray.
    StageSalvage(u32),
    /// Take the bag cell's item back out of the salvage tray.
    UnstageSalvage(u32),
    /// Break down everything in the salvage tray.
    SalvageTray,
    /// Make this exchange once.
    Exchange(String),
}

/// The frame's input, as the front-end reads it.
pub struct FrontInput<'a> {
    /// Pointer, client pixels.
    pub pointer: (f32, f32),
    /// Left button went down this frame.
    pub pressed: bool,
    /// Characters typed this frame (editing controls included).
    pub typed: &'a [char],
    /// Navigation keys pressed this frame.
    pub keys: FieldKeys,
    /// Escape pressed this frame.
    pub escape: bool,
    /// Reads the clipboard, when a field asks to paste.
    pub clipboard: &'a dyn Fn() -> Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldId {
    Email,
    DisplayName,
    Password,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    Field(FieldId),
    Remember,
    Submit,
    ToggleRegister,
    Quit,
    Logout,
    Character(usize),
    Continue,
    Back,
    DungeonFinder,
    Equipment,
    Workshop,
    ChangeCharacter,
    Dungeon(usize),
    Role(Role),
    LeaveQueue,
    Accept,
    Decline,
    InsertKey,
    WithdrawKey,
    Start,
    LeaveParty,
    ReturnToHub,
    Kick(String),
    /// A bag cell on the workshop screen, by bag index.
    BagCell(u32),
    /// A tray cell on the workshop screen, by position in the tray.
    TrayCell(usize),
    SalvageTray,
    Exchange(String),
    /// A panel with nothing to press: it only keeps the pointer from the camera.
    Panel,
}

/// The front-end's own state: typed text, focus, form mode.
#[derive(Debug)]
pub struct FrontEndUi {
    email: TextField,
    display_name: TextField,
    password: TextField,
    focus: Option<FieldId>,
    register: bool,
    remember: bool,
    pointer: (f32, f32),
    clock: f32,
}

impl Default for FrontEndUi {
    fn default() -> Self {
        Self::new()
    }
}

// --- palette ----------------------------------------------------------------------------

const BACKDROP_TOP: UiColor = [0.015, 0.02, 0.035, 0.90];
const BACKDROP_BOTTOM: UiColor = [0.04, 0.03, 0.05, 0.96];
const TITLE_TOP: UiColor = [0.01, 0.015, 0.03, 0.55];
const TITLE_BOTTOM: UiColor = [0.02, 0.015, 0.03, 0.82];
const PANEL_TOP: UiColor = [0.11, 0.12, 0.16, 0.97];
const PANEL_BOTTOM: UiColor = [0.06, 0.065, 0.09, 0.97];
const GOLD: UiColor = [0.96, 0.82, 0.48, 1.0];
const GOLD_EDGE: UiColor = [0.62, 0.50, 0.28, 0.85];
const TEXT: UiColor = [0.91, 0.92, 0.95, 1.0];
const DIM: UiColor = [0.62, 0.64, 0.71, 1.0];
const FAINT: UiColor = [0.40, 0.42, 0.48, 1.0];
const GOOD: UiColor = [0.42, 0.86, 0.46, 1.0];
const BUSY: UiColor = [0.96, 0.74, 0.30, 1.0];
const BAD: UiColor = [0.95, 0.38, 0.34, 1.0];
const FIELD: UiColor = [0.04, 0.045, 0.06, 1.0];
const SELECTED: UiColor = [0.96, 0.82, 0.48, 0.95];

fn status_color(level: StatusLevel) -> UiColor {
    match level {
        StatusLevel::Good => GOOD,
        StatusLevel::Busy => BUSY,
        StatusLevel::Warning => BUSY,
        StatusLevel::Bad => BAD,
    }
}

fn role_color(role: Role) -> UiColor {
    match role {
        Role::Tank => [0.38, 0.58, 0.98, 1.0],
        Role::Healer => [0.40, 0.88, 0.50, 1.0],
        Role::Dps => [0.96, 0.44, 0.38, 1.0],
    }
}

fn role_blurb(role: Role) -> &'static str {
    match role {
        Role::Tank => "Hold the enemy's attention and take the hits.",
        Role::Healer => "Keep the party alive through every pull.",
        Role::Dps => "Bring the enemies down before they bring you down.",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

/// One frame's interactive regions.
struct Layout {
    hits: Vec<(Hit, UiRect)>,
}

impl Layout {
    fn new() -> Self {
        Self {
            hits: Vec::with_capacity(32),
        }
    }

    fn add(&mut self, hit: Hit, rect: UiRect) -> UiRect {
        self.hits.push((hit, rect));
        rect
    }

    fn at(&self, pointer: (f32, f32)) -> Option<&Hit> {
        self.hits
            .iter()
            .rev()
            .find(|(_, rect)| rect.contains(pointer.0, pointer.1))
            .map(|(hit, _)| hit)
    }
}

/// Seconds as `m:ss`.
fn clock_text(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

impl FrontEndUi {
    /// A fresh front-end on the sign-in form.
    #[must_use]
    pub fn new() -> Self {
        Self {
            email: TextField::new(96),
            display_name: TextField::new(24),
            password: TextField::password(128),
            focus: Some(FieldId::Email),
            register: false,
            remember: true,
            pointer: (0.0, 0.0),
            clock: 0.0,
        }
    }

    /// Pre-fills the email field (a remembered account).
    pub fn set_email(&mut self, email: &str) {
        self.email.set_text(email);
        self.focus = Some(FieldId::Password);
    }

    /// Clears the password: after every attempt, successful or not.
    pub fn clear_password(&mut self) {
        self.password.clear();
    }

    /// Whether a text field has the keyboard (the app then sends no keys to gameplay).
    #[must_use]
    pub fn wants_keyboard(&self, view: &FrontEndView) -> bool {
        view.screen == Screen::Login && self.focus.is_some()
    }

    /// Whether the pointer is over front-end interface (the app then keeps it from the camera).
    #[must_use]
    pub fn wants_pointer(&self, view: &FrontEndView, viewport: (u32, u32), scale: UiScale) -> bool {
        !view.screen.is_overlay()
            || self
                .layout(view, viewport, scale)
                .at(self.pointer)
                .is_some()
    }

    fn field_mut(&mut self, id: FieldId) -> &mut TextField {
        match id {
            FieldId::Email => &mut self.email,
            FieldId::DisplayName => &mut self.display_name,
            FieldId::Password => &mut self.password,
        }
    }

    fn field_order(&self) -> Vec<FieldId> {
        if self.register {
            vec![FieldId::Email, FieldId::DisplayName, FieldId::Password]
        } else {
            vec![FieldId::Email, FieldId::Password]
        }
    }

    fn submit_login(&mut self) -> Option<FrontEndAction> {
        let email = self.email.text().trim().to_owned();
        let password = self.password.text().to_owned();
        if email.is_empty() || password.is_empty() {
            return None;
        }
        Some(if self.register {
            FrontEndAction::Register {
                email,
                password,
                display_name: self.display_name.text().trim().to_owned(),
                remember: self.remember,
            }
        } else {
            FrontEndAction::Login {
                email,
                password,
                remember: self.remember,
            }
        })
    }

    /// Reads one frame of input against the screen the view describes.
    pub fn update(
        &mut self,
        input: &FrontInput<'_>,
        view: &FrontEndView,
        viewport: (u32, u32),
        scale: UiScale,
        delta_seconds: f32,
    ) -> Vec<FrontEndAction> {
        self.clock += delta_seconds;
        self.pointer = input.pointer;
        let mut actions = Vec::new();

        // Typing, on the sign-in form.
        if view.screen == Screen::Login && view.busy.is_none() {
            if let Some(focus) = self.focus {
                let outcome = self.field_mut(focus).apply(input.typed, input.keys);
                if outcome.paste_requested
                    && let Some(text) = (input.clipboard)()
                {
                    self.field_mut(focus).paste(&text);
                }
                if outcome.tabbed {
                    let order = self.field_order();
                    let next = order
                        .iter()
                        .position(|id| *id == focus)
                        .map_or(0, |i| (i + 1) % order.len());
                    self.focus = order.get(next).copied();
                }
                if outcome.submitted
                    && let Some(action) = self.submit_login()
                {
                    actions.push(action);
                }
            }
        }

        if view.screen == Screen::Title
            && input.typed.iter().any(|c| matches!(c, '\r' | '\n' | ' '))
        {
            actions.push(FrontEndAction::ContinueFromTitle);
        }

        if input.escape {
            match view.screen {
                Screen::Dungeons | Screen::Roles | Screen::Workshop => {
                    actions.push(FrontEndAction::Back);
                }
                Screen::Login if self.register => self.register = false,
                _ => {}
            }
        }

        if !input.pressed {
            return actions;
        }
        let layout = self.layout(view, viewport, scale);
        let Some(hit) = layout.at(input.pointer).cloned() else {
            if view.screen == Screen::Login {
                self.focus = None;
            }
            return actions;
        };
        match hit {
            Hit::Field(id) => self.focus = Some(id),
            Hit::Remember => self.remember = !self.remember,
            Hit::Submit => {
                if let Some(action) = self.submit_login() {
                    actions.push(action);
                }
            }
            Hit::ToggleRegister => {
                self.register = !self.register;
                self.focus = Some(FieldId::Email);
            }
            Hit::Quit => actions.push(FrontEndAction::Quit),
            Hit::Logout => actions.push(FrontEndAction::Logout),
            Hit::Character(index) => actions.push(FrontEndAction::SelectCharacter(index)),
            Hit::Continue => actions.push(match view.screen {
                Screen::Title => FrontEndAction::ContinueFromTitle,
                Screen::Characters => FrontEndAction::ConfirmCharacter,
                Screen::Dungeons => FrontEndAction::ConfirmDungeon,
                _ => FrontEndAction::JoinQueue,
            }),
            Hit::Back => actions.push(FrontEndAction::Back),
            Hit::DungeonFinder => actions.push(FrontEndAction::OpenDungeonFinder),
            Hit::Equipment => actions.push(FrontEndAction::OpenEquipment),
            Hit::Workshop => actions.push(FrontEndAction::OpenWorkshop),
            Hit::ChangeCharacter => actions.push(FrontEndAction::ChangeCharacter),
            Hit::InsertKey => actions.push(FrontEndAction::InsertKey),
            Hit::WithdrawKey => actions.push(FrontEndAction::WithdrawKey),
            Hit::ReturnToHub => actions.push(FrontEndAction::ReturnToHub),
            Hit::BagCell(index) => actions.push(FrontEndAction::StageSalvage(index)),
            Hit::TrayCell(position) => {
                if let Some(index) = view
                    .workshop
                    .as_ref()
                    .and_then(|workshop| workshop.tray.get(position))
                {
                    actions.push(FrontEndAction::UnstageSalvage(*index));
                }
            }
            Hit::SalvageTray => actions.push(FrontEndAction::SalvageTray),
            Hit::Exchange(id) => actions.push(FrontEndAction::Exchange(id)),
            Hit::Panel => {}
            Hit::Dungeon(index) => actions.push(FrontEndAction::SelectDungeon(index)),
            Hit::Role(role) => actions.push(FrontEndAction::SelectRole(role)),
            Hit::LeaveQueue => actions.push(FrontEndAction::LeaveQueue),
            Hit::Accept => actions.push(FrontEndAction::Accept),
            Hit::Decline => actions.push(FrontEndAction::Decline),
            Hit::Start => actions.push(FrontEndAction::StartDungeon),
            Hit::LeaveParty => actions.push(FrontEndAction::LeaveParty),
            Hit::Kick(account) => actions.push(FrontEndAction::Kick(account)),
        }
        actions
    }

    // --- layout -----------------------------------------------------------------------

    fn layout(&self, view: &FrontEndView, viewport: (u32, u32), scale: UiScale) -> Layout {
        let mut layout = Layout::new();
        #[allow(
            clippy::cast_precision_loss,
            reason = "viewport sizes are small integers"
        )]
        let (w, h) = (viewport.0 as f32, viewport.1 as f32);
        let px = |v: f32| scale.px(v);

        // Top bar: account and sign-out, on every signed-in full-screen page.
        if shows_account(view) {
            layout.add(
                Hit::Logout,
                UiRect::new(w - px(140.0), px(18.0), px(120.0), px(34.0)),
            );
        }

        match view.screen {
            Screen::Title => {
                layout.add(Hit::Continue, title_continue(w, h, scale));
                layout.add(Hit::Quit, quit_button(h, scale));
            }
            Screen::Login => {
                let panel = login_panel(w, h, scale, self.register);
                let mut y = panel.y + px(118.0);
                layout.add(
                    Hit::Field(FieldId::Email),
                    UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(44.0)),
                );
                y += px(76.0);
                if self.register {
                    layout.add(
                        Hit::Field(FieldId::DisplayName),
                        UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(44.0)),
                    );
                    y += px(76.0);
                }
                layout.add(
                    Hit::Field(FieldId::Password),
                    UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(44.0)),
                );
                y += px(60.0);
                layout.add(
                    Hit::Remember,
                    UiRect::new(panel.x + px(36.0), y, px(200.0), px(28.0)),
                );
                y += px(44.0);
                layout.add(
                    Hit::Submit,
                    UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(48.0)),
                );
                y += px(60.0);
                layout.add(
                    Hit::ToggleRegister,
                    UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(36.0)),
                );
                layout.add(Hit::Quit, quit_button(h, scale));
            }
            Screen::Characters => {
                for (index, rect) in character_cards(view.characters.len(), w, h, scale)
                    .into_iter()
                    .enumerate()
                {
                    layout.add(Hit::Character(index), rect);
                }
                let (back, next) = footer_buttons(w, h, scale);
                let _ = back;
                if view.selected_character.is_some() {
                    layout.add(Hit::Continue, next);
                }
            }
            Screen::Hub => {
                if let Some(hub) = &view.hub {
                    let rects = hub_rects(hub.resources.len(), view.busy.is_some(), w, h, scale);
                    layout.add(Hit::Panel, rects.panel);
                    if hub.can_find_dungeon {
                        layout.add(Hit::DungeonFinder, rects.finder);
                    }
                    layout.add(Hit::Equipment, rects.equipment);
                    layout.add(Hit::Workshop, rects.workshop);
                    if hub.can_change_character {
                        layout.add(Hit::ChangeCharacter, rects.change);
                    }
                    layout.add(Hit::Logout, rects.logout);
                }
                if let Some(queue) = &view.queue {
                    let (panel, leave) = queue_panel(h, scale);
                    layout.add(Hit::Panel, panel);
                    if !queue.matched {
                        layout.add(Hit::LeaveQueue, leave);
                    }
                }
            }
            Screen::Dungeons => {
                let (list, _) = dungeon_panels(w, h, scale);
                for index in 0..view.dungeons.len() {
                    let row = UiRect::new(
                        list.x + px(12.0),
                        list.y + px(64.0) + px(74.0) * index as f32,
                        list.width - px(24.0),
                        px(64.0),
                    );
                    layout.add(Hit::Dungeon(index), row);
                }
                let (back, next) = footer_buttons(w, h, scale);
                layout.add(Hit::Back, back);
                if view.selected_dungeon.is_some() {
                    layout.add(Hit::Continue, next);
                }
            }
            Screen::Roles => {
                let playable = view
                    .selected_character
                    .and_then(|i| view.characters.get(i))
                    .map(|c| c.roles.clone())
                    .unwrap_or_default();
                for (role, rect) in Role::ALL.into_iter().zip(role_tiles(w, h, scale)) {
                    if playable.contains(&role) {
                        layout.add(Hit::Role(role), rect);
                    }
                }
                let (back, next) = footer_buttons(w, h, scale);
                layout.add(Hit::Back, back);
                if view.selected_role.is_some() && view.busy.is_none() {
                    layout.add(Hit::Continue, next);
                }
            }
            Screen::Workshop => {
                let panels = workshop_panels(w, h, scale);
                let exchange = panels.exchange;
                if let Some(workshop) = &view.workshop {
                    let idle = view.busy.is_none();
                    if idle {
                        for row in &workshop.bag {
                            if !workshop.tray.contains(&row.index) {
                                layout.add(
                                    Hit::BagCell(row.index),
                                    bag_cell_rect(panels.bag, row.index as usize, scale),
                                );
                            }
                        }
                        for position in 0..workshop.tray.len().min(TRAY_CELLS) {
                            layout.add(
                                Hit::TrayCell(position),
                                tray_cell_rect(panels.salvage, position, scale),
                            );
                        }
                        if !workshop.tray.is_empty() {
                            layout.add(Hit::SalvageTray, salvage_button(panels.salvage, scale));
                        }
                    }
                    for (index, row) in workshop
                        .exchanges
                        .iter()
                        .enumerate()
                        .take(workshop_rows(exchange, scale))
                    {
                        if row.affordable && idle {
                            layout.add(
                                Hit::Exchange(row.id.clone()),
                                workshop_button(workshop_row(exchange, index, scale), scale),
                            );
                        }
                    }
                }
                let (back, _) = footer_buttons(w, h, scale);
                layout.add(Hit::Back, back);
            }
            Screen::ReadyCheck => {
                let panel = ready_panel(w, h, scale);
                let answered = view.party.as_ref().is_some_and(|p| p.i_answered);
                if !answered {
                    let (accept, decline) = ready_buttons(panel, scale);
                    layout.add(Hit::Accept, accept);
                    layout.add(Hit::Decline, decline);
                }
            }
            Screen::KeySelect => {
                let panel = key_panel(w, h, scale);
                let (leave, insert) = key_buttons(panel, scale);
                if let Some(party) = &view.party
                    && view.busy.is_none()
                {
                    if party.my_key().is_some() {
                        layout.add(Hit::InsertKey, insert);
                    }
                    layout.add(Hit::LeaveParty, leave);
                }
            }
            Screen::Party => {
                let panel = party_panel(w, h, scale);
                if let Some(party) = &view.party {
                    let idle = view.busy.is_none();
                    for (index, member) in party.members.iter().enumerate() {
                        let row = member_row(panel, index, PARTY_ROWS_TOP, scale);
                        if party.i_lead && !member.me && idle {
                            layout.add(
                                Hit::Kick(member.account_id.clone()),
                                kick_button(row, scale),
                            );
                        }
                    }
                    let buttons = party_buttons(panel, scale);
                    if party.i_own_key && idle {
                        layout.add(Hit::WithdrawKey, buttons.withdraw);
                    }
                    if start_enabled(view, party) {
                        layout.add(Hit::Start, buttons.start);
                    }
                    if idle {
                        layout.add(Hit::LeaveParty, buttons.leave);
                    }
                }
            }
            Screen::InWorld => {
                if let Some(card) = &view.completion {
                    let (panel, button) = completion_rects(card, w, h, scale);
                    layout.add(Hit::Panel, panel);
                    if !card.returning {
                        layout.add(Hit::ReturnToHub, button);
                    }
                } else {
                    // A small "leave" control while in the dungeon.
                    layout.add(Hit::LeaveParty, in_world_leave(w, scale));
                }
            }
        }
        layout
    }

    // --- drawing ----------------------------------------------------------------------

    /// Draws the front-end for `view`.
    pub fn draw(
        &self,
        list: &mut UiDrawList,
        view: &FrontEndView,
        viewport: (u32, u32),
        scale: UiScale,
    ) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "viewport sizes are small integers"
        )]
        let (w, h) = (viewport.0 as f32, viewport.1 as f32);
        let layout = self.layout(view, viewport, scale);
        let hovered = layout.at(self.pointer).cloned();
        let ctx = Painter {
            list,
            scale,
            hovered: hovered.as_ref(),
        };
        let mut ctx = ctx;

        if !view.screen.is_overlay() {
            // The title lets the world show through; every other page covers it.
            let (top, bottom) = if view.screen == Screen::Title {
                (TITLE_TOP, TITLE_BOTTOM)
            } else {
                (BACKDROP_TOP, BACKDROP_BOTTOM)
            };
            ctx.list
                .panel(UiRect::new(0.0, 0.0, w, h), 0.0, top, bottom);
            ctx.top_bar(view, w);
        }
        match view.screen {
            Screen::Title => draw_title(&mut ctx, view, w, h, self.clock),
            Screen::Login => self.draw_login(&mut ctx, view, w, h),
            Screen::Characters => draw_characters(&mut ctx, view, w, h),
            Screen::Hub => draw_hub(&mut ctx, view, w, h, self.clock),
            Screen::Dungeons => draw_dungeons(&mut ctx, view, w, h),
            Screen::Roles => draw_roles(&mut ctx, view, w, h),
            Screen::Workshop => draw_workshop(&mut ctx, view, w, h),
            Screen::ReadyCheck => draw_ready_check(&mut ctx, view, w, h),
            Screen::KeySelect => draw_key_select(&mut ctx, view, w, h),
            Screen::Party => draw_party(&mut ctx, view, w, h),
            Screen::InWorld => draw_in_world(&mut ctx, view, w, h),
        }
        // Over the world a refusal is a toast; a full page keeps the sentence on screen.
        if let Some(error) = &view.error
            && !view.screen.is_overlay()
            && view.screen != Screen::Login
        {
            ctx.banner(error, StatusLevel::Bad, w, h - ctx.scale.px(120.0));
        }
        draw_toasts(&mut ctx, view, w, h);
        if let Some(text) = &view.reconnecting {
            ctx.reconnecting(text, w, h, self.clock);
        }
    }

    fn draw_login(&self, ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
        let scale = ctx.scale;
        let px = move |v: f32| scale.px(v);
        // The game's name over the form.
        ctx.text_styled(
            (w * 0.5, h * 0.12),
            "SHADOWS OF THE CRYPT",
            GOLD,
            UiFont::Display,
            46.0,
            TextAlign::Center,
        );
        ctx.text(
            (w * 0.5, h * 0.12 + px(60.0)),
            "Online",
            DIM,
            UiFont::Medium,
            18.0,
            TextAlign::Center,
        );

        let panel = login_panel(w, h, ctx.scale, self.register);
        ctx.panel(panel);
        let title = if self.register {
            "CREATE ACCOUNT"
        } else {
            "SIGN IN"
        };
        ctx.text(
            (panel.x + panel.width * 0.5, panel.y + px(34.0)),
            title,
            GOLD,
            UiFont::Display,
            28.0,
            TextAlign::Center,
        );

        let mut y = panel.y + px(118.0);
        let x = panel.x + px(36.0);
        let width = panel.width - px(72.0);
        ctx.text(
            (x, y - px(26.0)),
            "Email",
            DIM,
            UiFont::Medium,
            15.0,
            TextAlign::Left,
        );
        self.draw_field(
            ctx,
            FieldId::Email,
            UiRect::new(x, y, width, px(44.0)),
            &self.email,
        );
        y += px(76.0);
        if self.register {
            ctx.text(
                (x, y - px(26.0)),
                "Display name",
                DIM,
                UiFont::Medium,
                15.0,
                TextAlign::Left,
            );
            self.draw_field(
                ctx,
                FieldId::DisplayName,
                UiRect::new(x, y, width, px(44.0)),
                &self.display_name,
            );
            y += px(76.0);
        }
        ctx.text(
            (x, y - px(26.0)),
            "Password",
            DIM,
            UiFont::Medium,
            15.0,
            TextAlign::Left,
        );
        self.draw_field(
            ctx,
            FieldId::Password,
            UiRect::new(x, y, width, px(44.0)),
            &self.password,
        );
        y += px(60.0);
        // Remember me.
        let tick = UiRect::new(x, y + px(3.0), px(22.0), px(22.0));
        ctx.list.panel_stroke(tick, px(4.0), px(2.0), GOLD_EDGE);
        if self.remember {
            ctx.list.panel(tick.inset(px(5.0)), px(2.0), GOLD, GOLD);
        }
        ctx.text(
            (tick.right() + px(10.0), y + px(4.0)),
            "Remember me on this computer",
            TEXT,
            UiFont::Regular,
            16.0,
            TextAlign::Left,
        );
        y += px(44.0);
        let busy = view.busy.is_some();
        let label = if busy {
            view.busy.as_deref().unwrap_or("Working...")
        } else if self.register {
            "Create Account"
        } else {
            "Sign In"
        };
        let ready =
            !busy && !self.email.text().trim().is_empty() && !self.password.text().is_empty();
        ctx.button(
            UiRect::new(x, y, width, px(48.0)),
            label,
            ButtonKind::Primary,
            &Hit::Submit,
            ready,
        );
        y += px(60.0);
        let toggle = if self.register {
            "I already have an account"
        } else {
            "Create a new account"
        };
        ctx.link(
            UiRect::new(x, y, width, px(36.0)),
            toggle,
            &Hit::ToggleRegister,
        );
        y += px(50.0);
        if let Some(error) = &view.error {
            for (line_index, line) in UiFont::Medium
                .wrap(error, ctx.scale.px(15.0), width)
                .iter()
                .enumerate()
            {
                ctx.text(
                    (
                        panel.x + panel.width * 0.5,
                        y + px(20.0) * line_index as f32,
                    ),
                    line,
                    BAD,
                    UiFont::Medium,
                    15.0,
                    TextAlign::Center,
                );
            }
        }
        ctx.button(
            quit_button(h, ctx.scale),
            "Quit",
            ButtonKind::Secondary,
            &Hit::Quit,
            true,
        );
    }

    fn draw_field(&self, ctx: &mut Painter<'_>, id: FieldId, rect: UiRect, field: &TextField) {
        let scale = ctx.scale;
        let px = move |v: f32| scale.px(v);
        let focused = self.focus == Some(id);
        ctx.list.panel(rect, px(6.0), FIELD, FIELD);
        ctx.list.panel_stroke(
            rect,
            px(6.0),
            px(if focused { 2.0 } else { 1.0 }),
            if focused {
                GOLD
            } else {
                [0.30, 0.32, 0.38, 1.0]
            },
        );
        let size = 18.0;
        let baseline_top = rect.y + (rect.height - UiFont::Regular.line_height(px(size))) * 0.5;
        let shown = field.display();
        ctx.list.label(
            (rect.x + px(14.0), baseline_top),
            &shown,
            TEXT,
            UiFont::Regular,
            px(size),
            TextAlign::Left,
        );
        if focused && (self.clock * 2.0).fract() < 0.6 {
            let caret_x =
                rect.x + px(14.0) + UiFont::Regular.width(&field.display_before_caret(), px(size));
            ctx.list.rect(
                UiRect::new(caret_x, rect.y + px(10.0), px(2.0), rect.height - px(20.0)),
                GOLD,
            );
        }
    }
}

/// Drawing helpers over one draw list.
struct Painter<'a> {
    list: &'a mut UiDrawList,
    scale: UiScale,
    hovered: Option<&'a Hit>,
}

impl Painter<'_> {
    fn text(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: UiColor,
        font: UiFont,
        size: f32,
        align: TextAlign,
    ) {
        self.list
            .label(origin, text, color, font, self.scale.px(size), align);
    }

    fn text_styled(
        &mut self,
        origin: (f32, f32),
        text: &str,
        color: UiColor,
        font: UiFont,
        size: f32,
        align: TextAlign,
    ) {
        let px = self.scale.px(1.0);
        self.list.styled_label(
            origin,
            text,
            color,
            font,
            self.scale.px(size),
            align,
            TextStyle::outlined([0.0, 0.0, 0.0, 0.8], 2.0 * px, 2.0 * px, 0.3 * px),
        );
    }

    fn panel(&mut self, rect: UiRect) {
        let px = self.scale.px(1.0);
        self.list
            .shadow(rect, 12.0 * px, 24.0 * px, [0.0, 0.0, 0.0, 0.55]);
        self.list.panel(rect, 12.0 * px, PANEL_TOP, PANEL_BOTTOM);
        self.list.panel_stroke(rect, 12.0 * px, 1.5 * px, GOLD_EDGE);
    }

    fn is_hovered(&self, hit: &Hit) -> bool {
        self.hovered == Some(hit)
    }

    fn button(&mut self, rect: UiRect, label: &str, kind: ButtonKind, hit: &Hit, enabled: bool) {
        let px = self.scale.px(1.0);
        let hovered = enabled && self.is_hovered(hit);
        let (top, bottom, text) = match (kind, enabled) {
            (_, false) => ([0.13, 0.13, 0.15, 1.0], [0.10, 0.10, 0.12, 1.0], FAINT),
            (ButtonKind::Primary, true) => {
                if hovered {
                    (
                        [0.72, 0.52, 0.18, 1.0],
                        [0.50, 0.33, 0.09, 1.0],
                        [1.0, 0.97, 0.88, 1.0],
                    )
                } else {
                    (
                        [0.58, 0.40, 0.13, 1.0],
                        [0.40, 0.26, 0.07, 1.0],
                        [1.0, 0.95, 0.84, 1.0],
                    )
                }
            }
            (ButtonKind::Secondary, true) => {
                if hovered {
                    ([0.24, 0.27, 0.35, 1.0], [0.16, 0.18, 0.24, 1.0], TEXT)
                } else {
                    ([0.17, 0.19, 0.25, 1.0], [0.11, 0.12, 0.16, 1.0], TEXT)
                }
            }
            (ButtonKind::Danger, true) => {
                if hovered {
                    ([0.62, 0.20, 0.17, 1.0], [0.42, 0.11, 0.09, 1.0], TEXT)
                } else {
                    ([0.46, 0.15, 0.13, 1.0], [0.30, 0.08, 0.07, 1.0], TEXT)
                }
            }
        };
        if hovered {
            self.list
                .glow(rect, 8.0 * px, 10.0 * px, [0.96, 0.78, 0.40, 0.18]);
        }
        self.list.panel(rect, 8.0 * px, top, bottom);
        self.list.panel_stroke(
            rect,
            8.0 * px,
            1.0 * px,
            [1.0, 0.9, 0.7, if enabled { 0.25 } else { 0.08 }],
        );
        let size = 17.0;
        let top_y =
            rect.y + (rect.height - UiFont::SemiBold.line_height(self.scale.px(size))) * 0.5;
        self.text(
            (rect.x + rect.width * 0.5, top_y),
            label,
            text,
            UiFont::SemiBold,
            size,
            TextAlign::Center,
        );
    }

    fn link(&mut self, rect: UiRect, label: &str, hit: &Hit) {
        let color = if self.is_hovered(hit) { GOLD } else { DIM };
        let top_y = rect.y + (rect.height - UiFont::Medium.line_height(self.scale.px(16.0))) * 0.5;
        self.text(
            (rect.x + rect.width * 0.5, top_y),
            label,
            color,
            UiFont::Medium,
            16.0,
            TextAlign::Center,
        );
    }

    fn top_bar(&mut self, view: &FrontEndView, w: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        if let Some((level, text)) = &view.connection {
            let dot = (px(28.0), px(35.0));
            self.list.disc(dot, px(6.0), status_color(*level));
            self.text(
                (px(44.0), px(25.0)),
                text,
                DIM,
                UiFont::Medium,
                15.0,
                TextAlign::Left,
            );
        }
        if let Some(account) = &view.account
            && shows_account(view)
        {
            self.text(
                (w - px(160.0), px(25.0)),
                account,
                TEXT,
                UiFont::SemiBold,
                16.0,
                TextAlign::Right,
            );
            self.button(
                UiRect::new(w - px(140.0), px(18.0), px(120.0), px(34.0)),
                "Log Out",
                ButtonKind::Secondary,
                &Hit::Logout,
                true,
            );
        }
    }

    fn heading(&mut self, title: &str, subtitle: &str, w: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        self.text_styled(
            (w * 0.5, px(70.0)),
            title,
            GOLD,
            UiFont::Display,
            36.0,
            TextAlign::Center,
        );
        self.text(
            (w * 0.5, px(122.0)),
            subtitle,
            DIM,
            UiFont::Regular,
            17.0,
            TextAlign::Center,
        );
    }

    fn banner(&mut self, text: &str, level: StatusLevel, w: f32, y: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        let width = UiFont::Medium.width(text, px(16.0)) + px(48.0);
        let rect = UiRect::new((w - width) * 0.5, y, width, px(40.0));
        self.list.panel(
            rect,
            px(8.0),
            [0.08, 0.06, 0.07, 0.95],
            [0.05, 0.04, 0.05, 0.95],
        );
        self.list
            .panel_stroke(rect, px(8.0), px(1.5), status_color(level));
        let top_y = rect.y + (rect.height - UiFont::Medium.line_height(px(16.0))) * 0.5;
        self.text(
            (w * 0.5, top_y),
            text,
            TEXT,
            UiFont::Medium,
            16.0,
            TextAlign::Center,
        );
    }

    fn reconnecting(&mut self, text: &str, w: f32, h: f32, clock: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        self.list
            .rect(UiRect::new(0.0, 0.0, w, h), [0.0, 0.0, 0.0, 0.55]);
        let panel = centered(w, h, px(520.0), px(170.0));
        self.panel(panel);
        self.text(
            (w * 0.5, panel.y + px(28.0)),
            "Connection lost",
            BAD,
            UiFont::Display,
            28.0,
            TextAlign::Center,
        );
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a small animation phase"
        )]
        let dots = ".".repeat(((clock * 2.0) as usize % 3) + 1);
        self.text(
            (w * 0.5, panel.y + px(84.0)),
            &format!("{text}{dots}"),
            TEXT,
            UiFont::Medium,
            18.0,
            TextAlign::Center,
        );
        self.text(
            (w * 0.5, panel.y + px(118.0)),
            "Your character is safe. The party is waiting for you.",
            DIM,
            UiFont::Regular,
            15.0,
            TextAlign::Center,
        );
    }

    fn role_chip(&mut self, role: Role, x: f32, y: f32) -> f32 {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        let width = UiFont::SemiBold.width(role.name(), px(13.0)) + px(18.0);
        let rect = UiRect::new(x, y, width, px(22.0));
        let color = role_color(role);
        self.list.panel(
            rect,
            px(11.0),
            [color[0] * 0.35, color[1] * 0.35, color[2] * 0.35, 0.95],
            [color[0] * 0.22, color[1] * 0.22, color[2] * 0.22, 0.95],
        );
        self.list.panel_stroke(rect, px(11.0), px(1.0), color);
        self.text(
            (rect.x + width * 0.5, rect.y + px(2.0)),
            role.name(),
            color,
            UiFont::SemiBold,
            13.0,
            TextAlign::Center,
        );
        width
    }
}

/// The in-world "Leave Party" control: under the minimap, clear of the HUD's top-right buttons.
fn in_world_leave(w: f32, scale: UiScale) -> UiRect {
    UiRect::new(
        w - scale.px(160.0),
        scale.px(360.0),
        scale.px(140.0),
        scale.px(32.0),
    )
}

fn centered(w: f32, h: f32, width: f32, height: f32) -> UiRect {
    UiRect::new((w - width) * 0.5, (h - height) * 0.5, width, height)
}

fn login_panel(w: f32, h: f32, scale: UiScale, register: bool) -> UiRect {
    let height = if register { 560.0 } else { 484.0 };
    let panel = centered(w, h, scale.px(460.0), scale.px(height));
    UiRect::new(panel.x, panel.y + scale.px(40.0), panel.width, panel.height)
}

fn footer_buttons(w: f32, h: f32, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    (
        UiRect::new(w * 0.5 - px(250.0), h - px(96.0), px(220.0), px(50.0)),
        UiRect::new(w * 0.5 + px(30.0), h - px(96.0), px(220.0), px(50.0)),
    )
}

fn character_cards(count: usize, w: f32, h: f32, scale: UiScale) -> Vec<UiRect> {
    let px = |v: f32| scale.px(v);
    let card = (px(280.0), px(440.0));
    let gap = px(28.0);
    let per_row = (((w - px(80.0)) + gap) / (card.0 + gap)).floor().max(1.0) as usize;
    let rows = count.div_ceil(per_row).max(1);
    let total_height = rows as f32 * card.1 + (rows as f32 - 1.0) * gap;
    // Centred, but never under the heading nor over the footer's Enter World button.
    let lowest = (h - px(112.0) - total_height).max(px(150.0));
    let top = ((h - total_height) * 0.5).clamp(px(150.0), lowest);
    (0..count)
        .map(|index| {
            let row = index / per_row;
            let in_row = (count - row * per_row).min(per_row);
            let column = index % per_row;
            let row_width = in_row as f32 * card.0 + (in_row as f32 - 1.0) * gap;
            UiRect::new(
                (w - row_width) * 0.5 + column as f32 * (card.0 + gap),
                top + row as f32 * (card.1 + gap),
                card.0,
                card.1,
            )
        })
        .collect()
}

fn dungeon_panels(w: f32, h: f32, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let top = px(170.0);
    let height = h - top - px(130.0);
    let total = (w - px(120.0)).min(px(1200.0));
    let left = (w - total) * 0.5;
    (
        UiRect::new(left, top, total * 0.36, height),
        UiRect::new(
            left + total * 0.36 + px(24.0),
            top,
            total * 0.64 - px(24.0),
            height,
        ),
    )
}

fn role_tiles(w: f32, h: f32, scale: UiScale) -> Vec<UiRect> {
    let px = |v: f32| scale.px(v);
    let tile = (px(300.0), px(300.0));
    let gap = px(36.0);
    let total = tile.0 * 3.0 + gap * 2.0;
    let top = (h - tile.1) * 0.5;
    (0..3)
        .map(|i| {
            UiRect::new(
                (w - total) * 0.5 + i as f32 * (tile.0 + gap),
                top,
                tile.0,
                tile.1,
            )
        })
        .collect()
}

/// Where the first member's row starts inside the party panel (below the key information).
const PARTY_ROWS_TOP: f32 = 196.0;
/// Where the first member's row starts inside the key-selection panel.
const KEY_ROWS_TOP: f32 = 150.0;

fn member_row(panel: UiRect, index: usize, top: f32, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        panel.x + px(30.0),
        panel.y + px(top) + index as f32 * px(76.0),
        panel.width - px(60.0),
        px(64.0),
    )
}

fn kick_button(row: UiRect, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        row.right() - px(106.0),
        row.y + px(14.0),
        px(90.0),
        px(34.0),
    )
}

/// Whether the account name and sign-out sit in the top bar.
fn shows_account(view: &FrontEndView) -> bool {
    view.account.is_some()
        && !matches!(
            view.screen,
            Screen::Title | Screen::Login | Screen::Hub | Screen::InWorld
        )
}

/// Whether the local player may start the dungeon now: the key's owner, with everyone here.
fn start_enabled(view: &FrontEndView, party: &PartyCard) -> bool {
    party.i_lead
        && party.i_own_key
        && party.members.iter().all(|m| m.present)
        && view.busy.is_none()
}

fn quit_button(h: f32, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(px(20.0), h - px(54.0), px(110.0), px(34.0))
}

fn title_continue(w: f32, h: f32, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(w * 0.5 - px(150.0), h - px(190.0), px(300.0), px(56.0))
}

fn ready_panel(w: f32, h: f32, scale: UiScale) -> UiRect {
    centered(w, h, scale.px(640.0), scale.px(400.0))
}

fn ready_buttons(panel: UiRect, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    (
        UiRect::new(
            panel.x + px(40.0),
            panel.bottom() - px(76.0),
            px(220.0),
            px(48.0),
        ),
        UiRect::new(
            panel.right() - px(260.0),
            panel.bottom() - px(76.0),
            px(220.0),
            px(48.0),
        ),
    )
}

fn key_panel(w: f32, h: f32, scale: UiScale) -> UiRect {
    centered(w, h, scale.px(760.0), scale.px(500.0))
}

/// Leave Party and Insert Key.
fn key_buttons(panel: UiRect, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    (
        UiRect::new(
            panel.x + px(40.0),
            panel.bottom() - px(72.0),
            px(180.0),
            px(48.0),
        ),
        UiRect::new(
            panel.right() - px(320.0),
            panel.bottom() - px(72.0),
            px(280.0),
            px(48.0),
        ),
    )
}

fn party_panel(w: f32, h: f32, scale: UiScale) -> UiRect {
    centered(w, h, scale.px(760.0), scale.px(560.0))
}

struct PartyButtons {
    leave: UiRect,
    withdraw: UiRect,
    start: UiRect,
}

fn party_buttons(panel: UiRect, scale: UiScale) -> PartyButtons {
    let px = |v: f32| scale.px(v);
    let y = panel.bottom() - px(72.0);
    PartyButtons {
        leave: UiRect::new(panel.x + px(40.0), y, px(180.0), px(48.0)),
        withdraw: UiRect::new(panel.x + px(240.0), y, px(190.0), px(48.0)),
        start: UiRect::new(panel.right() - px(260.0), y, px(220.0), px(48.0)),
    }
}

/// The hub panel and its five buttons.
struct HubRects {
    panel: UiRect,
    finder: UiRect,
    equipment: UiRect,
    workshop: UiRect,
    change: UiRect,
    logout: UiRect,
}

/// The hub panel in the bottom-right corner (clear of the minimap above it and the action bars
/// beside it), tall enough for `resource_lines` resources and, while something is in flight, a
/// status line.
fn hub_rects(resource_lines: usize, status_line: bool, w: f32, h: f32, scale: UiScale) -> HubRects {
    let px = |v: f32| scale.px(v);
    let lines = resource_lines.max(1) as f32;
    let width = px(280.0);
    let x = w - width - px(20.0);
    let status = if status_line { px(22.0) } else { 0.0 };
    let to_buttons = px(112.0) + lines * px(19.0) + px(8.0) + status;
    let height = to_buttons + px(38.0) * 5.0 - px(6.0) + px(14.0);
    let y = h - height - px(20.0);
    let button = |i: f32| {
        UiRect::new(
            x + px(16.0),
            y + to_buttons + i * px(38.0),
            width - px(32.0),
            px(32.0),
        )
    };
    HubRects {
        panel: UiRect::new(x, y, width, height),
        finder: button(0.0),
        equipment: button(1.0),
        workshop: button(2.0),
        change: button(3.0),
        logout: button(4.0),
    }
}

/// The queue panel in the bottom-left corner of the hub, and its Leave Queue button.
fn queue_panel(h: f32, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let panel = UiRect::new(px(20.0), h - px(104.0) - px(20.0), px(380.0), px(104.0));
    let leave = UiRect::new(
        panel.right() - px(150.0),
        panel.bottom() - px(48.0),
        px(134.0),
        px(34.0),
    );
    (panel, leave)
}

/// How many cells the salvage tray draws: two rows of the bag's width.
const TRAY_CELLS: usize = INVENTORY_COLUMNS * 2;

/// The workshop's three panels: the bag on the left, the salvage tray and the exchanges stacked
/// on the right.
struct WorkshopPanels {
    bag: UiRect,
    salvage: UiRect,
    exchange: UiRect,
}

fn workshop_panels(w: f32, h: f32, scale: UiScale) -> WorkshopPanels {
    let px = |v: f32| scale.px(v);
    let top = px(156.0);
    let height = h - top - px(108.0);
    let total = (w - px(120.0)).min(px(1200.0));
    let left = (w - total) * 0.5;
    // Wide enough for a row of bag cells and the panel's padding either side.
    let bag_width = cells_width(INVENTORY_COLUMNS, scale) + px(40.0);
    let right = left + bag_width + px(24.0);
    let right_width = total - bag_width - px(24.0);
    let salvage_height = px(230.0);
    WorkshopPanels {
        bag: UiRect::new(left, top, bag_width, height),
        salvage: UiRect::new(right, top, right_width, salvage_height),
        exchange: UiRect::new(
            right,
            top + salvage_height + px(12.0),
            right_width,
            height - salvage_height - px(12.0),
        ),
    }
}

/// The width of `columns` item cells and the gaps between them: the character sheet's cell.
fn cells_width(columns: usize, scale: UiScale) -> f32 {
    use super::character_menu::{BAG_CELL, BAG_CELL_GAP};
    scale.px(BAG_CELL) * columns as f32 + scale.px(BAG_CELL_GAP) * (columns as f32 - 1.0)
}

/// The rect of the item cell in `column`, `row` of a grid whose first cell is at `origin`.
fn item_cell(origin: (f32, f32), column: usize, row: usize, scale: UiScale) -> UiRect {
    use super::character_menu::{BAG_CELL, BAG_CELL_GAP};
    let cell = scale.px(BAG_CELL);
    let pitch = cell + scale.px(BAG_CELL_GAP);
    UiRect::new(
        origin.0 + pitch * column as f32,
        origin.1 + pitch * row as f32,
        cell,
        cell,
    )
}

/// Bag cell `index` in the bag panel.
fn bag_cell_rect(bag: UiRect, index: usize, scale: UiScale) -> UiRect {
    let origin = (bag.x + scale.px(20.0), bag.y + scale.px(52.0));
    item_cell(
        origin,
        index % INVENTORY_COLUMNS,
        index / INVENTORY_COLUMNS,
        scale,
    )
}

/// Tray cell `position` in the salvage panel.
fn tray_cell_rect(salvage: UiRect, position: usize, scale: UiScale) -> UiRect {
    let origin = (salvage.x + scale.px(20.0), salvage.y + scale.px(46.0));
    item_cell(
        origin,
        position % INVENTORY_COLUMNS,
        position / INVENTORY_COLUMNS,
        scale,
    )
}

/// The Salvage button, at the bottom right of the salvage panel.
fn salvage_button(salvage: UiRect, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        salvage.x + px(20.0) + cells_width(INVENTORY_COLUMNS, scale) - px(150.0),
        salvage.bottom() - px(48.0),
        px(150.0),
        px(36.0),
    )
}

/// How many rows of the exchange list fit in `panel`.
fn workshop_rows(panel: UiRect, scale: UiScale) -> usize {
    let rows = ((panel.height - scale.px(64.0)) / scale.px(44.0)).floor();
    if rows < 1.0 {
        return 0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive row count"
    )]
    let rows = rows as usize;
    rows
}

fn workshop_row(panel: UiRect, index: usize, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        panel.x + px(12.0),
        panel.y + px(52.0) + px(44.0) * index as f32,
        panel.width - px(24.0),
        px(38.0),
    )
}

fn workshop_button(row: UiRect, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        row.right() - px(124.0),
        row.y + px(4.0),
        px(112.0),
        px(30.0),
    )
}

fn draw_characters(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading(
        "CHOOSE YOUR CHARACTER",
        "Your progress, gear and talents travel with each character.",
        w,
    );
    if view.characters.is_empty() {
        let text = view.busy.as_deref().unwrap_or("Loading characters...");
        ctx.text(
            (w * 0.5, h * 0.5),
            text,
            DIM,
            UiFont::Medium,
            18.0,
            TextAlign::Center,
        );
    }
    for (index, (card, rect)) in view
        .characters
        .iter()
        .zip(character_cards(view.characters.len(), w, h, ctx.scale))
        .enumerate()
    {
        let selected = view.selected_character == Some(index);
        let hovered = ctx.is_hovered(&Hit::Character(index));
        if selected {
            ctx.list
                .glow(rect, px(14.0), px(18.0), [0.96, 0.78, 0.40, 0.30]);
        }
        ctx.list.panel(
            rect,
            px(14.0),
            if hovered {
                [0.15, 0.16, 0.21, 0.98]
            } else {
                PANEL_TOP
            },
            PANEL_BOTTOM,
        );
        ctx.list.panel_stroke(
            rect,
            px(14.0),
            px(if selected { 2.5 } else { 1.0 }),
            if selected { SELECTED } else { GOLD_EDGE },
        );
        let cx = rect.x + rect.width * 0.5;
        // A crest: the class initial in a ring.
        let crest = (cx, rect.y + px(78.0));
        ctx.list.disc(crest, px(46.0), [0.20, 0.17, 0.12, 1.0]);
        ctx.list.disc(crest, px(42.0), [0.08, 0.08, 0.11, 1.0]);
        let initial: String = card.class.chars().take(1).collect();
        ctx.text_styled(
            (cx, crest.1 - px(26.0)),
            &initial,
            GOLD,
            UiFont::Display,
            40.0,
            TextAlign::Center,
        );
        ctx.text_styled(
            (cx, rect.y + px(140.0)),
            &card.name,
            TEXT,
            UiFont::SemiBold,
            24.0,
            TextAlign::Center,
        );
        ctx.text(
            (cx, rect.y + px(176.0)),
            &format!("Level {} {}", card.level, card.class),
            DIM,
            UiFont::Medium,
            16.0,
            TextAlign::Center,
        );
        // Role chips, centred.
        let widths: f32 = card
            .roles
            .iter()
            .map(|r| UiFont::SemiBold.width(r.name(), px(13.0)) + px(18.0))
            .sum::<f32>()
            + px(8.0) * card.roles.len().saturating_sub(1) as f32;
        let mut x = cx - widths * 0.5;
        for role in &card.roles {
            x += ctx.role_chip(*role, x, rect.y + px(208.0)) + px(8.0);
        }
        // Stats.
        let line = |ctx: &mut Painter<'_>, y: f32, label: &str, value: &str| {
            ctx.text(
                (rect.x + px(28.0), y),
                label,
                DIM,
                UiFont::Medium,
                15.0,
                TextAlign::Left,
            );
            ctx.text(
                (rect.right() - px(28.0), y),
                value,
                TEXT,
                UiFont::SemiBold,
                15.0,
                TextAlign::Right,
            );
        };
        line(
            ctx,
            rect.y + px(256.0),
            "Gear Score",
            &card.gear_score.to_string(),
        );
        line(
            ctx,
            rect.y + px(284.0),
            "Equipment",
            &format!(
                "{} / {} slots",
                card.equipped,
                engine_game::character::EquipSlot::COUNT
            ),
        );
        // The dungeon key, and what the character holds.
        match &card.key {
            Some(key) => ctx.text(
                (cx, rect.y + px(312.0)),
                &key.text(),
                GOLD,
                UiFont::SemiBold,
                14.0,
                TextAlign::Center,
            ),
            None => ctx.text(
                (cx, rect.y + px(312.0)),
                "No dungeon key",
                FAINT,
                UiFont::Medium,
                14.0,
                TextAlign::Center,
            ),
        }
        line(
            ctx,
            rect.y + px(338.0),
            "Resources",
            &card.resources.to_string(),
        );
        // Experience bar.
        let bar = UiRect::new(
            rect.x + px(28.0),
            rect.y + px(390.0),
            rect.width - px(56.0),
            px(10.0),
        );
        xp_bar(ctx, bar, card.experience, card.next_level);
        ctx.text(
            (cx, rect.y + px(406.0)),
            &xp_text(card.experience, card.next_level),
            FAINT,
            UiFont::Medium,
            13.0,
            TextAlign::Center,
        );
    }
    let (_, next) = footer_buttons(w, h, ctx.scale);
    ctx.button(
        next,
        "Enter World",
        ButtonKind::Primary,
        &Hit::Continue,
        view.selected_character.is_some(),
    );
}

fn draw_dungeons(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading(
        "DUNGEON FINDER",
        "Choose a dungeon. The realm finds a Tank, a Healer and a DPS of similar gear.",
        w,
    );
    let (list, detail) = dungeon_panels(w, h, ctx.scale);
    ctx.panel(list);
    ctx.text(
        (list.x + px(20.0), list.y + px(18.0)),
        "DUNGEONS",
        GOLD,
        UiFont::SemiBold,
        17.0,
        TextAlign::Left,
    );
    let character_gs = view
        .selected_character
        .and_then(|i| view.characters.get(i))
        .map(|c| (c.level, c.gear_score));
    for (index, dungeon) in view.dungeons.iter().enumerate() {
        let row = UiRect::new(
            list.x + px(12.0),
            list.y + px(64.0) + px(74.0) * index as f32,
            list.width - px(24.0),
            px(64.0),
        );
        let selected = view.selected_dungeon == Some(index);
        let hovered = ctx.is_hovered(&Hit::Dungeon(index));
        let fill = if selected {
            [0.22, 0.18, 0.11, 1.0]
        } else if hovered {
            [0.15, 0.16, 0.21, 1.0]
        } else {
            [0.09, 0.10, 0.13, 1.0]
        };
        ctx.list.panel(row, px(8.0), fill, fill);
        if selected {
            ctx.list.panel_stroke(row, px(8.0), px(1.5), SELECTED);
        }
        ctx.text(
            (row.x + px(16.0), row.y + px(10.0)),
            &dungeon.name,
            TEXT,
            UiFont::SemiBold,
            18.0,
            TextAlign::Left,
        );
        let meta = format!(
            "{} | Levels {}-{}",
            dungeon.difficulty, dungeon.levels.0, dungeon.levels.1
        );
        ctx.text(
            (row.x + px(16.0), row.y + px(36.0)),
            &meta,
            DIM,
            UiFont::Medium,
            14.0,
            TextAlign::Left,
        );
    }
    ctx.panel(detail);
    if let Some(dungeon) = view.selected_dungeon.and_then(|i| view.dungeons.get(i)) {
        let x = detail.x + px(32.0);
        let width = detail.width - px(64.0);
        let mut y = detail.y + px(26.0);
        ctx.text_styled(
            (x, y),
            &dungeon.name,
            GOLD,
            UiFont::Display,
            32.0,
            TextAlign::Left,
        );
        y += px(54.0);
        ctx.text(
            (x, y),
            &format!(
                "{} | Levels {}-{}",
                dungeon.difficulty, dungeon.levels.0, dungeon.levels.1
            ),
            DIM,
            UiFont::Medium,
            16.0,
            TextAlign::Left,
        );
        y += px(40.0);
        for line in UiFont::Regular.wrap(&dungeon.description, px(16.0), width) {
            ctx.text((x, y), &line, TEXT, UiFont::Regular, 16.0, TextAlign::Left);
            y += px(24.0);
        }
        y += px(20.0);
        let gs_color = match character_gs {
            Some((_, gs)) if gs < dungeon.min_gear_score => BAD,
            Some((_, gs)) if gs < dungeon.recommended_gear_score => BUSY,
            _ => GOOD,
        };
        let info = [
            (
                "Recommended gear score",
                dungeon.recommended_gear_score.to_string(),
                gs_color,
            ),
            ("Group", "1 Tank | 1 Healer | 1 DPS".to_owned(), TEXT),
            (
                "Searching now",
                format!(
                    "{} Tank | {} Healer | {} DPS",
                    dungeon.population[0], dungeon.population[1], dungeon.population[2]
                ),
                TEXT,
            ),
        ];
        for (label, value, color) in info {
            ctx.text((x, y), label, DIM, UiFont::Medium, 15.0, TextAlign::Left);
            ctx.text(
                (x + width, y),
                &value,
                color,
                UiFont::SemiBold,
                15.0,
                TextAlign::Right,
            );
            y += px(30.0);
        }
        if let Some((_, gs)) = character_gs {
            ctx.text(
                (x, y),
                "Your gear score",
                DIM,
                UiFont::Medium,
                15.0,
                TextAlign::Left,
            );
            ctx.text(
                (x + width, y),
                &gs.to_string(),
                gs_color,
                UiFont::SemiBold,
                15.0,
                TextAlign::Right,
            );
            y += px(30.0);
        }
        y += px(14.0);
        ctx.text(
            (x, y),
            "REWARDS",
            GOLD,
            UiFont::SemiBold,
            15.0,
            TextAlign::Left,
        );
        y += px(28.0);
        for reward in &dungeon.rewards {
            ctx.list.disc((x + px(6.0), y + px(10.0)), px(3.0), GOLD);
            ctx.text(
                (x + px(18.0), y),
                reward,
                TEXT,
                UiFont::Regular,
                15.0,
                TextAlign::Left,
            );
            y += px(24.0);
        }
    } else {
        ctx.text(
            (
                detail.x + detail.width * 0.5,
                detail.y + detail.height * 0.5,
            ),
            "Select a dungeon",
            DIM,
            UiFont::Medium,
            18.0,
            TextAlign::Center,
        );
    }
    let (back, next) = footer_buttons(w, h, ctx.scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    ctx.button(
        next,
        "Choose Role",
        ButtonKind::Primary,
        &Hit::Continue,
        view.selected_dungeon.is_some(),
    );
}

fn draw_roles(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let character = view.selected_character.and_then(|i| view.characters.get(i));
    let dungeon = view
        .selected_dungeon
        .and_then(|i| view.dungeons.get(i))
        .map_or("", |d| d.name.as_str());
    ctx.heading(
        "CHOOSE YOUR ROLE",
        &format!(
            "{} | {}",
            character.map_or("", |c| c.name.as_str()),
            dungeon
        ),
        w,
    );
    let playable = character.map(|c| c.roles.clone()).unwrap_or_default();
    for (role, rect) in Role::ALL.into_iter().zip(role_tiles(w, h, ctx.scale)) {
        let can = playable.contains(&role);
        let selected = view.selected_role == Some(role);
        let hovered = can && ctx.is_hovered(&Hit::Role(role));
        let color = role_color(role);
        if selected {
            ctx.list.glow(
                rect,
                px(16.0),
                px(20.0),
                [color[0], color[1], color[2], 0.30],
            );
        }
        let top = if hovered {
            [0.16, 0.17, 0.22, 0.98]
        } else {
            PANEL_TOP
        };
        ctx.list.panel(rect, px(16.0), top, PANEL_BOTTOM);
        ctx.list.panel_stroke(
            rect,
            px(16.0),
            px(if selected { 2.5 } else { 1.0 }),
            if selected {
                color
            } else if can {
                GOLD_EDGE
            } else {
                [0.25, 0.25, 0.28, 1.0]
            },
        );
        let cx = rect.x + rect.width * 0.5;
        let faded = |c: UiColor| {
            if can {
                c
            } else {
                [c[0] * 0.4, c[1] * 0.4, c[2] * 0.4, 1.0]
            }
        };
        ctx.list.disc(
            (cx, rect.y + px(90.0)),
            px(48.0),
            faded([color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 1.0]),
        );
        let letter = match role {
            Role::Tank => "T",
            Role::Healer => "H",
            Role::Dps => "D",
        };
        ctx.text_styled(
            (cx, rect.y + px(62.0)),
            letter,
            faded(color),
            UiFont::Display,
            44.0,
            TextAlign::Center,
        );
        ctx.text_styled(
            (cx, rect.y + px(158.0)),
            role.name(),
            faded(TEXT),
            UiFont::SemiBold,
            26.0,
            TextAlign::Center,
        );
        let blurb = if can {
            role_blurb(role)
        } else {
            "Your character cannot fill this role."
        };
        for (i, line) in UiFont::Regular
            .wrap(blurb, px(15.0), rect.width - px(48.0))
            .iter()
            .enumerate()
        {
            ctx.text(
                (cx, rect.y + px(206.0) + px(22.0) * i as f32),
                line,
                if can { DIM } else { FAINT },
                UiFont::Regular,
                15.0,
                TextAlign::Center,
            );
        }
    }
    let (back, next) = footer_buttons(w, h, ctx.scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    let busy = view.busy.is_some();
    ctx.button(
        next,
        if busy { "Joining..." } else { "Find Group" },
        ButtonKind::Primary,
        &Hit::Continue,
        view.selected_role.is_some() && !busy,
    );
}

/// An experience bar filled to `experience / next_level` (full at the cap, `next_level` 0).
fn xp_bar(ctx: &mut Painter<'_>, bar: UiRect, experience: u64, next_level: u64) {
    let px = ctx.scale.px(1.0);
    #[allow(
        clippy::cast_precision_loss,
        reason = "experience values are far below 2^24"
    )]
    let fraction = if next_level == 0 {
        1.0
    } else {
        experience as f32 / next_level as f32
    };
    ctx.list.panel(
        bar,
        5.0 * px,
        [0.05, 0.05, 0.07, 1.0],
        [0.05, 0.05, 0.07, 1.0],
    );
    ctx.list.panel(
        bar.fraction_of_width(fraction.clamp(0.0, 1.0)),
        5.0 * px,
        [0.55, 0.40, 0.95, 1.0],
        [0.38, 0.26, 0.75, 1.0],
    );
}

fn xp_text(experience: u64, next_level: u64) -> String {
    if next_level == 0 {
        "Max level".to_owned()
    } else {
        format!("{experience} / {next_level} XP")
    }
}

fn draw_title(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32, clock: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let intro = &view.intro;
    let top = h * 0.14;
    ctx.text_styled(
        (w * 0.5, top),
        &intro.title,
        GOLD,
        UiFont::Display,
        56.0,
        TextAlign::Center,
    );
    ctx.text(
        (w * 0.5, top + px(84.0)),
        &intro.subtitle,
        DIM,
        UiFont::Medium,
        20.0,
        TextAlign::Center,
    );
    let width = (w - px(80.0)).min(px(720.0));
    let mut y = top + px(160.0);
    for paragraph in &intro.lore {
        for line in UiFont::Regular.wrap(paragraph, px(18.0), width) {
            ctx.text(
                (w * 0.5, y),
                &line,
                TEXT,
                UiFont::Regular,
                18.0,
                TextAlign::Center,
            );
            y += px(28.0);
        }
        y += px(14.0);
    }
    let button = title_continue(w, h, scale);
    ctx.button(
        button,
        &intro.continue_label,
        ButtonKind::Primary,
        &Hit::Continue,
        true,
    );
    // The hint breathes, so the page is seen to be waiting for the player.
    let pulse = 0.55 + 0.45 * (clock * 2.4).sin().abs();
    ctx.text(
        (w * 0.5, button.bottom() + px(16.0)),
        "or press Enter",
        [DIM[0], DIM[1], DIM[2], pulse],
        UiFont::Medium,
        14.0,
        TextAlign::Center,
    );
    if let Some(busy) = &view.busy {
        ctx.text(
            (w * 0.5, button.y - px(34.0)),
            busy,
            BUSY,
            UiFont::Medium,
            16.0,
            TextAlign::Center,
        );
    }
    ctx.button(
        quit_button(h, scale),
        "Quit",
        ButtonKind::Secondary,
        &Hit::Quit,
        true,
    );
}

fn draw_hub(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32, clock: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    if let Some(hub) = &view.hub {
        let rects = hub_rects(hub.resources.len(), view.busy.is_some(), w, h, scale);
        let panel = rects.panel;
        ctx.panel(panel);
        let x = panel.x + px(16.0);
        let width = panel.width - px(32.0);
        ctx.text_styled(
            (x, panel.y + px(10.0)),
            &hub.name,
            TEXT,
            UiFont::SemiBold,
            18.0,
            TextAlign::Left,
        );
        ctx.text(
            (x, panel.y + px(34.0)),
            &format!("Level {} {}", hub.level, hub.class),
            DIM,
            UiFont::Medium,
            13.0,
            TextAlign::Left,
        );
        xp_bar(
            ctx,
            UiRect::new(x, panel.y + px(56.0), width, px(8.0)),
            hub.experience,
            hub.next_level,
        );
        ctx.text(
            (x, panel.y + px(68.0)),
            &xp_text(hub.experience, hub.next_level),
            FAINT,
            UiFont::Medium,
            12.0,
            TextAlign::Left,
        );
        match &hub.key {
            Some(key) => ctx.text(
                (x, panel.y + px(88.0)),
                &key.text(),
                GOLD,
                UiFont::SemiBold,
                14.0,
                TextAlign::Left,
            ),
            None => ctx.text(
                (x, panel.y + px(88.0)),
                "No dungeon key",
                FAINT,
                UiFont::Medium,
                14.0,
                TextAlign::Left,
            ),
        }
        let first = panel.y + px(112.0);
        if hub.resources.is_empty() {
            ctx.text(
                (x, first),
                "No resources yet",
                FAINT,
                UiFont::Medium,
                13.0,
                TextAlign::Left,
            );
        }
        for (index, (name, amount)) in hub.resources.iter().enumerate() {
            let y = first + px(19.0) * index as f32;
            ctx.text((x, y), name, DIM, UiFont::Medium, 13.0, TextAlign::Left);
            ctx.text(
                (x + width, y),
                &amount.to_string(),
                TEXT,
                UiFont::SemiBold,
                13.0,
                TextAlign::Right,
            );
        }
        if let Some(busy) = &view.busy {
            ctx.text(
                (
                    x,
                    first + px(19.0) * hub.resources.len().max(1) as f32 + px(6.0),
                ),
                busy,
                BUSY,
                UiFont::Medium,
                13.0,
                TextAlign::Left,
            );
        }
        ctx.button(
            rects.finder,
            "Dungeon Finder",
            ButtonKind::Primary,
            &Hit::DungeonFinder,
            hub.can_find_dungeon,
        );
        ctx.button(
            rects.equipment,
            "Equipment",
            ButtonKind::Secondary,
            &Hit::Equipment,
            true,
        );
        ctx.button(
            rects.workshop,
            "Salvage & Exchange",
            ButtonKind::Secondary,
            &Hit::Workshop,
            true,
        );
        ctx.button(
            rects.change,
            "Change Character",
            ButtonKind::Secondary,
            &Hit::ChangeCharacter,
            hub.can_change_character,
        );
        ctx.button(
            rects.logout,
            "Log Out",
            ButtonKind::Secondary,
            &Hit::Logout,
            true,
        );
    }
    if let Some(queue) = &view.queue {
        draw_queue_panel(ctx, queue, h, clock);
    }
    if let Some((level, text)) = &view.connection
        && *level != StatusLevel::Good
    {
        ctx.text(
            (px(24.0), px(24.0)),
            text,
            status_color(*level),
            UiFont::Medium,
            14.0,
            TextAlign::Left,
        );
    }
}

/// The queue ticket over the hub: where it is, for how long, and the way out.
fn draw_queue_panel(ctx: &mut Painter<'_>, queue: &QueueCard, h: f32, clock: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let (panel, leave) = queue_panel(h, scale);
    ctx.panel(panel);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small animation phase"
    )]
    let dots = ".".repeat(((clock * 2.0) as usize % 3) + 1);
    let (title, color) = if queue.matched {
        ("Match found!".to_owned(), GOOD)
    } else {
        (format!("Searching{dots}"), GOLD)
    };
    ctx.text(
        (panel.x + px(18.0), panel.y + px(12.0)),
        &title,
        color,
        UiFont::SemiBold,
        19.0,
        TextAlign::Left,
    );
    ctx.text(
        (panel.x + px(18.0), panel.y + px(42.0)),
        &format!("{} \u{b7} {}", queue.dungeon, queue.role.name()),
        TEXT,
        UiFont::Medium,
        14.0,
        TextAlign::Left,
    );
    ctx.text(
        (panel.x + px(18.0), panel.y + px(66.0)),
        &format!(
            "In queue {} \u{b7} range +/-{}%",
            clock_text(queue.waited_seconds),
            queue.tolerance_percent
        ),
        DIM,
        UiFont::Medium,
        14.0,
        TextAlign::Left,
    );
    ctx.button(
        leave,
        "Leave Queue",
        ButtonKind::Secondary,
        &Hit::LeaveQueue,
        !queue.matched,
    );
}

/// One item cell, as the character sheet draws a bag cell: a well, or a pocket tinted by the
/// item's quality with its icon, and the stack size when there is more than one.
fn draw_item_cell(
    ctx: &mut Painter<'_>,
    rect: UiRect,
    row: Option<&BagRow>,
    dimmed: bool,
    hit: Option<&Hit>,
) {
    let px = ctx.scale.px(1.0);
    let corner = 6.0 * px;
    let hovered = hit.is_some_and(|hit| ctx.is_hovered(hit));
    let Some(row) = row else {
        ctx.list.panel(rect, corner, FIELD, FIELD);
        ctx.list
            .panel_stroke(rect, corner, px, [0.30, 0.32, 0.38, 1.0]);
        return;
    };
    let quality = super::character_menu::quality_color(row.quality);
    let fade = if dimmed { 0.35 } else { 1.0 };
    let tint = |amount: f32| {
        [
            FIELD[0] + (quality[0] - FIELD[0]) * amount * fade,
            FIELD[1] + (quality[1] - FIELD[1]) * amount * fade,
            FIELD[2] + (quality[2] - FIELD[2]) * amount * fade,
            1.0,
        ]
    };
    ctx.list.panel(rect, corner, tint(0.22), tint(0.08));
    ctx.list.icon(
        rect.inset(rect.width * 0.14),
        super::character_menu::icon_for_item(row.icon),
        [1.0, 1.0, 1.0, fade],
    );
    if row.quantity > 1 {
        ctx.text(
            (rect.right() - 5.0 * px, rect.bottom() - 18.0 * px),
            &row.quantity.to_string(),
            [1.0, 1.0, 1.0, fade],
            UiFont::SemiBold,
            13.0,
            TextAlign::Right,
        );
    }
    if hovered {
        ctx.list
            .glow(rect, corner, 10.0 * px, [0.96, 0.78, 0.40, 0.35]);
    }
    let edge = if hovered {
        GOLD
    } else {
        [quality[0], quality[1], quality[2], 0.85 * fade]
    };
    ctx.list.panel_stroke(
        rect,
        corner,
        if hovered { 2.0 * px } else { 1.5 * px },
        edge,
    );
}

fn draw_workshop(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let empty = WorkshopCard::default();
    let workshop = view.workshop.as_ref().unwrap_or(&empty);
    let holding = if workshop.resources.is_empty() {
        "You hold no resources yet.".to_owned()
    } else {
        let parts: Vec<String> = workshop
            .resources
            .iter()
            .map(|(name, amount)| format!("{name} {amount}"))
            .collect();
        format!("Holding: {}", parts.join("  \u{b7}  "))
    };
    ctx.heading("SALVAGE & EXCHANGE", &holding, w);
    let panels = workshop_panels(w, h, scale);
    let (bag, salvage, exchange) = (panels.bag, panels.salvage, panels.exchange);
    for (panel, title) in [(bag, "BAG"), (salvage, "SALVAGE"), (exchange, "EXCHANGE")] {
        ctx.panel(panel);
        ctx.text(
            (panel.x + px(20.0), panel.y + px(18.0)),
            title,
            GOLD,
            UiFont::SemiBold,
            17.0,
            TextAlign::Left,
        );
    }
    let find = |index: u32| workshop.bag.iter().find(|row| row.index == index);

    // The whole bag, laid out like the character sheet's. What is in the tray stays visible
    // but dimmed, so the player sees where it came from.
    for index in 0..INVENTORY_SLOTS {
        let row = find(u32::try_from(index).unwrap_or(u32::MAX));
        let staged = row.is_some_and(|row| workshop.tray.contains(&row.index));
        let hit = row.map(|row| Hit::BagCell(row.index));
        draw_item_cell(
            ctx,
            bag_cell_rect(bag, index, scale),
            row,
            staged,
            hit.as_ref(),
        );
    }
    let grid_bottom = bag_cell_rect(bag, INVENTORY_SLOTS.saturating_sub(1), scale).bottom();
    let hovered_row = match ctx.hovered {
        Some(Hit::BagCell(index)) => find(*index),
        Some(Hit::TrayCell(position)) => {
            workshop.tray.get(*position).and_then(|index| find(*index))
        }
        _ => None,
    };
    let hint = match hovered_row {
        Some(row) => {
            let name = if row.quantity > 1 {
                format!("{} x{}", row.name, row.quantity)
            } else {
                row.name.clone()
            };
            format!(
                "{name} \u{b7} Item level {} \u{b7} {}",
                row.item_level, row.yields
            )
        }
        None if workshop.bag.is_empty() => "Your bag is empty.".to_owned(),
        None => "Click an item to put it in the salvage tray.".to_owned(),
    };
    ctx.text(
        (bag.x + px(20.0), grid_bottom + px(12.0)),
        &hint,
        if hovered_row.is_some() { TEXT } else { DIM },
        UiFont::Medium,
        14.0,
        TextAlign::Left,
    );

    // The tray: the same cells, filled in the order the player chose.
    for position in 0..workshop.tray_slots.min(TRAY_CELLS) {
        let row = workshop.tray.get(position).and_then(|index| find(*index));
        let hit = row.map(|_| Hit::TrayCell(position));
        draw_item_cell(
            ctx,
            tray_cell_rect(salvage, position, scale),
            row,
            false,
            hit.as_ref(),
        );
    }
    let button = salvage_button(salvage, scale);
    let (line, color) = if workshop.tray_yield.is_empty() {
        ("Nothing in the tray.", DIM)
    } else {
        (workshop.tray_yield.as_str(), TEXT)
    };
    ctx.text(
        (salvage.x + px(20.0), button.y + px(9.0)),
        line,
        color,
        UiFont::SemiBold,
        16.0,
        TextAlign::Left,
    );
    ctx.button(
        button,
        "Salvage",
        ButtonKind::Danger,
        &Hit::SalvageTray,
        !workshop.tray.is_empty() && view.busy.is_none(),
    );
    if workshop.exchanges.is_empty() {
        ctx.text(
            (exchange.x + exchange.width * 0.5, exchange.y + px(90.0)),
            "No exchanges are offered.",
            DIM,
            UiFont::Medium,
            16.0,
            TextAlign::Center,
        );
    }
    for (index, row) in workshop
        .exchanges
        .iter()
        .enumerate()
        .take(workshop_rows(exchange, scale))
    {
        let rect = workshop_row(exchange, index, scale);
        ctx.list.panel(
            rect,
            px(8.0),
            [0.09, 0.10, 0.13, 1.0],
            [0.09, 0.10, 0.13, 1.0],
        );
        ctx.text(
            (rect.x + px(12.0), rect.y + px(11.0)),
            &row.label,
            if row.affordable { TEXT } else { FAINT },
            UiFont::SemiBold,
            14.0,
            TextAlign::Left,
        );
        ctx.button(
            workshop_button(rect, scale),
            "Exchange",
            ButtonKind::Primary,
            &Hit::Exchange(row.id.clone()),
            row.affordable && view.busy.is_none(),
        );
    }
    let (back, _) = footer_buttons(w, h, scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    if let Some(busy) = &view.busy {
        ctx.text(
            (w * 0.5 + px(140.0), back.y + px(15.0)),
            busy,
            BUSY,
            UiFont::Medium,
            16.0,
            TextAlign::Left,
        );
    }
}

fn ready_label(state: ReadyState) -> (&'static str, UiColor) {
    match state {
        ReadyState::Waiting => ("Waiting...", BUSY),
        ReadyState::Ready => ("Ready", GOOD),
        ReadyState::Declined => ("Declined", BAD),
    }
}

fn draw_ready_check(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let panel = ready_panel(w, h, scale);
    ctx.list
        .glow(panel, px(16.0), px(40.0), [0.96, 0.78, 0.40, 0.20]);
    ctx.panel(panel);
    ctx.text_styled(
        (w * 0.5, panel.y + px(26.0)),
        "Dungeon Found!",
        GOLD,
        UiFont::Display,
        34.0,
        TextAlign::Center,
    );
    let Some(party) = &view.party else { return };
    ctx.text(
        (w * 0.5, panel.y + px(82.0)),
        &party.dungeon,
        TEXT,
        UiFont::SemiBold,
        20.0,
        TextAlign::Center,
    );
    let mut y = panel.y + px(128.0);
    for member in &party.members {
        let x = panel.x + px(36.0);
        ctx.role_chip(member.role, x, y);
        let name = if member.me {
            format!("{} (you)", member.character)
        } else {
            member.character.clone()
        };
        ctx.text(
            (x + px(90.0), y + px(1.0)),
            &name,
            TEXT,
            UiFont::Medium,
            17.0,
            TextAlign::Left,
        );
        let name_end = x + px(90.0) + UiFont::Medium.width(&name, px(17.0)) + px(10.0);
        ctx.text(
            (name_end, y + px(3.0)),
            &member.class,
            DIM,
            UiFont::Medium,
            14.0,
            TextAlign::Left,
        );
        ctx.text(
            (panel.right() - px(160.0), y + px(3.0)),
            &format!("Lv {} \u{b7} GS {}", member.level, member.gear_score),
            DIM,
            UiFont::Medium,
            14.0,
            TextAlign::Right,
        );
        let (label, color) = ready_label(member.ready);
        let label = if member.present {
            label
        } else {
            "Disconnected"
        };
        let color = if member.present { color } else { BAD };
        ctx.text(
            (panel.right() - px(36.0), y + px(1.0)),
            label,
            color,
            UiFont::SemiBold,
            17.0,
            TextAlign::Right,
        );
        y += px(40.0);
    }
    let bar = UiRect::new(
        panel.x + px(48.0),
        panel.bottom() - px(110.0),
        panel.width - px(96.0),
        px(8.0),
    );
    ctx.list.panel(
        bar,
        px(4.0),
        [0.05, 0.05, 0.07, 1.0],
        [0.05, 0.05, 0.07, 1.0],
    );
    ctx.list.panel(
        bar.fraction_of_width(party.fraction_left.clamp(0.0, 1.0)),
        px(4.0),
        GOLD,
        [0.7, 0.5, 0.2, 1.0],
    );
    ctx.text(
        (w * 0.5, bar.y - px(28.0)),
        &format!("{} s to answer", party.seconds_left),
        DIM,
        UiFont::Medium,
        14.0,
        TextAlign::Center,
    );
    if party.i_answered {
        ctx.text(
            (w * 0.5, panel.bottom() - px(64.0)),
            "Waiting for players...",
            DIM,
            UiFont::Medium,
            17.0,
            TextAlign::Center,
        );
    } else {
        let (accept, decline) = ready_buttons(panel, scale);
        ctx.button(accept, "Accept", ButtonKind::Primary, &Hit::Accept, true);
        ctx.button(decline, "Decline", ButtonKind::Danger, &Hit::Decline, true);
    }
}

/// One party member's row: role, name, class, level, gear score and their key.
fn draw_member_row(ctx: &mut Painter<'_>, row: UiRect, member: &MemberRow, right_pad: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let fill = if member.me {
        [0.15, 0.14, 0.11, 1.0]
    } else {
        [0.09, 0.10, 0.13, 1.0]
    };
    ctx.list.panel(row, px(10.0), fill, fill);
    ctx.list.panel_stroke(
        row,
        px(10.0),
        px(1.0),
        if member.leader || member.owner {
            SELECTED
        } else {
            [0.25, 0.26, 0.31, 1.0]
        },
    );
    ctx.list.disc(
        (row.x + px(22.0), row.y + row.height * 0.5),
        px(6.0),
        if member.present { GOOD } else { BAD },
    );
    ctx.role_chip(member.role, row.x + px(40.0), row.y + px(10.0));
    let mut name = member.character.clone();
    if member.me {
        name.push_str("  (you)");
    }
    ctx.text(
        (row.x + px(140.0), row.y + px(9.0)),
        &name,
        TEXT,
        UiFont::SemiBold,
        18.0,
        TextAlign::Left,
    );
    let tag = match (member.leader, member.owner) {
        (true, true) => Some("LEADER \u{b7} KEY OWNER"),
        (true, false) => Some("LEADER"),
        (false, true) => Some("KEY OWNER"),
        (false, false) => None,
    };
    if let Some(tag) = tag {
        let x = row.x + px(150.0) + UiFont::SemiBold.width(&name, px(18.0)) + px(14.0);
        ctx.text(
            (x, row.y + px(12.0)),
            tag,
            GOLD,
            UiFont::SemiBold,
            13.0,
            TextAlign::Left,
        );
    }
    ctx.text(
        (row.x + px(40.0), row.y + px(36.0)),
        &format!(
            "{} \u{b7} Lv {} \u{b7} GS {} \u{b7} {}",
            member.class, member.level, member.gear_score, member.player
        ),
        DIM,
        UiFont::Medium,
        14.0,
        TextAlign::Left,
    );
    let right = row.right() - right_pad;
    match member.key_level {
        Some(level) => ctx.text(
            (right, row.y + px(10.0)),
            &format!("Key level {level}"),
            GOLD,
            UiFont::SemiBold,
            15.0,
            TextAlign::Right,
        ),
        None => ctx.text(
            (right, row.y + px(11.0)),
            "No key for this dungeon",
            FAINT,
            UiFont::Medium,
            14.0,
            TextAlign::Right,
        ),
    }
    if !member.present {
        ctx.text(
            (right, row.y + px(36.0)),
            "Disconnected",
            BAD,
            UiFont::SemiBold,
            14.0,
            TextAlign::Right,
        );
    }
}

fn draw_key_select(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let panel = key_panel(w, h, scale);
    ctx.panel(panel);
    let Some(party) = &view.party else { return };
    ctx.text_styled(
        (panel.x + px(30.0), panel.y + px(24.0)),
        "Choose a Key",
        GOLD,
        UiFont::Display,
        30.0,
        TextAlign::Left,
    );
    ctx.text(
        (panel.right() - px(30.0), panel.y + px(34.0)),
        &party.dungeon,
        DIM,
        UiFont::SemiBold,
        17.0,
        TextAlign::Right,
    );
    for (i, line) in UiFont::Regular
        .wrap(
            "Choose whose key to use. The key's owner leads the party and sets the difficulty.",
            px(15.0),
            panel.width - px(60.0),
        )
        .iter()
        .enumerate()
    {
        ctx.text(
            (panel.x + px(30.0), panel.y + px(78.0) + px(22.0) * i as f32),
            line,
            TEXT,
            UiFont::Regular,
            15.0,
            TextAlign::Left,
        );
    }
    for (index, member) in party.members.iter().enumerate() {
        draw_member_row(
            ctx,
            member_row(panel, index, KEY_ROWS_TOP, scale),
            member,
            px(20.0),
        );
    }
    let (leave, insert) = key_buttons(panel, scale);
    let mine = party.my_key();
    let busy = view.busy.is_some();
    ctx.button(
        insert,
        &mine.map_or_else(
            || "Insert My Key".to_owned(),
            |level| format!("Insert My Key (level {level})"),
        ),
        ButtonKind::Primary,
        &Hit::InsertKey,
        mine.is_some() && !busy,
    );
    ctx.button(
        leave,
        "Leave Party",
        ButtonKind::Secondary,
        &Hit::LeaveParty,
        !busy,
    );
    let note = if let Some(busy) = &view.busy {
        Some((busy.clone(), BUSY))
    } else if mine.is_none() {
        Some((format!("You have no key for {}.", party.dungeon), BAD))
    } else {
        None
    };
    if let Some((text, color)) = note {
        ctx.text(
            (panel.right() - px(40.0), panel.bottom() - px(100.0)),
            &text,
            color,
            UiFont::Medium,
            15.0,
            TextAlign::Right,
        );
    }
}

fn draw_party(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let panel = party_panel(w, h, scale);
    ctx.panel(panel);
    let Some(party) = &view.party else { return };
    ctx.text_styled(
        (panel.x + px(30.0), panel.y + px(24.0)),
        "Your Party",
        GOLD,
        UiFont::Display,
        30.0,
        TextAlign::Left,
    );
    ctx.text(
        (panel.right() - px(30.0), panel.y + px(34.0)),
        &party.dungeon,
        TEXT,
        UiFont::SemiBold,
        18.0,
        TextAlign::Right,
    );
    for (i, line) in UiFont::Regular
        .wrap(&party.description, px(14.0), panel.width - px(60.0))
        .iter()
        .take(2)
        .enumerate()
    {
        ctx.text(
            (panel.x + px(30.0), panel.y + px(72.0) + px(20.0) * i as f32),
            line,
            DIM,
            UiFont::Regular,
            14.0,
            TextAlign::Left,
        );
    }
    ctx.text(
        (panel.x + px(30.0), panel.y + px(122.0)),
        &format!("Key level {}", party.key_level),
        GOLD,
        UiFont::SemiBold,
        18.0,
        TextAlign::Left,
    );
    if let Some(owner) = &party.key_owner {
        ctx.text(
            (panel.right() - px(30.0), panel.y + px(124.0)),
            &format!("{owner}'s key"),
            TEXT,
            UiFont::SemiBold,
            16.0,
            TextAlign::Right,
        );
    }
    let roles: Vec<&str> = party.members.iter().map(|m| m.role.name()).collect();
    ctx.text(
        (panel.x + px(30.0), panel.y + px(152.0)),
        &format!("Roles: {}", roles.join(", ")),
        DIM,
        UiFont::Medium,
        14.0,
        TextAlign::Left,
    );
    for (index, member) in party.members.iter().enumerate() {
        let row = member_row(panel, index, PARTY_ROWS_TOP, scale);
        let kickable = party.i_lead && !member.me;
        draw_member_row(
            ctx,
            row,
            member,
            if kickable { px(120.0) } else { px(20.0) },
        );
        if kickable {
            ctx.button(
                kick_button(row, scale),
                "Kick",
                ButtonKind::Danger,
                &Hit::Kick(member.account_id.clone()),
                view.busy.is_none(),
            );
        }
    }
    let buttons = party_buttons(panel, scale);
    let idle = view.busy.is_none();
    if party.i_own_key {
        ctx.button(
            buttons.withdraw,
            "Withdraw Key",
            ButtonKind::Secondary,
            &Hit::WithdrawKey,
            idle,
        );
    }
    if party.i_lead && party.i_own_key {
        ctx.button(
            buttons.start,
            if idle { "Start Dungeon" } else { "Starting..." },
            ButtonKind::Primary,
            &Hit::Start,
            start_enabled(view, party),
        );
    } else {
        ctx.text(
            (panel.right() - px(40.0), panel.bottom() - px(60.0)),
            "Waiting for the key's owner to start...",
            DIM,
            UiFont::Medium,
            16.0,
            TextAlign::Right,
        );
    }
    ctx.button(
        buttons.leave,
        "Leave Party",
        ButtonKind::Secondary,
        &Hit::LeaveParty,
        idle,
    );
    if let Some(busy) = &view.busy {
        ctx.text(
            (w * 0.5, panel.bottom() - px(104.0)),
            busy,
            BUSY,
            UiFont::Medium,
            16.0,
            TextAlign::Center,
        );
    } else if party.i_lead && !party.members.iter().all(|m| m.present) {
        ctx.text(
            (w * 0.5, panel.bottom() - px(104.0)),
            "Waiting for everyone to be connected.",
            BUSY,
            UiFont::Medium,
            15.0,
            TextAlign::Center,
        );
    }
}

fn draw_in_world(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    match view.countdown {
        Some(CountdownDisplay::Seconds(n)) => {
            ctx.text_styled(
                (w * 0.5, h * 0.28),
                &n.to_string(),
                GOLD,
                UiFont::Display,
                120.0,
                TextAlign::Center,
            );
            ctx.text_styled(
                (w * 0.5, h * 0.28 + px(150.0)),
                "The barrier lifts soon",
                TEXT,
                UiFont::SemiBold,
                22.0,
                TextAlign::Center,
            );
        }
        Some(CountdownDisplay::Go) => {
            ctx.text_styled(
                (w * 0.5, h * 0.28),
                "GO!",
                GOLD,
                UiFont::Display,
                120.0,
                TextAlign::Center,
            );
        }
        _ => {}
    }
    if let Some(card) = &view.completion {
        draw_completion(ctx, card, w, h);
    } else {
        if let Some(busy) = &view.busy {
            ctx.banner(busy, StatusLevel::Busy, w, px(14.0));
        }
        ctx.button(
            in_world_leave(w, ctx.scale),
            "Leave Party",
            ButtonKind::Secondary,
            &Hit::LeaveParty,
            true,
        );
    }
    if let Some((level, text)) = &view.connection
        && *level != StatusLevel::Good
    {
        ctx.text(
            (w - px(180.0), px(366.0)),
            text,
            status_color(*level),
            UiFont::Medium,
            14.0,
            TextAlign::Right,
        );
    }
}

/// The lines of the completion panel: label, value, colour.
fn completion_rows(card: &CompletionCard) -> Vec<(String, String, UiColor)> {
    let mut rows = vec![("Key level".to_owned(), card.key_level.to_string(), TEXT)];
    if card.key_owner {
        rows.push(if card.success {
            (
                "Key upgraded".to_owned(),
                format!("{} -> {}", card.key_level, card.key_level_after),
                GOOD,
            )
        } else {
            ("Key".to_owned(), "Your key is unchanged".to_owned(), DIM)
        });
    }
    rows.push(("Time".to_owned(), clock_text(card.duration_seconds), TEXT));
    if card.success {
        rows.push((
            "Experience".to_owned(),
            format!("+{}", card.experience),
            TEXT,
        ));
        for (name, amount) in &card.resources {
            rows.push((name.clone(), format!("+{amount}"), TEXT));
        }
        if let Some(loot) = &card.loot {
            rows.push(("Loot".to_owned(), loot.clone(), GOLD));
        }
    }
    rows
}

/// The completion panel and its button.
fn completion_rects(card: &CompletionCard, w: f32, h: f32, scale: UiScale) -> (UiRect, UiRect) {
    let px = |v: f32| scale.px(v);
    let height = px(176.0) + px(26.0) * completion_rows(card).len() as f32;
    let panel = UiRect::new(
        (w - px(520.0)) * 0.5,
        ((h - height) * 0.3).max(px(16.0)),
        px(520.0),
        height,
    );
    let button = UiRect::new(
        panel.x + px(130.0),
        panel.bottom() - px(62.0),
        panel.width - px(260.0),
        px(44.0),
    );
    (panel, button)
}

fn draw_completion(ctx: &mut Painter<'_>, card: &CompletionCard, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let (panel, button) = completion_rects(card, w, h, scale);
    if card.success {
        ctx.list
            .glow(panel, px(16.0), px(40.0), [0.96, 0.78, 0.40, 0.22]);
    }
    ctx.panel(panel);
    ctx.text_styled(
        (panel.x + panel.width * 0.5, panel.y + px(20.0)),
        if card.success {
            "Dungeon Complete"
        } else {
            "Run Failed"
        },
        if card.success { GOLD } else { BAD },
        UiFont::Display,
        34.0,
        TextAlign::Center,
    );
    ctx.text(
        (panel.x + panel.width * 0.5, panel.y + px(68.0)),
        &card.dungeon,
        TEXT,
        UiFont::SemiBold,
        19.0,
        TextAlign::Center,
    );
    let mut y = panel.y + px(108.0);
    for (label, value, color) in completion_rows(card) {
        ctx.text(
            (panel.x + px(40.0), y),
            &label,
            DIM,
            UiFont::Medium,
            16.0,
            TextAlign::Left,
        );
        ctx.text(
            (panel.right() - px(40.0), y),
            &value,
            color,
            UiFont::SemiBold,
            16.0,
            TextAlign::Right,
        );
        y += px(26.0);
    }
    ctx.button(
        button,
        if card.returning {
            "Saving rewards..."
        } else {
            "Return to Hub"
        },
        ButtonKind::Primary,
        &Hit::ReturnToHub,
        !card.returning,
    );
}

fn draw_toasts(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    // Under the completion panel, which owns the middle of the screen; otherwise, in the
    // world, above the action bars and clear of the unit frames at the top.
    if let Some(card) = &view.completion {
        let mut y = completion_rects(card, w, h, scale).0.bottom() + px(12.0);
        for toast in view.toasts.iter().rev().take(4) {
            ctx.banner(&toast.message, toast.level, w, y);
            y += px(48.0);
        }
        return;
    }
    let mut y = if view.screen.is_overlay() {
        h - px(250.0)
    } else {
        h - px(180.0)
    };
    for toast in view.toasts.iter().rev().take(4) {
        ctx.banner(&toast.message, toast.level, w, y);
        y -= px(48.0);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn input<'a>(
        pointer: (f32, f32),
        pressed: bool,
        typed: &'a [char],
        clipboard: &'a dyn Fn() -> Option<String>,
    ) -> FrontInput<'a> {
        FrontInput {
            pointer,
            pressed,
            typed,
            keys: FieldKeys::default(),
            escape: false,
            clipboard,
        }
    }

    fn center(rect: UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn scale() -> UiScale {
        UiScale::new(1.0)
    }

    /// Where `hit` is laid out in `view`, if it is clickable at all.
    fn rect_of(ui: &FrontEndUi, view: &FrontEndView, hit: &Hit) -> Option<UiRect> {
        ui.layout(view, VIEWPORT, scale())
            .hits
            .iter()
            .find(|(candidate, _)| candidate == hit)
            .map(|(_, rect)| *rect)
    }

    /// Clicks the middle of `hit` and returns what the front-end asked for.
    fn click(ui: &mut FrontEndUi, view: &FrontEndView, hit: &Hit) -> Vec<FrontEndAction> {
        let rect = rect_of(ui, view, hit).expect("the control is laid out");
        let none = || None;
        ui.update(
            &input(center(rect), true, &[], &none),
            view,
            VIEWPORT,
            scale(),
            0.016,
        )
    }

    fn member(me: bool, leader: bool, owner: bool, key: Option<u32>) -> MemberRow {
        MemberRow {
            account_id: if me { "me".into() } else { "other".into() },
            character: "C".into(),
            player: "P".into(),
            class: "Mage".into(),
            role: Role::Tank,
            level: 1,
            gear_score: 40,
            key_level: key,
            ready: ReadyState::Ready,
            present: true,
            leader,
            owner,
            me,
        }
    }

    fn party(members: Vec<MemberRow>, i_lead: bool, i_own_key: bool) -> PartyCard {
        PartyCard {
            dungeon: "Crypt".into(),
            description: "A crypt.".into(),
            key_level: 3,
            key_owner: Some("C".into()),
            i_own_key,
            members,
            i_lead,
            i_answered: true,
            ..PartyCard::default()
        }
    }

    fn view_of(screen: Screen, party: Option<PartyCard>) -> FrontEndView {
        FrontEndView {
            screen,
            party,
            ..FrontEndView::default()
        }
    }

    fn hub_view() -> FrontEndView {
        FrontEndView {
            screen: Screen::Hub,
            account: Some("Sonit".into()),
            hub: Some(HubCard {
                name: "Ember".into(),
                class: "Mage".into(),
                level: 4,
                experience: 120,
                next_level: 400,
                key: Some(KeyCard {
                    dungeon: "Graveyard Crypt".into(),
                    level: 2,
                }),
                resources: vec![("Crypt Dust".into(), 12)],
                can_find_dungeon: true,
                can_change_character: true,
            }),
            ..FrontEndView::default()
        }
    }

    fn queue_card(matched: bool) -> QueueCard {
        QueueCard {
            dungeon: "Graveyard Crypt".into(),
            role: Role::Dps,
            character: "Ember".into(),
            waited_seconds: 75,
            tolerance_percent: 10,
            matched,
        }
    }

    fn completion(success: bool, owner: bool, returning: bool) -> CompletionCard {
        CompletionCard {
            success,
            dungeon: "Graveyard Crypt".into(),
            key_level: 3,
            key_owner: owner,
            key_level_after: if success { 4 } else { 3 },
            duration_seconds: 754,
            experience: if success { 350 } else { 0 },
            resources: if success {
                vec![("Crypt Dust".into(), 11)]
            } else {
                Vec::new()
            },
            loot: success.then(|| "Open the chest to claim your loot".to_owned()),
            returning,
        }
    }

    #[test]
    fn typing_and_enter_signs_in() {
        let mut ui = FrontEndUi::new();
        let view = view_of(Screen::Login, None);
        let none = || None;
        let typed: Vec<char> = "me@x.y\tsecret\r".chars().collect();
        // One character at a time, as frames would deliver them.
        let mut actions = Vec::new();
        for c in typed {
            actions.extend(ui.update(
                &input((0.0, 0.0), false, &[c], &none),
                &view,
                VIEWPORT,
                scale(),
                0.016,
            ));
        }
        assert_eq!(
            actions,
            vec![FrontEndAction::Login {
                email: "me@x.y".into(),
                password: "secret".into(),
                remember: true
            }]
        );
    }

    #[test]
    fn the_title_continues_on_click_enter_and_space() {
        let mut ui = FrontEndUi::new();
        let view = view_of(Screen::Title, None);
        assert_eq!(
            click(&mut ui, &view, &Hit::Continue),
            vec![FrontEndAction::ContinueFromTitle]
        );
        let none = || None;
        for key in ['\r', ' '] {
            let actions = ui.update(
                &input((0.0, 0.0), false, &[key], &none),
                &view,
                VIEWPORT,
                scale(),
                0.016,
            );
            assert_eq!(actions, vec![FrontEndAction::ContinueFromTitle]);
        }
        // Typing on the sign-in form is not a way past the title, and Enter there signs in.
        let login = view_of(Screen::Login, None);
        let actions = ui.update(
            &input((0.0, 0.0), false, &[' '], &none),
            &login,
            VIEWPORT,
            scale(),
            0.016,
        );
        assert!(!actions.contains(&FrontEndAction::ContinueFromTitle));
    }

    #[test]
    fn clicking_the_drawn_button_is_the_action() {
        let mut ui = FrontEndUi::new();
        let mut card = party(vec![], false, false);
        card.i_answered = false;
        card.seconds_left = 20;
        card.fraction_left = 0.5;
        let view = view_of(Screen::ReadyCheck, Some(card));
        assert_eq!(
            click(&mut ui, &view, &Hit::Accept),
            vec![FrontEndAction::Accept]
        );
        assert_eq!(
            click(&mut ui, &view, &Hit::Decline),
            vec![FrontEndAction::Decline]
        );
        // Drawing the same view produces commands and does not panic.
        let mut list = UiDrawList::new();
        ui.draw(&mut list, &view, VIEWPORT, scale());
        assert!(!list.is_empty());
    }

    #[test]
    fn an_answered_ready_check_has_nothing_to_press() {
        let ui = FrontEndUi::new();
        let view = view_of(Screen::ReadyCheck, Some(party(vec![], false, false)));
        assert!(rect_of(&ui, &view, &Hit::Accept).is_none());
        assert!(rect_of(&ui, &view, &Hit::Decline).is_none());
    }

    #[test]
    fn only_the_leader_gets_leader_controls() {
        let ui = FrontEndUi::new();
        let members = vec![
            member(true, false, false, Some(2)),
            member(false, true, true, Some(3)),
        ];
        let hits = ui
            .layout(
                &view_of(Screen::Party, Some(party(members.clone(), false, false))),
                VIEWPORT,
                scale(),
            )
            .hits;
        assert!(
            !hits
                .iter()
                .any(|(hit, _)| matches!(hit, Hit::Start | Hit::Kick(_) | Hit::WithdrawKey))
        );
        let mut members = members;
        members[0].leader = true;
        members[0].owner = true;
        let hits = ui
            .layout(
                &view_of(Screen::Party, Some(party(members, true, true))),
                VIEWPORT,
                scale(),
            )
            .hits;
        assert!(hits.iter().any(|(hit, _)| *hit == Hit::Start));
        assert!(
            hits.iter()
                .any(|(hit, _)| *hit == Hit::Kick("other".into()))
        );
        assert!(!hits.iter().any(|(hit, _)| *hit == Hit::Kick("me".into())));
    }

    #[test]
    fn start_needs_the_key_owner_and_withdraw_is_the_owners_alone() {
        let ui = FrontEndUi::new();
        let members = vec![
            member(true, true, false, Some(2)),
            member(false, false, true, Some(3)),
        ];
        // A leader who does not own the key (the server never says so, but the screen must
        // not offer what the server would refuse).
        let view = view_of(Screen::Party, Some(party(members.clone(), true, false)));
        assert!(rect_of(&ui, &view, &Hit::Start).is_none());
        assert!(rect_of(&ui, &view, &Hit::WithdrawKey).is_none());
        // The owner who leads gets both.
        let view = view_of(Screen::Party, Some(party(members.clone(), true, true)));
        assert!(rect_of(&ui, &view, &Hit::Start).is_some());
        assert!(rect_of(&ui, &view, &Hit::WithdrawKey).is_some());
        // Everyone can leave.
        assert!(rect_of(&ui, &view, &Hit::LeaveParty).is_some());
        // Nobody is offered Make Leader: the server refuses it before the start.
        let mut ui = FrontEndUi::new();
        assert_eq!(
            click(&mut ui, &view, &Hit::Start),
            vec![FrontEndAction::StartDungeon]
        );
        assert_eq!(
            click(&mut ui, &view, &Hit::WithdrawKey),
            vec![FrontEndAction::WithdrawKey]
        );
        // Once started, the screen is busy and offers neither.
        let mut busy = view;
        busy.busy = Some("Starting dungeon...".into());
        assert!(rect_of(&ui, &busy, &Hit::Start).is_none());
        assert!(rect_of(&ui, &busy, &Hit::WithdrawKey).is_none());
        // A member who is not connected holds the start back.
        let mut away = view_of(Screen::Party, Some(party(members, true, true)));
        if let Some(card) = away.party.as_mut() {
            card.members[1].present = false;
        }
        assert!(rect_of(&ui, &away, &Hit::Start).is_none());
    }

    #[test]
    fn insert_key_is_clickable_only_with_a_valid_key() {
        let mut ui = FrontEndUi::new();
        let with_key = view_of(
            Screen::KeySelect,
            Some(party(
                vec![
                    member(true, false, false, Some(2)),
                    member(false, false, false, None),
                ],
                false,
                false,
            )),
        );
        assert_eq!(
            click(&mut ui, &with_key, &Hit::InsertKey),
            vec![FrontEndAction::InsertKey]
        );
        let without = view_of(
            Screen::KeySelect,
            Some(party(
                vec![
                    member(true, false, false, None),
                    member(false, false, false, Some(3)),
                ],
                false,
                false,
            )),
        );
        assert!(rect_of(&ui, &without, &Hit::InsertKey).is_none());
        assert_eq!(
            click(&mut ui, &without, &Hit::LeaveParty),
            vec![FrontEndAction::LeaveParty]
        );
        // The disabled button still draws, with its reason.
        let mut list = UiDrawList::new();
        ui.draw(&mut list, &without, VIEWPORT, scale());
        assert!(!list.is_empty());
        // After pressing it the screen waits for the server rather than inviting a second try.
        let mut waiting = with_key;
        waiting.busy = Some("Inserting your key...".into());
        assert!(rect_of(&ui, &waiting, &Hit::InsertKey).is_none());
    }

    #[test]
    fn the_hub_buttons_are_their_actions() {
        let mut ui = FrontEndUi::new();
        let view = hub_view();
        for (hit, action) in [
            (Hit::DungeonFinder, FrontEndAction::OpenDungeonFinder),
            (Hit::Equipment, FrontEndAction::OpenEquipment),
            (Hit::Workshop, FrontEndAction::OpenWorkshop),
            (Hit::ChangeCharacter, FrontEndAction::ChangeCharacter),
            (Hit::Logout, FrontEndAction::Logout),
        ] {
            assert_eq!(click(&mut ui, &view, &hit), vec![action]);
        }
    }

    #[test]
    fn a_queued_hub_offers_leave_queue_and_no_second_search_or_character_change() {
        let mut ui = FrontEndUi::new();
        let mut view = hub_view();
        view.queue = Some(queue_card(false));
        if let Some(hub) = view.hub.as_mut() {
            hub.can_find_dungeon = false;
            hub.can_change_character = false;
        }
        assert!(rect_of(&ui, &view, &Hit::DungeonFinder).is_none());
        assert!(rect_of(&ui, &view, &Hit::ChangeCharacter).is_none());
        assert_eq!(
            click(&mut ui, &view, &Hit::LeaveQueue),
            vec![FrontEndAction::LeaveQueue]
        );
        // Once a group is found the ticket can no longer be withdrawn from here.
        view.queue = Some(queue_card(true));
        assert!(rect_of(&ui, &view, &Hit::LeaveQueue).is_none());
        let mut list = UiDrawList::new();
        ui.draw(&mut list, &view, VIEWPORT, scale());
        assert!(!list.is_empty());
    }

    #[test]
    fn the_hub_takes_the_pointer_only_over_its_own_panels() {
        let mut ui = FrontEndUi::new();
        let mut view = hub_view();
        view.queue = Some(queue_card(false));
        let none = || None;
        let over = |ui: &mut FrontEndUi, at: (f32, f32)| {
            ui.update(
                &input(at, false, &[], &none),
                &view,
                VIEWPORT,
                scale(),
                0.016,
            );
            ui.wants_pointer(&view, VIEWPORT, scale())
        };
        let panel = rect_of(&ui, &view, &Hit::Panel).expect("a panel blocks the pointer");
        assert!(over(&mut ui, center(panel)));
        // The middle of the screen is the world's: mouse-look must keep working there.
        assert!(!over(&mut ui, (960.0, 540.0)));
        // A full-screen page takes it everywhere.
        let page = view_of(Screen::Party, None);
        ui.update(
            &input((960.0, 540.0), false, &[], &none),
            &page,
            VIEWPORT,
            scale(),
            0.016,
        );
        assert!(ui.wants_pointer(&page, VIEWPORT, scale()));
    }

    #[test]
    fn return_to_hub_is_clickable_until_it_is_pressed() {
        let mut ui = FrontEndUi::new();
        let mut view = view_of(Screen::InWorld, None);
        view.completion = Some(completion(true, true, false));
        assert_eq!(
            click(&mut ui, &view, &Hit::ReturnToHub),
            vec![FrontEndAction::ReturnToHub]
        );
        // The in-world Leave is the early exit; the panel replaces it once the run is over.
        assert!(rect_of(&ui, &view, &Hit::LeaveParty).is_none());
        view.completion = Some(completion(true, true, true));
        assert!(rect_of(&ui, &view, &Hit::ReturnToHub).is_none());
        view.completion = None;
        assert_eq!(
            click(&mut ui, &view, &Hit::LeaveParty),
            vec![FrontEndAction::LeaveParty]
        );
    }

    #[test]
    fn the_completion_panel_says_what_the_run_gave() {
        let rows = |card: &CompletionCard| -> Vec<String> {
            completion_rows(card)
                .into_iter()
                .map(|(label, value, _)| format!("{label}: {value}"))
                .collect()
        };
        let won = rows(&completion(true, true, false));
        assert!(won.contains(&"Key upgraded: 3 -> 4".to_owned()));
        assert!(won.contains(&"Time: 12:34".to_owned()));
        assert!(won.contains(&"Experience: +350".to_owned()));
        assert!(won.contains(&"Crypt Dust: +11".to_owned()));
        assert!(won.contains(&"Loot: Open the chest to claim your loot".to_owned()));
        let lost = rows(&completion(false, true, false));
        assert!(lost.contains(&"Key: Your key is unchanged".to_owned()));
        assert!(!lost.iter().any(|row| row.starts_with("Experience")));
        // Only the key's owner hears about the key.
        let guest = rows(&completion(true, false, false));
        assert!(!guest.iter().any(|row| row.starts_with("Key upgraded")));
    }

    fn rusty_sword(index: u32) -> BagRow {
        BagRow {
            index,
            name: "RUSTY SWORD".into(),
            quantity: 1,
            item_level: 6,
            icon: ItemIcon::Helm,
            quality: ItemQuality::Common,
            yields: "+8 Crypt Dust".into(),
        }
    }

    fn workshop_view(tray: Vec<u32>) -> FrontEndView {
        let mut view = view_of(Screen::Workshop, None);
        view.workshop = Some(WorkshopCard {
            bag: vec![rusty_sword(4), rusty_sword(11)],
            tray_yield: if tray.is_empty() {
                String::new()
            } else {
                "+16 Crypt Dust".into()
            },
            tray,
            tray_slots: 16,
            resources: vec![("Crypt Dust".into(), 12)],
            exchanges: vec![
                ExchangeRow {
                    id: "dust_to_shard".into(),
                    label: "10 Crypt Dust for 1 Ember Shard".into(),
                    affordable: true,
                },
                ExchangeRow {
                    id: "shard_to_dust".into(),
                    label: "1 Ember Shard for 5 Crypt Dust".into(),
                    affordable: false,
                },
            ],
        });
        view
    }

    #[test]
    fn the_workshop_stages_and_unstages_before_it_salvages() {
        let mut ui = FrontEndUi::new();
        let view = workshop_view(vec![]);
        assert_eq!(
            click(&mut ui, &view, &Hit::BagCell(4)),
            vec![FrontEndAction::StageSalvage(4)]
        );
        // An empty tray has nothing to salvage.
        assert!(rect_of(&ui, &view, &Hit::SalvageTray).is_none());
        assert!(rect_of(&ui, &view, &Hit::TrayCell(0)).is_none());

        let mut view = workshop_view(vec![11, 4]);
        assert_eq!(
            click(&mut ui, &view, &Hit::TrayCell(0)),
            vec![FrontEndAction::UnstageSalvage(11)]
        );
        assert_eq!(
            click(&mut ui, &view, &Hit::SalvageTray),
            vec![FrontEndAction::SalvageTray]
        );
        // What is in the tray cannot be put there twice.
        assert!(rect_of(&ui, &view, &Hit::BagCell(4)).is_none());
        assert!(rect_of(&ui, &view, &Hit::BagCell(11)).is_none());
        assert_eq!(
            click(&mut ui, &view, &Hit::Exchange("dust_to_shard".into())),
            vec![FrontEndAction::Exchange("dust_to_shard".into())]
        );
        assert!(rect_of(&ui, &view, &Hit::Exchange("shard_to_dust".into())).is_none());
        assert_eq!(
            click(&mut ui, &view, &Hit::Back),
            vec![FrontEndAction::Back]
        );
        // While a request is in flight nothing else can be sent.
        view.busy = Some("Salvaging...".into());
        for hit in [Hit::SalvageTray, Hit::TrayCell(0), Hit::BagCell(4)] {
            assert!(rect_of(&ui, &view, &hit).is_none());
        }
        let mut list = UiDrawList::new();
        ui.draw(&mut list, &view, VIEWPORT, scale());
        assert!(!list.is_empty());
    }

    #[test]
    fn the_tray_is_two_rows_of_bag_sized_cells_inside_the_smallest_window() {
        use super::super::character_menu::BAG_CELL;
        let small = (1280, 720);
        let ui = FrontEndUi::new();
        let view = workshop_view((0..16).map(|i| i + 100).collect());
        let panels = workshop_panels(1280.0, 720.0, scale());
        let tray: Vec<UiRect> = (0..16)
            .map(|position| tray_cell_rect(panels.salvage, position, scale()))
            .collect();
        assert_eq!(tray.len(), 16);
        for cell in &tray {
            assert!((cell.width - BAG_CELL).abs() < 1e-3);
            assert!((cell.height - BAG_CELL).abs() < 1e-3);
        }
        // Eight to a row, two rows, aligned like the bag grid.
        assert_eq!(tray.iter().filter(|c| c.y == tray[0].y).count(), 8);
        assert_eq!(tray.iter().filter(|c| c.y == tray[8].y).count(), 8);
        assert!(tray[8].y > tray[0].y);
        let bag_cell = bag_cell_rect(panels.bag, 0, scale());
        assert_eq!((bag_cell.width, bag_cell.height), (BAG_CELL, BAG_CELL));
        assert_eq!(
            tray[1].x - tray[0].x,
            bag_cell_rect(panels.bag, 1, scale()).x - bag_cell.x
        );
        // Everything laid out and every panel stays on screen and does not overlap.
        let layout = ui.layout(&view, small, scale());
        for (_, rect) in &layout.hits {
            assert!(rect.x >= 0.0 && rect.y >= 0.0, "{rect:?}");
            assert!(rect.right() <= 1280.0 && rect.bottom() <= 720.0, "{rect:?}");
        }
        for panel in [panels.bag, panels.salvage, panels.exchange] {
            assert!(panel.right() <= 1280.0 && panel.bottom() <= 720.0);
        }
        assert!(panels.salvage.bottom() <= panels.exchange.y);
        let last_bag = bag_cell_rect(panels.bag, INVENTORY_SLOTS - 1, scale());
        assert!(last_bag.bottom() <= panels.bag.bottom());
        let last_tray = tray[15];
        assert!(last_tray.bottom() <= salvage_button(panels.salvage, scale()).y);
        let (back, _) = footer_buttons(1280.0, 720.0, scale());
        assert!(panels.exchange.bottom() <= back.y);
        let mut list = UiDrawList::new();
        FrontEndUi::new().draw(&mut list, &view, small, scale());
        assert!(!list.is_empty());
    }

    #[test]
    fn roles_the_character_cannot_play_are_not_clickable() {
        let ui = FrontEndUi::new();
        let view = FrontEndView {
            screen: Screen::Roles,
            characters: vec![CharacterCard {
                name: "W".into(),
                class: "Warrior".into(),
                roles: vec![Role::Tank],
                level: 1,
                experience: 0,
                next_level: 400,
                gear_score: 40,
                equipped: 4,
                key: None,
                resources: 0,
            }],
            selected_character: Some(0),
            ..FrontEndView::default()
        };
        let hits = ui.layout(&view, VIEWPORT, scale()).hits;
        assert!(hits.iter().any(|(hit, _)| *hit == Hit::Role(Role::Tank)));
        assert!(!hits.iter().any(|(hit, _)| *hit == Hit::Role(Role::Healer)));
    }

    #[test]
    fn every_screen_draws() {
        let ui = FrontEndUi::new();
        for screen in [
            Screen::Title,
            Screen::Login,
            Screen::Characters,
            Screen::Hub,
            Screen::Dungeons,
            Screen::Roles,
            Screen::Workshop,
            Screen::ReadyCheck,
            Screen::KeySelect,
            Screen::Party,
            Screen::InWorld,
        ] {
            // Empty, and again with everything a screen can show.
            let full = FrontEndView {
                screen,
                account: Some("Sonit".into()),
                busy: Some("Working...".into()),
                error: Some("Something was refused.".into()),
                characters: vec![CharacterCard {
                    name: "Ember".into(),
                    class: "Mage".into(),
                    roles: vec![Role::Dps],
                    level: 4,
                    experience: 10,
                    next_level: 400,
                    gear_score: 40,
                    equipped: 4,
                    key: Some(KeyCard {
                        dungeon: "Graveyard Crypt".into(),
                        level: 2,
                    }),
                    resources: 12,
                }],
                selected_character: Some(0),
                hub: hub_view().hub,
                queue: Some(queue_card(false)),
                workshop: Some(WorkshopCard::default()),
                party: Some(party(
                    vec![
                        member(true, true, true, Some(2)),
                        member(false, false, false, None),
                    ],
                    true,
                    true,
                )),
                completion: Some(completion(true, true, false)),
                ..FrontEndView::default()
            };
            for view in [
                FrontEndView {
                    screen,
                    reconnecting: Some("Reconnecting".into()),
                    countdown: Some(CountdownDisplay::Seconds(3)),
                    ..FrontEndView::default()
                },
                full,
            ] {
                let mut list = UiDrawList::new();
                ui.draw(&mut list, &view, VIEWPORT, scale());
                assert!(!list.is_empty(), "{screen:?} drew nothing");
            }
        }
    }
}
