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
//! `screen`, chosen by the app from what the *server* says (a ticket exists → Queue, a party in
//! its ready check → ReadyCheck ...), so a reconnect lands on the right screen by construction.
//! The only state kept here is the interface's own: what is typed in the login fields, which
//! field has focus, whether the sign-in form is in "create account" mode.
//!
//! Layout is a pure function of the view, the viewport and the UI scale; `update` and `draw`
//! both call it, so what is clicked is always what was drawn.

pub mod text_field;

use engine_realm::Role;
use engine_realm::instance::CountdownDisplay;
use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, TextStyle, UiColor, UiDrawList, UiRect, UiScale};

pub use text_field::{FieldKeys, FieldOutcome, TextField};

/// Which screen is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// Sign in or create an account.
    #[default]
    Login,
    /// Choose a character.
    Characters,
    /// Choose a dungeon.
    Dungeons,
    /// Choose a role.
    Roles,
    /// Searching for a group.
    Queue,
    /// "Dungeon found": accept or decline.
    ReadyCheck,
    /// The party, waiting for its leader.
    Party,
    /// In the dungeon: no full-screen interface, only overlays.
    InWorld,
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
    /// Role.
    pub role: Role,
    /// Level.
    pub level: u32,
    /// Gear score.
    pub gear_score: u32,
    /// Ready-check answer.
    pub ready: ReadyState,
    /// Connected.
    pub present: bool,
    /// Leads the party.
    pub leader: bool,
    /// Is the local player.
    pub me: bool,
}

/// The local player's party.
#[derive(Debug, Clone, PartialEq)]
pub struct PartyCard {
    /// Dungeon name.
    pub dungeon: String,
    /// Members, in party order.
    pub members: Vec<MemberRow>,
    /// Whether the local player leads.
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
    /// Sign out.
    Logout,
    /// Leave the game.
    Quit,
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
    /// Start the dungeon (leader).
    StartDungeon,
    /// Remove a member (leader).
    Kick(String),
    /// Hand over leadership (leader).
    MakeLeader(String),
    /// Leave the party.
    LeaveParty,
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
    Dungeon(usize),
    Role(Role),
    LeaveQueue,
    Accept,
    Decline,
    Start,
    LeaveParty,
    Kick(String),
    MakeLeader(String),
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
        Self { hits: Vec::with_capacity(32) }
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
        view.screen != Screen::InWorld
            || self.layout(view, viewport, scale).at(self.pointer).is_some()
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

        if input.escape {
            match view.screen {
                Screen::Dungeons | Screen::Roles => actions.push(FrontEndAction::Back),
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
                Screen::Characters => FrontEndAction::ConfirmCharacter,
                Screen::Dungeons => FrontEndAction::ConfirmDungeon,
                _ => FrontEndAction::JoinQueue,
            }),
            Hit::Back => actions.push(FrontEndAction::Back),
            Hit::Dungeon(index) => actions.push(FrontEndAction::SelectDungeon(index)),
            Hit::Role(role) => actions.push(FrontEndAction::SelectRole(role)),
            Hit::LeaveQueue => actions.push(FrontEndAction::LeaveQueue),
            Hit::Accept => actions.push(FrontEndAction::Accept),
            Hit::Decline => actions.push(FrontEndAction::Decline),
            Hit::Start => actions.push(FrontEndAction::StartDungeon),
            Hit::LeaveParty => actions.push(FrontEndAction::LeaveParty),
            Hit::Kick(account) => actions.push(FrontEndAction::Kick(account)),
            Hit::MakeLeader(account) => actions.push(FrontEndAction::MakeLeader(account)),
        }
        actions
    }

    // --- layout -----------------------------------------------------------------------

    fn layout(&self, view: &FrontEndView, viewport: (u32, u32), scale: UiScale) -> Layout {
        let mut layout = Layout::new();
        #[allow(clippy::cast_precision_loss, reason = "viewport sizes are small integers")]
        let (w, h) = (viewport.0 as f32, viewport.1 as f32);
        let px = |v: f32| scale.px(v);

        // Top bar: account and sign-out, on every signed-in screen but the world.
        if view.account.is_some() && !matches!(view.screen, Screen::Login | Screen::InWorld) {
            layout.add(Hit::Logout, UiRect::new(w - px(140.0), px(18.0), px(120.0), px(34.0)));
        }

        match view.screen {
            Screen::Login => {
                let panel = login_panel(w, h, scale, self.register);
                let mut y = panel.y + px(118.0);
                layout.add(Hit::Field(FieldId::Email), UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(44.0)));
                y += px(76.0);
                if self.register {
                    layout.add(Hit::Field(FieldId::DisplayName), UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(44.0)));
                    y += px(76.0);
                }
                layout.add(Hit::Field(FieldId::Password), UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(44.0)));
                y += px(60.0);
                layout.add(Hit::Remember, UiRect::new(panel.x + px(36.0), y, px(200.0), px(28.0)));
                y += px(44.0);
                layout.add(Hit::Submit, UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(48.0)));
                y += px(60.0);
                layout.add(Hit::ToggleRegister, UiRect::new(panel.x + px(36.0), y, panel.width - px(72.0), px(36.0)));
                layout.add(Hit::Quit, UiRect::new(px(20.0), h - px(54.0), px(110.0), px(34.0)));
            }
            Screen::Characters => {
                for (index, rect) in character_cards(view.characters.len(), w, h, scale).into_iter().enumerate() {
                    layout.add(Hit::Character(index), rect);
                }
                let (back, next) = footer_buttons(w, h, scale);
                let _ = back;
                if view.selected_character.is_some() {
                    layout.add(Hit::Continue, next);
                }
            }
            Screen::Dungeons => {
                let (list, _) = dungeon_panels(w, h, scale);
                for index in 0..view.dungeons.len() {
                    let row = UiRect::new(list.x + px(12.0), list.y + px(64.0) + px(74.0) * index as f32, list.width - px(24.0), px(64.0));
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
                if view.selected_role.is_some() {
                    layout.add(Hit::Continue, next);
                }
            }
            Screen::Queue => {
                let panel = centered(w, h, px(520.0), px(330.0));
                layout.add(Hit::LeaveQueue, UiRect::new(panel.x + panel.width * 0.5 - px(110.0), panel.bottom() - px(70.0), px(220.0), px(44.0)));
            }
            Screen::ReadyCheck => {
                let panel = centered(w, h, px(560.0), px(400.0));
                let answered = view.party.as_ref().is_some_and(|p| p.i_answered);
                if !answered {
                    layout.add(Hit::Accept, UiRect::new(panel.x + px(40.0), panel.bottom() - px(76.0), px(220.0), px(48.0)));
                    layout.add(Hit::Decline, UiRect::new(panel.right() - px(260.0), panel.bottom() - px(76.0), px(220.0), px(48.0)));
                }
            }
            Screen::Party => {
                let panel = centered(w, h, px(760.0), px(470.0));
                if let Some(party) = &view.party {
                    for (index, member) in party.members.iter().enumerate() {
                        let row = member_row(panel, index, scale);
                        if party.i_lead && !member.me {
                            layout.add(Hit::MakeLeader(member.account_id.clone()), UiRect::new(row.right() - px(236.0), row.y + px(14.0), px(120.0), px(34.0)));
                            layout.add(Hit::Kick(member.account_id.clone()), UiRect::new(row.right() - px(106.0), row.y + px(14.0), px(90.0), px(34.0)));
                        }
                    }
                    if party.i_lead {
                        layout.add(Hit::Start, UiRect::new(panel.right() - px(260.0), panel.bottom() - px(72.0), px(220.0), px(48.0)));
                    }
                }
                layout.add(Hit::LeaveParty, UiRect::new(panel.x + px(40.0), panel.bottom() - px(72.0), px(180.0), px(48.0)));
            }
            Screen::InWorld => {
                // A small "leave" control while in the dungeon.
                layout.add(Hit::LeaveParty, UiRect::new(w - px(160.0), px(18.0), px(140.0), px(32.0)));
            }
        }
        layout
    }

    // --- drawing ----------------------------------------------------------------------

    /// Draws the front-end for `view`.
    pub fn draw(&self, list: &mut UiDrawList, view: &FrontEndView, viewport: (u32, u32), scale: UiScale) {
        #[allow(clippy::cast_precision_loss, reason = "viewport sizes are small integers")]
        let (w, h) = (viewport.0 as f32, viewport.1 as f32);
        let layout = self.layout(view, viewport, scale);
        let hovered = layout.at(self.pointer).cloned();
        let ctx = Painter { list, scale, hovered: hovered.as_ref() };
        let mut ctx = ctx;

        if view.screen != Screen::InWorld {
            ctx.list.panel(UiRect::new(0.0, 0.0, w, h), 0.0, BACKDROP_TOP, BACKDROP_BOTTOM);
            ctx.top_bar(view, w);
        }
        match view.screen {
            Screen::Login => self.draw_login(&mut ctx, view, w, h),
            Screen::Characters => draw_characters(&mut ctx, view, w, h),
            Screen::Dungeons => draw_dungeons(&mut ctx, view, w, h),
            Screen::Roles => draw_roles(&mut ctx, view, w, h),
            Screen::Queue => draw_queue(&mut ctx, view, w, h, self.clock),
            Screen::ReadyCheck => draw_ready_check(&mut ctx, view, w, h),
            Screen::Party => draw_party(&mut ctx, view, w, h),
            Screen::InWorld => draw_in_world(&mut ctx, view, w, h),
        }
        if let Some(error) = &view.error
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
        ctx.text_styled((w * 0.5, h * 0.12), "SHADOWS OF THE CRYPT", GOLD, UiFont::Display, 46.0, TextAlign::Center);
        ctx.text((w * 0.5, h * 0.12 + px(60.0)), "Online", DIM, UiFont::Medium, 18.0, TextAlign::Center);

        let panel = login_panel(w, h, ctx.scale, self.register);
        ctx.panel(panel);
        let title = if self.register { "CREATE ACCOUNT" } else { "SIGN IN" };
        ctx.text((panel.x + panel.width * 0.5, panel.y + px(34.0)), title, GOLD, UiFont::Display, 28.0, TextAlign::Center);

        let mut y = panel.y + px(118.0);
        let x = panel.x + px(36.0);
        let width = panel.width - px(72.0);
        ctx.text((x, y - px(26.0)), "Email", DIM, UiFont::Medium, 15.0, TextAlign::Left);
        self.draw_field(ctx, FieldId::Email, UiRect::new(x, y, width, px(44.0)), &self.email);
        y += px(76.0);
        if self.register {
            ctx.text((x, y - px(26.0)), "Display name", DIM, UiFont::Medium, 15.0, TextAlign::Left);
            self.draw_field(ctx, FieldId::DisplayName, UiRect::new(x, y, width, px(44.0)), &self.display_name);
            y += px(76.0);
        }
        ctx.text((x, y - px(26.0)), "Password", DIM, UiFont::Medium, 15.0, TextAlign::Left);
        self.draw_field(ctx, FieldId::Password, UiRect::new(x, y, width, px(44.0)), &self.password);
        y += px(60.0);
        // Remember me.
        let tick = UiRect::new(x, y + px(3.0), px(22.0), px(22.0));
        ctx.list.panel_stroke(tick, px(4.0), px(2.0), GOLD_EDGE);
        if self.remember {
            ctx.list.panel(tick.inset(px(5.0)), px(2.0), GOLD, GOLD);
        }
        ctx.text((tick.right() + px(10.0), y + px(4.0)), "Remember me on this computer", TEXT, UiFont::Regular, 16.0, TextAlign::Left);
        y += px(44.0);
        let busy = view.busy.is_some();
        let label = if busy {
            view.busy.as_deref().unwrap_or("Working...")
        } else if self.register {
            "Create Account"
        } else {
            "Sign In"
        };
        let ready = !busy && !self.email.text().trim().is_empty() && !self.password.text().is_empty();
        ctx.button(UiRect::new(x, y, width, px(48.0)), label, ButtonKind::Primary, &Hit::Submit, ready);
        y += px(60.0);
        let toggle = if self.register { "I already have an account" } else { "Create a new account" };
        ctx.link(UiRect::new(x, y, width, px(36.0)), toggle, &Hit::ToggleRegister);
        y += px(50.0);
        if let Some(error) = &view.error {
            for (line_index, line) in UiFont::Medium.wrap(error, ctx.scale.px(15.0), width).iter().enumerate() {
                ctx.text((panel.x + panel.width * 0.5, y + px(20.0) * line_index as f32), line, BAD, UiFont::Medium, 15.0, TextAlign::Center);
            }
        }
        ctx.button(UiRect::new(px(20.0), h - px(54.0), px(110.0), px(34.0)), "Quit", ButtonKind::Secondary, &Hit::Quit, true);
    }

    fn draw_field(&self, ctx: &mut Painter<'_>, id: FieldId, rect: UiRect, field: &TextField) {
        let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
        let focused = self.focus == Some(id);
        ctx.list.panel(rect, px(6.0), FIELD, FIELD);
        ctx.list.panel_stroke(rect, px(6.0), px(if focused { 2.0 } else { 1.0 }), if focused { GOLD } else { [0.30, 0.32, 0.38, 1.0] });
        let size = 18.0;
        let baseline_top = rect.y + (rect.height - UiFont::Regular.line_height(px(size))) * 0.5;
        let shown = field.display();
        ctx.list.label((rect.x + px(14.0), baseline_top), &shown, TEXT, UiFont::Regular, px(size), TextAlign::Left);
        if focused && (self.clock * 2.0).fract() < 0.6 {
            let caret_x = rect.x + px(14.0) + UiFont::Regular.width(&field.display_before_caret(), px(size));
            ctx.list.rect(UiRect::new(caret_x, rect.y + px(10.0), px(2.0), rect.height - px(20.0)), GOLD);
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
    fn text(&mut self, origin: (f32, f32), text: &str, color: UiColor, font: UiFont, size: f32, align: TextAlign) {
        self.list.label(origin, text, color, font, self.scale.px(size), align);
    }

    fn text_styled(&mut self, origin: (f32, f32), text: &str, color: UiColor, font: UiFont, size: f32, align: TextAlign) {
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
        self.list.shadow(rect, 12.0 * px, 24.0 * px, [0.0, 0.0, 0.0, 0.55]);
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
                    ([0.72, 0.52, 0.18, 1.0], [0.50, 0.33, 0.09, 1.0], [1.0, 0.97, 0.88, 1.0])
                } else {
                    ([0.58, 0.40, 0.13, 1.0], [0.40, 0.26, 0.07, 1.0], [1.0, 0.95, 0.84, 1.0])
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
            self.list.glow(rect, 8.0 * px, 10.0 * px, [0.96, 0.78, 0.40, 0.18]);
        }
        self.list.panel(rect, 8.0 * px, top, bottom);
        self.list.panel_stroke(rect, 8.0 * px, 1.0 * px, [1.0, 0.9, 0.7, if enabled { 0.25 } else { 0.08 }]);
        let size = 17.0;
        let top_y = rect.y + (rect.height - UiFont::SemiBold.line_height(self.scale.px(size))) * 0.5;
        self.text((rect.x + rect.width * 0.5, top_y), label, text, UiFont::SemiBold, size, TextAlign::Center);
    }

    fn link(&mut self, rect: UiRect, label: &str, hit: &Hit) {
        let color = if self.is_hovered(hit) { GOLD } else { DIM };
        let top_y = rect.y + (rect.height - UiFont::Medium.line_height(self.scale.px(16.0))) * 0.5;
        self.text((rect.x + rect.width * 0.5, top_y), label, color, UiFont::Medium, 16.0, TextAlign::Center);
    }

    fn top_bar(&mut self, view: &FrontEndView, w: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        if let Some((level, text)) = &view.connection {
            let dot = (px(28.0), px(35.0));
            self.list.disc(dot, px(6.0), status_color(*level));
            self.text((px(44.0), px(25.0)), text, DIM, UiFont::Medium, 15.0, TextAlign::Left);
        }
        if let Some(account) = &view.account
            && !matches!(view.screen, Screen::Login | Screen::InWorld)
        {
            self.text((w - px(160.0), px(25.0)), account, TEXT, UiFont::SemiBold, 16.0, TextAlign::Right);
            self.button(UiRect::new(w - px(140.0), px(18.0), px(120.0), px(34.0)), "Log Out", ButtonKind::Secondary, &Hit::Logout, true);
        }
    }

    fn heading(&mut self, title: &str, subtitle: &str, w: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        self.text_styled((w * 0.5, px(70.0)), title, GOLD, UiFont::Display, 36.0, TextAlign::Center);
        self.text((w * 0.5, px(122.0)), subtitle, DIM, UiFont::Regular, 17.0, TextAlign::Center);
    }

    fn banner(&mut self, text: &str, level: StatusLevel, w: f32, y: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        let width = UiFont::Medium.width(text, px(16.0)) + px(48.0);
        let rect = UiRect::new((w - width) * 0.5, y, width, px(40.0));
        self.list.panel(rect, px(8.0), [0.08, 0.06, 0.07, 0.95], [0.05, 0.04, 0.05, 0.95]);
        self.list.panel_stroke(rect, px(8.0), px(1.5), status_color(level));
        let top_y = rect.y + (rect.height - UiFont::Medium.line_height(px(16.0))) * 0.5;
        self.text((w * 0.5, top_y), text, TEXT, UiFont::Medium, 16.0, TextAlign::Center);
    }

    fn reconnecting(&mut self, text: &str, w: f32, h: f32, clock: f32) {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        self.list.rect(UiRect::new(0.0, 0.0, w, h), [0.0, 0.0, 0.0, 0.55]);
        let panel = centered(w, h, px(520.0), px(170.0));
        self.panel(panel);
        self.text((w * 0.5, panel.y + px(28.0)), "Connection lost", BAD, UiFont::Display, 28.0, TextAlign::Center);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a small animation phase")]
        let dots = ".".repeat(((clock * 2.0) as usize % 3) + 1);
        self.text((w * 0.5, panel.y + px(84.0)), &format!("{text}{dots}"), TEXT, UiFont::Medium, 18.0, TextAlign::Center);
        self.text((w * 0.5, panel.y + px(118.0)), "Your character is safe. The party is waiting for you.", DIM, UiFont::Regular, 15.0, TextAlign::Center);
    }

    fn role_chip(&mut self, role: Role, x: f32, y: f32) -> f32 {
        let scale = self.scale;
        let px = move |v: f32| scale.px(v);
        let width = UiFont::SemiBold.width(role.name(), px(13.0)) + px(18.0);
        let rect = UiRect::new(x, y, width, px(22.0));
        let color = role_color(role);
        self.list.panel(rect, px(11.0), [color[0] * 0.35, color[1] * 0.35, color[2] * 0.35, 0.95], [color[0] * 0.22, color[1] * 0.22, color[2] * 0.22, 0.95]);
        self.list.panel_stroke(rect, px(11.0), px(1.0), color);
        self.text((rect.x + width * 0.5, rect.y + px(2.0)), role.name(), color, UiFont::SemiBold, 13.0, TextAlign::Center);
        width
    }
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
    let card = (px(280.0), px(380.0));
    let gap = px(28.0);
    let per_row = (((w - px(80.0)) + gap) / (card.0 + gap)).floor().max(1.0) as usize;
    let rows = count.div_ceil(per_row).max(1);
    let total_height = rows as f32 * card.1 + (rows as f32 - 1.0) * gap;
    let top = ((h - total_height) * 0.5).max(px(170.0));
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
        UiRect::new(left + total * 0.36 + px(24.0), top, total * 0.64 - px(24.0), height),
    )
}

fn role_tiles(w: f32, h: f32, scale: UiScale) -> Vec<UiRect> {
    let px = |v: f32| scale.px(v);
    let tile = (px(300.0), px(300.0));
    let gap = px(36.0);
    let total = tile.0 * 3.0 + gap * 2.0;
    let top = (h - tile.1) * 0.5;
    (0..3)
        .map(|i| UiRect::new((w - total) * 0.5 + i as f32 * (tile.0 + gap), top, tile.0, tile.1))
        .collect()
}

fn member_row(panel: UiRect, index: usize, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(panel.x + px(30.0), panel.y + px(100.0) + index as f32 * px(76.0), panel.width - px(60.0), px(64.0))
}

fn draw_characters(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading("CHOOSE YOUR CHARACTER", "Your progress, gear and talents travel with each character.", w);
    if view.characters.is_empty() {
        let text = view.busy.as_deref().unwrap_or("Loading characters...");
        ctx.text((w * 0.5, h * 0.5), text, DIM, UiFont::Medium, 18.0, TextAlign::Center);
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
            ctx.list.glow(rect, px(14.0), px(18.0), [0.96, 0.78, 0.40, 0.30]);
        }
        ctx.list.panel(rect, px(14.0), if hovered { [0.15, 0.16, 0.21, 0.98] } else { PANEL_TOP }, PANEL_BOTTOM);
        ctx.list.panel_stroke(rect, px(14.0), px(if selected { 2.5 } else { 1.0 }), if selected { SELECTED } else { GOLD_EDGE });
        let cx = rect.x + rect.width * 0.5;
        // A crest: the class initial in a ring.
        let crest = (cx, rect.y + px(78.0));
        ctx.list.disc(crest, px(46.0), [0.20, 0.17, 0.12, 1.0]);
        ctx.list.disc(crest, px(42.0), [0.08, 0.08, 0.11, 1.0]);
        let initial: String = card.class.chars().take(1).collect();
        ctx.text_styled((cx, crest.1 - px(26.0)), &initial, GOLD, UiFont::Display, 40.0, TextAlign::Center);
        ctx.text_styled((cx, rect.y + px(140.0)), &card.name, TEXT, UiFont::SemiBold, 24.0, TextAlign::Center);
        ctx.text((cx, rect.y + px(176.0)), &format!("Level {} {}", card.level, card.class), DIM, UiFont::Medium, 16.0, TextAlign::Center);
        // Role chips, centred.
        let widths: f32 = card.roles.iter().map(|r| UiFont::SemiBold.width(r.name(), px(13.0)) + px(18.0)).sum::<f32>()
            + px(8.0) * card.roles.len().saturating_sub(1) as f32;
        let mut x = cx - widths * 0.5;
        for role in &card.roles {
            x += ctx.role_chip(*role, x, rect.y + px(208.0)) + px(8.0);
        }
        // Stats.
        let line = |ctx: &mut Painter<'_>, y: f32, label: &str, value: &str| {
            ctx.text((rect.x + px(28.0), y), label, DIM, UiFont::Medium, 15.0, TextAlign::Left);
            ctx.text((rect.right() - px(28.0), y), value, TEXT, UiFont::SemiBold, 15.0, TextAlign::Right);
        };
        line(ctx, rect.y + px(256.0), "Gear Score", &card.gear_score.to_string());
        line(ctx, rect.y + px(284.0), "Equipment", &format!("{} / 15 slots", card.equipped));
        // Experience bar.
        let bar = UiRect::new(rect.x + px(28.0), rect.y + px(326.0), rect.width - px(56.0), px(10.0));
        #[allow(clippy::cast_precision_loss, reason = "experience values are far below 2^24")]
        let fraction = if card.next_level == 0 { 1.0 } else { card.experience as f32 / card.next_level as f32 };
        ctx.list.panel(bar, px(5.0), [0.05, 0.05, 0.07, 1.0], [0.05, 0.05, 0.07, 1.0]);
        ctx.list.panel(bar.fraction_of_width(fraction.clamp(0.0, 1.0)), px(5.0), [0.55, 0.40, 0.95, 1.0], [0.38, 0.26, 0.75, 1.0]);
        let xp = if card.next_level == 0 { "Max level".to_owned() } else { format!("{} / {} XP", card.experience, card.next_level) };
        ctx.text((cx, rect.y + px(342.0)), &xp, FAINT, UiFont::Medium, 13.0, TextAlign::Center);
    }
    let (_, next) = footer_buttons(w, h, ctx.scale);
    ctx.button(next, "Enter World", ButtonKind::Primary, &Hit::Continue, view.selected_character.is_some());
}

fn draw_dungeons(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading("DUNGEON FINDER", "Choose a dungeon. The realm finds a Tank, a Healer and a DPS of similar gear.", w);
    let (list, detail) = dungeon_panels(w, h, ctx.scale);
    ctx.panel(list);
    ctx.text((list.x + px(20.0), list.y + px(18.0)), "DUNGEONS", GOLD, UiFont::SemiBold, 17.0, TextAlign::Left);
    let character_gs = view.selected_character.and_then(|i| view.characters.get(i)).map(|c| (c.level, c.gear_score));
    for (index, dungeon) in view.dungeons.iter().enumerate() {
        let row = UiRect::new(list.x + px(12.0), list.y + px(64.0) + px(74.0) * index as f32, list.width - px(24.0), px(64.0));
        let selected = view.selected_dungeon == Some(index);
        let hovered = ctx.is_hovered(&Hit::Dungeon(index));
        let fill = if selected { [0.22, 0.18, 0.11, 1.0] } else if hovered { [0.15, 0.16, 0.21, 1.0] } else { [0.09, 0.10, 0.13, 1.0] };
        ctx.list.panel(row, px(8.0), fill, fill);
        if selected {
            ctx.list.panel_stroke(row, px(8.0), px(1.5), SELECTED);
        }
        ctx.text((row.x + px(16.0), row.y + px(10.0)), &dungeon.name, TEXT, UiFont::SemiBold, 18.0, TextAlign::Left);
        let meta = format!("{} | Levels {}-{}", dungeon.difficulty, dungeon.levels.0, dungeon.levels.1);
        ctx.text((row.x + px(16.0), row.y + px(36.0)), &meta, DIM, UiFont::Medium, 14.0, TextAlign::Left);
    }
    ctx.panel(detail);
    if let Some(dungeon) = view.selected_dungeon.and_then(|i| view.dungeons.get(i)) {
        let x = detail.x + px(32.0);
        let width = detail.width - px(64.0);
        let mut y = detail.y + px(26.0);
        ctx.text_styled((x, y), &dungeon.name, GOLD, UiFont::Display, 32.0, TextAlign::Left);
        y += px(54.0);
        ctx.text((x, y), &format!("{} | Levels {}-{}", dungeon.difficulty, dungeon.levels.0, dungeon.levels.1), DIM, UiFont::Medium, 16.0, TextAlign::Left);
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
            ("Recommended gear score", dungeon.recommended_gear_score.to_string(), gs_color),
            ("Group", "1 Tank | 1 Healer | 1 DPS".to_owned(), TEXT),
            (
                "Searching now",
                format!("{} Tank | {} Healer | {} DPS", dungeon.population[0], dungeon.population[1], dungeon.population[2]),
                TEXT,
            ),
        ];
        for (label, value, color) in info {
            ctx.text((x, y), label, DIM, UiFont::Medium, 15.0, TextAlign::Left);
            ctx.text((x + width, y), &value, color, UiFont::SemiBold, 15.0, TextAlign::Right);
            y += px(30.0);
        }
        if let Some((_, gs)) = character_gs {
            ctx.text((x, y), "Your gear score", DIM, UiFont::Medium, 15.0, TextAlign::Left);
            ctx.text((x + width, y), &gs.to_string(), gs_color, UiFont::SemiBold, 15.0, TextAlign::Right);
            y += px(30.0);
        }
        y += px(14.0);
        ctx.text((x, y), "REWARDS", GOLD, UiFont::SemiBold, 15.0, TextAlign::Left);
        y += px(28.0);
        for reward in &dungeon.rewards {
            ctx.list.disc((x + px(6.0), y + px(10.0)), px(3.0), GOLD);
            ctx.text((x + px(18.0), y), reward, TEXT, UiFont::Regular, 15.0, TextAlign::Left);
            y += px(24.0);
        }
    } else {
        ctx.text((detail.x + detail.width * 0.5, detail.y + detail.height * 0.5), "Select a dungeon", DIM, UiFont::Medium, 18.0, TextAlign::Center);
    }
    let (back, next) = footer_buttons(w, h, ctx.scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    ctx.button(next, "Choose Role", ButtonKind::Primary, &Hit::Continue, view.selected_dungeon.is_some());
}

fn draw_roles(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let character = view.selected_character.and_then(|i| view.characters.get(i));
    let dungeon = view.selected_dungeon.and_then(|i| view.dungeons.get(i)).map_or("", |d| d.name.as_str());
    ctx.heading("CHOOSE YOUR ROLE", &format!("{} | {}", character.map_or("", |c| c.name.as_str()), dungeon), w);
    let playable = character.map(|c| c.roles.clone()).unwrap_or_default();
    for (role, rect) in Role::ALL.into_iter().zip(role_tiles(w, h, ctx.scale)) {
        let can = playable.contains(&role);
        let selected = view.selected_role == Some(role);
        let hovered = can && ctx.is_hovered(&Hit::Role(role));
        let color = role_color(role);
        if selected {
            ctx.list.glow(rect, px(16.0), px(20.0), [color[0], color[1], color[2], 0.30]);
        }
        let top = if hovered { [0.16, 0.17, 0.22, 0.98] } else { PANEL_TOP };
        ctx.list.panel(rect, px(16.0), top, PANEL_BOTTOM);
        ctx.list.panel_stroke(rect, px(16.0), px(if selected { 2.5 } else { 1.0 }), if selected { color } else if can { GOLD_EDGE } else { [0.25, 0.25, 0.28, 1.0] });
        let cx = rect.x + rect.width * 0.5;
        let faded = |c: UiColor| if can { c } else { [c[0] * 0.4, c[1] * 0.4, c[2] * 0.4, 1.0] };
        ctx.list.disc((cx, rect.y + px(90.0)), px(48.0), faded([color[0] * 0.3, color[1] * 0.3, color[2] * 0.3, 1.0]));
        let letter = match role { Role::Tank => "T", Role::Healer => "H", Role::Dps => "D" };
        ctx.text_styled((cx, rect.y + px(62.0)), letter, faded(color), UiFont::Display, 44.0, TextAlign::Center);
        ctx.text_styled((cx, rect.y + px(158.0)), role.name(), faded(TEXT), UiFont::SemiBold, 26.0, TextAlign::Center);
        let blurb = if can { role_blurb(role) } else { "Your character cannot fill this role." };
        for (i, line) in UiFont::Regular.wrap(blurb, px(15.0), rect.width - px(48.0)).iter().enumerate() {
            ctx.text((cx, rect.y + px(206.0) + px(22.0) * i as f32), line, if can { DIM } else { FAINT }, UiFont::Regular, 15.0, TextAlign::Center);
        }
    }
    let (back, next) = footer_buttons(w, h, ctx.scale);
    ctx.button(back, "Back", ButtonKind::Secondary, &Hit::Back, true);
    let busy = view.busy.is_some();
    ctx.button(next, if busy { "Joining..." } else { "Find Group" }, ButtonKind::Primary, &Hit::Continue, view.selected_role.is_some() && !busy);
}

fn draw_queue(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32, clock: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let panel = centered(w, h, px(520.0), px(330.0));
    ctx.panel(panel);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "a small animation phase")]
    let dots = ".".repeat(((clock * 2.0) as usize % 3) + 1);
    ctx.text_styled((w * 0.5, panel.y + px(28.0)), &format!("Searching for a group{dots}"), GOLD, UiFont::Display, 26.0, TextAlign::Center);
    // A spinner: three orbiting dots.
    let center = (w * 0.5, panel.y + px(110.0));
    for i in 0..3 {
        let angle = clock * 3.0 + i as f32 * std::f32::consts::TAU / 3.0;
        ctx.list.disc((center.0 + angle.cos() * px(20.0), center.1 + angle.sin() * px(20.0)), px(5.0), GOLD);
    }
    if let Some(queue) = &view.queue {
        let rows = [
            ("Dungeon", queue.dungeon.clone()),
            ("Character", queue.character.clone()),
            ("Role", queue.role.name().to_owned()),
            ("Time in queue", clock_text(queue.waited_seconds)),
            ("Matchmaking range", format!("+/-{}% gear score", queue.tolerance_percent)),
        ];
        let mut y = panel.y + px(146.0);
        for (label, value) in rows {
            ctx.text((panel.x + px(48.0), y), label, DIM, UiFont::Medium, 15.0, TextAlign::Left);
            ctx.text((panel.right() - px(48.0), y), &value, TEXT, UiFont::SemiBold, 15.0, TextAlign::Right);
            y += px(22.0);
        }
    }
    let leave = UiRect::new(panel.x + panel.width * 0.5 - px(110.0), panel.bottom() - px(70.0), px(220.0), px(44.0));
    ctx.button(leave, "Leave Queue", ButtonKind::Secondary, &Hit::LeaveQueue, true);
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
    let panel = centered(w, h, px(560.0), px(400.0));
    ctx.list.glow(panel, px(16.0), px(40.0), [0.96, 0.78, 0.40, 0.20]);
    ctx.panel(panel);
    ctx.text_styled((w * 0.5, panel.y + px(26.0)), "Dungeon Found!", GOLD, UiFont::Display, 34.0, TextAlign::Center);
    let Some(party) = &view.party else { return };
    ctx.text((w * 0.5, panel.y + px(82.0)), &party.dungeon, TEXT, UiFont::SemiBold, 20.0, TextAlign::Center);
    let mut y = panel.y + px(132.0);
    for member in &party.members {
        let x = panel.x + px(48.0);
        ctx.role_chip(member.role, x, y);
        let name = if member.me { format!("{} (you)", member.character) } else { member.character.clone() };
        ctx.text((x + px(90.0), y + px(1.0)), &name, TEXT, UiFont::Medium, 17.0, TextAlign::Left);
        let (label, color) = ready_label(member.ready);
        let label = if member.present { label } else { "Disconnected" };
        let color = if member.present { color } else { BAD };
        ctx.text((panel.right() - px(48.0), y + px(1.0)), label, color, UiFont::SemiBold, 17.0, TextAlign::Right);
        y += px(40.0);
    }
    let bar = UiRect::new(panel.x + px(48.0), panel.bottom() - px(110.0), panel.width - px(96.0), px(8.0));
    ctx.list.panel(bar, px(4.0), [0.05, 0.05, 0.07, 1.0], [0.05, 0.05, 0.07, 1.0]);
    ctx.list.panel(bar.fraction_of_width(party.fraction_left.clamp(0.0, 1.0)), px(4.0), GOLD, [0.7, 0.5, 0.2, 1.0]);
    ctx.text((w * 0.5, bar.y - px(28.0)), &format!("{} s to answer", party.seconds_left), DIM, UiFont::Medium, 14.0, TextAlign::Center);
    if party.i_answered {
        ctx.text((w * 0.5, panel.bottom() - px(64.0)), "Waiting for the others...", DIM, UiFont::Medium, 17.0, TextAlign::Center);
    } else {
        ctx.button(UiRect::new(panel.x + px(40.0), panel.bottom() - px(76.0), px(220.0), px(48.0)), "Accept", ButtonKind::Primary, &Hit::Accept, true);
        ctx.button(UiRect::new(panel.right() - px(260.0), panel.bottom() - px(76.0), px(220.0), px(48.0)), "Decline", ButtonKind::Danger, &Hit::Decline, true);
    }
}

fn draw_party(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let panel = centered(w, h, px(760.0), px(470.0));
    ctx.panel(panel);
    let Some(party) = &view.party else { return };
    ctx.text_styled((panel.x + px(30.0), panel.y + px(24.0)), "Your Party", GOLD, UiFont::Display, 30.0, TextAlign::Left);
    ctx.text((panel.right() - px(30.0), panel.y + px(34.0)), &party.dungeon, DIM, UiFont::SemiBold, 17.0, TextAlign::Right);
    for (index, member) in party.members.iter().enumerate() {
        let row = member_row(panel, index, ctx.scale);
        let fill = if member.me { [0.15, 0.14, 0.11, 1.0] } else { [0.09, 0.10, 0.13, 1.0] };
        ctx.list.panel(row, px(10.0), fill, fill);
        ctx.list.panel_stroke(row, px(10.0), px(1.0), if member.leader { SELECTED } else { [0.25, 0.26, 0.31, 1.0] });
        ctx.list.disc((row.x + px(22.0), row.y + row.height * 0.5), px(6.0), if member.present { GOOD } else { BAD });
        ctx.role_chip(member.role, row.x + px(40.0), row.y + px(10.0));
        let mut name = member.character.clone();
        if member.me {
            name.push_str("  (you)");
        }
        ctx.text((row.x + px(40.0), row.y + px(36.0)), &format!("{} | Level {} | GS {}", member.player, member.level, member.gear_score), DIM, UiFont::Medium, 14.0, TextAlign::Left);
        ctx.text((row.x + px(140.0), row.y + px(9.0)), &name, TEXT, UiFont::SemiBold, 18.0, TextAlign::Left);
        if member.leader {
            let x = row.x + px(150.0) + UiFont::SemiBold.width(&name, px(18.0)) + px(14.0);
            ctx.text((x, row.y + px(12.0)), "LEADER", GOLD, UiFont::SemiBold, 13.0, TextAlign::Left);
        }
        if !member.present {
            ctx.text((row.right() - px(250.0), row.y + px(22.0)), "Disconnected", BAD, UiFont::SemiBold, 15.0, TextAlign::Right);
        }
        if party.i_lead && !member.me {
            ctx.button(UiRect::new(row.right() - px(236.0), row.y + px(14.0), px(120.0), px(34.0)), "Make Leader", ButtonKind::Secondary, &Hit::MakeLeader(member.account_id.clone()), true);
            ctx.button(UiRect::new(row.right() - px(106.0), row.y + px(14.0), px(90.0), px(34.0)), "Kick", ButtonKind::Danger, &Hit::Kick(member.account_id.clone()), true);
        }
    }
    let all_present = party.members.iter().all(|m| m.present);
    if party.i_lead {
        let busy = view.busy.is_some();
        ctx.button(UiRect::new(panel.right() - px(260.0), panel.bottom() - px(72.0), px(220.0), px(48.0)), if busy { "Starting..." } else { "Start Dungeon" }, ButtonKind::Primary, &Hit::Start, all_present && !busy);
    } else {
        ctx.text((panel.right() - px(40.0), panel.bottom() - px(60.0)), "Waiting for the leader to start...", DIM, UiFont::Medium, 16.0, TextAlign::Right);
    }
    ctx.button(UiRect::new(panel.x + px(40.0), panel.bottom() - px(72.0), px(180.0), px(48.0)), "Leave Party", ButtonKind::Secondary, &Hit::LeaveParty, true);
}

fn draw_in_world(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    match view.countdown {
        Some(CountdownDisplay::Seconds(n)) => {
            ctx.text_styled((w * 0.5, h * 0.28), &n.to_string(), GOLD, UiFont::Display, 120.0, TextAlign::Center);
            ctx.text_styled((w * 0.5, h * 0.28 + px(150.0)), "The barrier lifts soon", TEXT, UiFont::SemiBold, 22.0, TextAlign::Center);
        }
        Some(CountdownDisplay::Go) => {
            ctx.text_styled((w * 0.5, h * 0.28), "GO!", GOLD, UiFont::Display, 120.0, TextAlign::Center);
        }
        _ => {}
    }
    ctx.button(UiRect::new(w - px(160.0), px(18.0), px(140.0), px(32.0)), "Leave Party", ButtonKind::Secondary, &Hit::LeaveParty, true);
    if let Some((level, text)) = &view.connection
        && *level != StatusLevel::Good
    {
        ctx.text((w - px(180.0), px(24.0)), text, status_color(*level), UiFont::Medium, 14.0, TextAlign::Right);
    }
}

fn draw_toasts(ctx: &mut Painter<'_>, view: &FrontEndView, w: f32, h: f32) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let mut y = if view.screen == Screen::InWorld { h * 0.18 } else { h - px(180.0) };
    for toast in view.toasts.iter().rev().take(4) {
        ctx.banner(&toast.message, toast.level, w, y);
        y -= px(48.0);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn input<'a>(pointer: (f32, f32), pressed: bool, typed: &'a [char], clipboard: &'a dyn Fn() -> Option<String>) -> FrontInput<'a> {
        FrontInput { pointer, pressed, typed, keys: FieldKeys::default(), escape: false, clipboard }
    }

    fn center(rect: UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn scale() -> UiScale {
        UiScale::new(1.0)
    }

    #[test]
    fn typing_and_enter_signs_in() {
        let mut ui = FrontEndUi::new();
        let view = FrontEndView::default();
        let none = || None;
        let typed: Vec<char> = "me@x.y\tsecret\r".chars().collect();
        // One character at a time, as frames would deliver them.
        let mut actions = Vec::new();
        for c in typed {
            actions.extend(ui.update(&input((0.0, 0.0), false, &[c], &none), &view, VIEWPORT, scale(), 0.016));
        }
        assert_eq!(
            actions,
            vec![FrontEndAction::Login { email: "me@x.y".into(), password: "secret".into(), remember: true }]
        );
    }

    #[test]
    fn clicking_the_drawn_button_is_the_action() {
        let mut ui = FrontEndUi::new();
        let view = FrontEndView {
            screen: Screen::ReadyCheck,
            party: Some(PartyCard {
                dungeon: "Crypt".into(),
                members: vec![],
                i_lead: false,
                i_answered: false,
                seconds_left: 20,
                fraction_left: 0.5,
            }),
            ..FrontEndView::default()
        };
        let layout = ui.layout(&view, VIEWPORT, scale());
        let accept = layout.hits.iter().find(|(hit, _)| *hit == Hit::Accept).map(|(_, r)| *r).expect("accept is laid out");
        let none = || None;
        let actions = ui.update(&input(center(accept), true, &[], &none), &view, VIEWPORT, scale(), 0.016);
        assert_eq!(actions, vec![FrontEndAction::Accept]);
        // Drawing the same view produces commands and does not panic.
        let mut list = UiDrawList::new();
        ui.draw(&mut list, &view, VIEWPORT, scale());
        assert!(!list.is_empty());
    }

    #[test]
    fn only_the_leader_gets_leader_controls() {
        let ui = FrontEndUi::new();
        let member = |me: bool, leader: bool| MemberRow {
            account_id: if me { "me".into() } else { "other".into() },
            character: "C".into(),
            player: "P".into(),
            role: Role::Tank,
            level: 1,
            gear_score: 40,
            ready: ReadyState::Ready,
            present: true,
            leader,
            me,
        };
        let mut party = PartyCard { dungeon: "Crypt".into(), members: vec![member(true, false), member(false, true)], i_lead: false, i_answered: true, seconds_left: 0, fraction_left: 0.0 };
        let view = |party: PartyCard| FrontEndView { screen: Screen::Party, party: Some(party), ..FrontEndView::default() };
        let hits = ui.layout(&view(party.clone()), VIEWPORT, scale()).hits;
        assert!(!hits.iter().any(|(hit, _)| matches!(hit, Hit::Start | Hit::Kick(_) | Hit::MakeLeader(_))));
        party.i_lead = true;
        let hits = ui.layout(&view(party), VIEWPORT, scale()).hits;
        assert!(hits.iter().any(|(hit, _)| *hit == Hit::Start));
        assert!(hits.iter().any(|(hit, _)| *hit == Hit::Kick("other".into())));
        assert!(!hits.iter().any(|(hit, _)| *hit == Hit::Kick("me".into())));
    }

    #[test]
    fn roles_the_character_cannot_play_are_not_clickable() {
        let ui = FrontEndUi::new();
        let view = FrontEndView {
            screen: Screen::Roles,
            characters: vec![CharacterCard { name: "W".into(), class: "Warrior".into(), roles: vec![Role::Tank], level: 1, experience: 0, next_level: 400, gear_score: 40, equipped: 4 }],
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
        for screen in [Screen::Login, Screen::Characters, Screen::Dungeons, Screen::Roles, Screen::Queue, Screen::ReadyCheck, Screen::Party, Screen::InWorld] {
            let view = FrontEndView { screen, reconnecting: Some("Reconnecting".into()), countdown: Some(CountdownDisplay::Seconds(3)), ..FrontEndView::default() };
            let mut list = UiDrawList::new();
            ui.draw(&mut list, &view, VIEWPORT, scale());
            assert!(!list.is_empty(), "{screen:?} drew nothing");
        }
    }
}
