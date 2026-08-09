//! The game's menus, built from the reusable widgets.
//!
//! ```text
//! Playing --ESC--> Menu --SETTINGS--> Settings --BACK/ESC--> Menu --RESUME/ESC--> Playing
//!                   |
//!                   +--QUIT--> MenuAction::QuitRequested
//! ```
//!
//! ## The menu does not stop the game
//!
//! This is a multiplayer game: the world is a shared simulation that keeps running whether or
//! not one player is reading a settings screen. Opening the menu therefore freezes nothing —
//! no tick is skipped, no cooldown is held, and the player's character stays exactly as
//! exposed as it was. The menu is an overlay drawn on top of a live game, and every rule that
//! would have made it a pause is deliberately absent.
//!
//! This module is the *only* place that knows a pause menu exists. Everything below it —
//! widgets, the screen container, the input translation — would build a key-binding screen
//! or a confirmation dialog without a line changing.
//!
//! Nothing here performs an action. Quitting, resuming and changing a setting all leave as
//! a [`MenuAction`] for the application to carry out, which is what keeps the UI from
//! holding a mutable borrow of the world it is drawn over.

use engine_render_data::ui::UiRect;
use engine_settings::{
    CAMERA_SENSITIVITY_MAX, CAMERA_SENSITIVITY_MIN, CAMERA_SENSITIVITY_STEP, GameSettings,
};

use super::input::UiInput;
use super::screen::UiScreen;
use super::widget::{Button, Slider, Widget, WidgetEvent};

/// Which menu is on screen.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MenuScreen {
    /// No menu: the game is being played.
    #[default]
    None,
    /// The pause menu.
    Pause,
    /// The settings screen.
    Settings,
}

impl MenuScreen {
    /// Whether any menu is on screen.
    #[must_use]
    pub fn is_open(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The pause menu's items, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseItem {
    /// Opens the settings screen.
    Settings,
    /// Ends the session.
    Quit,
    /// Closes the menu. Visually separated, at the bottom.
    Resume,
}

/// The settings screen's items, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsItem {
    /// How fast the camera turns.
    CameraSensitivity,
    /// Returns to the pause menu.
    Back,
}

/// Something the application should do about what the player did in a menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuAction {
    /// The menu opened. The game keeps running behind it.
    Opened,
    /// Every menu closed.
    Closed,
    /// The settings screen opened.
    OpenedSettings,
    /// The settings screen closed back to the pause menu.
    ClosedSettings,
    /// The player asked to end the session.
    QuitRequested,
    /// Camera sensitivity is being dragged and now reads this. Apply it; do not save it.
    SensitivityChanged(f32),
    /// Camera sensitivity settled on this value. Apply it and persist it.
    SensitivityCommitted(f32),
}

/// Layout constants, in pixels. The menu is a fixed size and centred, like the HUD.
mod layout {
    /// Width of the menu panel.
    pub const PANEL_WIDTH: f32 = 420.0;
    /// Height of one button.
    pub const BUTTON_HEIGHT: f32 = 52.0;
    /// Gap between buttons inside one group.
    pub const BUTTON_GAP: f32 = 12.0;
    /// Gap between two groups: what makes RESUME read as separate from QUIT.
    pub const GROUP_GAP: f32 = 34.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 28.0;
    /// Height reserved for the panel's title.
    pub const TITLE_HEIGHT: f32 = 56.0;
    /// Height of a slider's track.
    pub const TRACK_HEIGHT: f32 = 18.0;
    /// Space above a slider's track for its label.
    pub const SLIDER_LABEL_HEIGHT: f32 = 30.0;
    /// Space below a slider's track for its current value.
    pub const SLIDER_VALUE_HEIGHT: f32 = 30.0;
    /// Width reserved either side of a track for its minimum and maximum.
    pub const BOUND_WIDTH: f32 = 62.0;
}

/// The menus and which one is showing.
///
/// Both screens are built once, at construction, and kept: opening the settings screen
/// allocates nothing and creates no GPU resource, because a screen is a list of rectangles
/// and the renderer sees only the draw list every frame already produces.
#[derive(Debug)]
pub struct MenuSystem {
    screen: MenuScreen,
    /// What the menu prints at the top: the running build's version.
    title: &'static str,
    pause: UiScreen<PauseItem>,
    settings: UiScreen<SettingsItem>,
    /// This frame's actions. Cleared every update, never freed.
    actions: Vec<MenuAction>,
    /// The viewport the current layout was computed for.
    viewport: (u32, u32),
}

impl MenuSystem {
    /// Builds both screens, with the sensitivity slider showing `settings`.
    ///
    /// `title` is what the menu prints at the top. The application passes its version there:
    /// a menu over a live multiplayer game has no state to announce, and the build a player
    /// is running is the thing worth being able to read off the screen.
    #[must_use]
    pub fn new(settings: GameSettings, title: &'static str) -> Self {
        let mut pause = UiScreen::new();
        pause.push(PauseItem::Settings, Button::new("SETTINGS"));
        pause.push(PauseItem::Quit, Button::new("QUIT"));
        pause.push(PauseItem::Resume, Button::new("RESUME"));

        let mut settings_screen = UiScreen::new();
        settings_screen.push(
            SettingsItem::CameraSensitivity,
            Slider::new(
                "CAMERA SENSITIVITY",
                CAMERA_SENSITIVITY_MIN,
                CAMERA_SENSITIVITY_MAX,
                CAMERA_SENSITIVITY_STEP,
                settings.camera_sensitivity,
            ),
        );
        settings_screen.push(SettingsItem::Back, Button::new("BACK"));

        let mut system = Self {
            screen: MenuScreen::None,
            title,
            pause,
            settings: settings_screen,
            actions: Vec::with_capacity(4),
            viewport: (0, 0),
        };
        system.layout((1920, 1080));
        system
    }

    /// Which menu is showing.
    #[must_use]
    pub fn screen(&self) -> MenuScreen {
        self.screen
    }

    /// Whether any menu is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.screen.is_open()
    }

    /// What the menu prints at the top.
    #[must_use]
    pub fn title(&self) -> &'static str {
        self.title
    }

    /// The main menu's widgets, for drawing.
    #[must_use]
    pub fn pause_screen(&self) -> &UiScreen<PauseItem> {
        &self.pause
    }

    /// The settings screen's widgets, for drawing.
    #[must_use]
    pub fn settings_screen(&self) -> &UiScreen<SettingsItem> {
        &self.settings
    }

    /// The sensitivity slider, which the settings screen draws in full.
    #[must_use]
    pub fn sensitivity_slider(&self) -> Option<&Slider> {
        match self.settings.widget(SettingsItem::CameraSensitivity) {
            Some(Widget::Slider(slider)) => Some(slider),
            _ => None,
        }
    }

    /// Pushes a settings value into the widgets without emitting anything.
    ///
    /// How loaded settings reach the UI. The slider is a view of the value; the value lives
    /// in `SettingsManager`.
    pub fn apply_settings(&mut self, settings: GameSettings) {
        if let Some(Widget::Slider(slider)) =
            self.settings.widget_mut(SettingsItem::CameraSensitivity)
        {
            slider.set_value(settings.camera_sensitivity);
        }
    }

    /// Opens the main menu, if no menu is open. Does not stop the game.
    pub fn open_pause(&mut self) {
        if self.screen.is_open() {
            return;
        }
        self.screen = MenuScreen::Pause;
        self.pause.reset();
        self.actions.push(MenuAction::Opened);
    }

    /// Closes every menu.
    pub fn close(&mut self) {
        if !self.screen.is_open() {
            return;
        }
        self.screen = MenuScreen::None;
        self.pause.reset();
        self.settings.reset();
        self.actions.push(MenuAction::Closed);
    }

    /// Positions every widget for a viewport, in client pixels.
    ///
    /// Only recomputes when the viewport actually changed: layout is arithmetic, but doing
    /// it once per resize instead of once per frame is free and says what it means.
    pub fn layout(&mut self, viewport: (u32, u32)) {
        if viewport == self.viewport {
            return;
        }
        self.viewport = viewport;
        self.layout_pause(viewport);
        self.layout_settings(viewport);
    }

    /// The panel the pause menu draws in.
    #[must_use]
    pub fn pause_panel(&self) -> UiRect {
        // Two groups of one button, then a separated RESUME.
        let content = layout::TITLE_HEIGHT
            + layout::BUTTON_HEIGHT * 3.0
            + layout::BUTTON_GAP
            + layout::GROUP_GAP * 2.0;
        Self::centred_panel(self.viewport, content + layout::PADDING * 2.0)
    }

    /// The panel the settings screen draws in.
    #[must_use]
    pub fn settings_panel(&self) -> UiRect {
        let content = layout::TITLE_HEIGHT
            + layout::SLIDER_LABEL_HEIGHT
            + layout::TRACK_HEIGHT
            + layout::SLIDER_VALUE_HEIGHT
            + layout::GROUP_GAP
            + layout::BUTTON_HEIGHT;
        Self::centred_panel(self.viewport, content + layout::PADDING * 2.0)
    }

    /// A panel of a given height, centred in the viewport.
    fn centred_panel(viewport: (u32, u32), height: f32) -> UiRect {
        let (width, view_height) = (viewport.0 as f32, viewport.1 as f32);
        UiRect::new(
            ((width - layout::PANEL_WIDTH) * 0.5).max(0.0),
            ((view_height - height) * 0.5).max(0.0),
            layout::PANEL_WIDTH,
            height,
        )
    }

    fn layout_pause(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.pause_panel();
        let x = panel.x + layout::PADDING;
        let width = panel.width - layout::PADDING * 2.0;
        let mut y = panel.y + layout::PADDING + layout::TITLE_HEIGHT;

        // Group one: settings and anything a future build puts beside it.
        self.pause.set_rect(
            PauseItem::Settings,
            UiRect::new(x, y, width, layout::BUTTON_HEIGHT),
        );
        y += layout::BUTTON_HEIGHT + layout::GROUP_GAP;

        // Group two: leaving the game.
        self.pause.set_rect(
            PauseItem::Quit,
            UiRect::new(x, y, width, layout::BUTTON_HEIGHT),
        );
        y += layout::BUTTON_HEIGHT + layout::GROUP_GAP;

        // Group three: the way back into the game, separated from everything above it.
        self.pause.set_rect(
            PauseItem::Resume,
            UiRect::new(x, y, width, layout::BUTTON_HEIGHT),
        );
    }

    fn layout_settings(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.settings_panel();
        let x = panel.x + layout::PADDING;
        let width = panel.width - layout::PADDING * 2.0;
        let y = panel.y + layout::PADDING + layout::TITLE_HEIGHT + layout::SLIDER_LABEL_HEIGHT;

        // The track sits between its printed minimum and maximum, so the numbers at either
        // end are the ends of the thing they label.
        self.settings.set_rect(
            SettingsItem::CameraSensitivity,
            UiRect::new(
                x + layout::BOUND_WIDTH,
                y,
                (width - layout::BOUND_WIDTH * 2.0).max(0.0),
                layout::TRACK_HEIGHT,
            ),
        );

        let back_y = y + layout::TRACK_HEIGHT + layout::SLIDER_VALUE_HEIGHT + layout::GROUP_GAP;
        self.settings.set_rect(
            SettingsItem::Back,
            UiRect::new(x, back_y, width, layout::BUTTON_HEIGHT),
        );
    }

    /// Applies one frame of input and returns what the application should do about it.
    ///
    /// Called every frame, open or closed: when no menu is showing the only thing it looks
    /// for is the intent that opens one.
    pub fn update(&mut self, input: &UiInput) -> &[MenuAction] {
        self.actions.clear();
        match self.screen {
            MenuScreen::None => {
                if input.cancel {
                    self.open_pause();
                }
            }
            MenuScreen::Pause => self.update_pause(input),
            MenuScreen::Settings => self.update_settings(input),
        }
        &self.actions
    }

    fn update_pause(&mut self, input: &UiInput) {
        // Escape closes the menu, exactly as RESUME does: the key that opened it is the key
        // that closes it, and the button is there for a player using the mouse.
        if input.cancel {
            self.close();
            return;
        }
        let mut open_settings = false;
        for (item, event) in self.pause.update(input) {
            if *event != WidgetEvent::Activated {
                continue;
            }
            match item {
                PauseItem::Settings => open_settings = true,
                PauseItem::Quit => self.actions.push(MenuAction::QuitRequested),
                PauseItem::Resume => {
                    self.close();
                    return;
                }
            }
        }
        if open_settings {
            self.screen = MenuScreen::Settings;
            self.pause.reset();
            self.settings.reset();
            self.actions.push(MenuAction::OpenedSettings);
        }
    }

    fn update_settings(&mut self, input: &UiInput) {
        if input.cancel {
            self.back_to_pause();
            return;
        }
        let mut back = false;
        // Collected first: the borrow of the screen's event buffer ends here, and the
        // actions below are pushed onto this system's own.
        let mut changed = None;
        let mut committed = None;
        for (item, event) in self.settings.update(input) {
            match (item, event) {
                (SettingsItem::Back, WidgetEvent::Activated) => back = true,
                (SettingsItem::CameraSensitivity, WidgetEvent::ValueChanged(value)) => {
                    changed = Some(*value);
                }
                (SettingsItem::CameraSensitivity, WidgetEvent::ValueCommitted(value)) => {
                    committed = Some(*value);
                }
                _ => {}
            }
        }
        if let Some(value) = changed {
            self.actions.push(MenuAction::SensitivityChanged(value));
        }
        if let Some(value) = committed {
            self.actions.push(MenuAction::SensitivityCommitted(value));
        }
        if back {
            self.back_to_pause();
        }
    }

    fn back_to_pause(&mut self) {
        self.screen = MenuScreen::Pause;
        self.settings.reset();
        self.pause.reset();
        self.actions.push(MenuAction::ClosedSettings);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> UiInput {
        UiInput::new().with_pointer(x, y)
    }

    fn press(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..at(x, y)
        }
    }

    fn hold(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_down: true,
            ..at(x, y)
        }
    }

    fn release(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_released: true,
            ..at(x, y)
        }
    }

    /// Escape: the only key the menu reads.
    fn escape() -> UiInput {
        UiInput {
            cancel: true,
            ..UiInput::new()
        }
    }

    fn menu() -> MenuSystem {
        let mut menu = MenuSystem::new(GameSettings::default(), "0.1.0");
        menu.layout((1280, 720));
        menu
    }

    /// The centre of a pause-menu item, for clicking on it.
    fn centre_of_pause(menu: &MenuSystem, item: PauseItem) -> (f32, f32) {
        let rect = menu
            .pause_screen()
            .widget(item)
            .expect("the item exists")
            .rect();
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    fn centre_of_settings(menu: &MenuSystem, item: SettingsItem) -> (f32, f32) {
        let rect = menu
            .settings_screen()
            .widget(item)
            .expect("the item exists")
            .rect();
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// Clicks a pause-menu item: press then release on it.
    fn click_pause(menu: &mut MenuSystem, item: PauseItem) -> Vec<MenuAction> {
        let (x, y) = centre_of_pause(menu, item);
        menu.update(&press(x, y));
        menu.update(&release(x, y)).to_vec()
    }

    fn click_settings(menu: &mut MenuSystem, item: SettingsItem) -> Vec<MenuAction> {
        let (x, y) = centre_of_settings(menu, item);
        menu.update(&press(x, y));
        menu.update(&release(x, y)).to_vec()
    }

    #[test]
    fn escape_opens_the_pause_menu_from_gameplay() {
        let mut menu = menu();
        assert!(!menu.is_open());
        assert_eq!(menu.screen(), MenuScreen::None);

        let actions = menu.update(&escape()).to_vec();
        assert_eq!(actions, [MenuAction::Opened]);
        assert_eq!(menu.screen(), MenuScreen::Pause);
        assert!(menu.is_open());
    }

    #[test]
    fn escape_closes_the_pause_menu_again() {
        let mut menu = menu();
        menu.update(&escape());
        let actions = menu.update(&escape()).to_vec();
        assert_eq!(actions, [MenuAction::Closed]);
        assert_eq!(menu.screen(), MenuScreen::None);
    }

    #[test]
    fn resume_closes_the_menu() {
        let mut menu = menu();
        menu.open_pause();
        let actions = click_pause(&mut menu, PauseItem::Resume);
        assert_eq!(actions, [MenuAction::Closed]);
        assert!(!menu.is_open());
    }

    #[test]
    fn quit_asks_the_application_to_shut_down_rather_than_doing_it() {
        let mut menu = menu();
        menu.open_pause();
        let actions = click_pause(&mut menu, PauseItem::Quit);
        assert_eq!(actions, [MenuAction::QuitRequested]);
        // The menu stays up: ending the session is the application's decision to carry out.
        assert_eq!(menu.screen(), MenuScreen::Pause);
    }

    #[test]
    fn settings_opens_and_back_returns_to_the_pause_menu() {
        let mut menu = menu();
        menu.open_pause();

        let opened = click_pause(&mut menu, PauseItem::Settings);
        assert_eq!(opened, [MenuAction::OpenedSettings]);
        assert_eq!(menu.screen(), MenuScreen::Settings);

        let closed = click_settings(&mut menu, SettingsItem::Back);
        assert_eq!(closed, [MenuAction::ClosedSettings]);
        assert_eq!(menu.screen(), MenuScreen::Pause);
    }

    #[test]
    fn escape_in_settings_returns_to_the_pause_menu_rather_than_to_the_game() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);

        let actions = menu.update(&escape()).to_vec();
        assert_eq!(actions, [MenuAction::ClosedSettings]);
        assert_eq!(menu.screen(), MenuScreen::Pause);
        assert!(menu.is_open(), "escape from settings closed the whole menu");
    }

    #[test]
    fn the_whole_journey_ends_back_in_gameplay() {
        // Gameplay -> pause -> settings -> pause -> gameplay, the flow from the brief.
        let mut menu = menu();
        menu.update(&escape());
        click_pause(&mut menu, PauseItem::Settings);
        click_settings(&mut menu, SettingsItem::Back);
        click_pause(&mut menu, PauseItem::Resume);
        assert_eq!(menu.screen(), MenuScreen::None);
    }

    #[test]
    fn the_menu_exposes_nothing_that_could_stop_the_game() {
        // This is a multiplayer game: the world keeps running while a player reads a settings
        // screen, so the menu must offer the application no way to freeze it. The only thing
        // it reports is whether it is on screen.
        let mut menu = menu();
        assert!(!menu.is_open());
        menu.open_pause();
        assert!(menu.is_open());
        click_pause(&mut menu, PauseItem::Settings);
        assert!(menu.is_open());
        menu.close();
        assert!(!menu.is_open());
    }

    #[test]
    fn the_menu_prints_the_build_it_is_running() {
        // Where a single-player game would say PAUSED. There is nothing to pause, and the
        // version is what a player reporting a bug needs to be able to read.
        let menu = MenuSystem::new(GameSettings::default(), "1.2.3");
        assert_eq!(menu.title(), "1.2.3");
    }

    #[test]
    fn the_pause_menu_puts_resume_at_the_bottom_separated_from_quit() {
        // The brief's visual hierarchy, asserted rather than eyeballed.
        let menu = menu();
        let rect = |item| {
            menu.pause_screen()
                .widget(item)
                .expect("the item exists")
                .rect()
        };
        let settings = rect(PauseItem::Settings);
        let quit = rect(PauseItem::Quit);
        let resume = rect(PauseItem::Resume);

        assert!(settings.bottom() <= quit.y, "SETTINGS is not above QUIT");
        assert!(quit.bottom() <= resume.y, "QUIT is not above RESUME");

        let settings_to_quit = quit.y - settings.bottom();
        let quit_to_resume = resume.y - quit.bottom();
        assert!(settings_to_quit > 0.0, "the groups touch");
        assert!(
            quit_to_resume >= settings_to_quit,
            "RESUME is not separated from what is above it: {quit_to_resume} vs {settings_to_quit}"
        );
    }

    #[test]
    fn every_widget_stays_inside_its_panel() {
        let mut menu = menu();
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160), (640, 480)] {
            menu.layout(viewport);
            let panel = menu.pause_panel();
            for (item, widget) in menu.pause_screen().widgets() {
                let rect = widget.rect();
                assert!(rect.x >= panel.x, "{item:?} escaped left");
                assert!(
                    rect.right() <= panel.right() + 0.01,
                    "{item:?} escaped right"
                );
                assert!(
                    rect.bottom() <= panel.bottom() + 0.01,
                    "{item:?} escaped below"
                );
            }
            let panel = menu.settings_panel();
            for (item, widget) in menu.settings_screen().widgets() {
                let rect = widget.rect();
                assert!(rect.x >= panel.x, "{item:?} escaped left");
                assert!(
                    rect.right() <= panel.right() + 0.01,
                    "{item:?} escaped right"
                );
            }
        }
    }

    #[test]
    fn dragging_the_sensitivity_slider_reports_changes_then_one_commit() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);

        let track = menu
            .settings_screen()
            .widget(SettingsItem::CameraSensitivity)
            .expect("the slider exists")
            .rect();
        let y = track.y + track.height * 0.5;

        menu.update(&press(track.x + track.width * 0.25, y));
        let dragged = menu.update(&hold(track.x + track.width * 0.75, y)).to_vec();
        assert!(
            matches!(dragged.as_slice(), [MenuAction::SensitivityChanged(_)]),
            "{dragged:?}"
        );

        let released = menu
            .update(&release(track.x + track.width * 0.75, y))
            .to_vec();
        assert!(
            matches!(released.as_slice(), [MenuAction::SensitivityCommitted(_)]),
            "{released:?}"
        );
    }

    #[test]
    fn the_slider_never_leaves_the_settings_range() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);
        let track = menu
            .settings_screen()
            .widget(SettingsItem::CameraSensitivity)
            .expect("the slider exists")
            .rect();
        let y = track.y + track.height * 0.5;

        menu.update(&press(track.x, y));
        for x in [-5_000.0, 0.0, track.x, track.right(), 9_000.0] {
            menu.update(&hold(x, y));
            let value = menu.sensitivity_slider().expect("a slider").value();
            assert!(
                (CAMERA_SENSITIVITY_MIN..=CAMERA_SENSITIVITY_MAX).contains(&value),
                "{value} at x={x}"
            );
        }
    }

    #[test]
    fn loaded_settings_reach_the_slider_without_reporting_a_change() {
        let mut menu = menu();
        menu.apply_settings(GameSettings {
            camera_sensitivity: 2.5,
        });
        assert_eq!(menu.sensitivity_slider().expect("a slider").value(), 2.5);
        assert!(
            menu.update(&at(0.0, 0.0)).is_empty(),
            "applying settings emitted"
        );
    }

    #[test]
    fn the_slider_shows_the_settings_bounds_and_step() {
        let menu = menu();
        let slider = menu.sensitivity_slider().expect("a slider");
        assert_eq!(slider.minimum(), CAMERA_SENSITIVITY_MIN);
        assert_eq!(slider.maximum(), CAMERA_SENSITIVITY_MAX);
        assert_eq!(slider.step(), CAMERA_SENSITIVITY_STEP);
    }

    #[test]
    fn the_menu_answers_the_pointer_and_escape_and_nothing_else() {
        // Every other key belongs to the game, which is still being played behind the menu:
        // a menu that claimed space or tab would take jumping and targeting away mid-fight.
        let mut menu = menu();
        menu.open_pause();

        assert!(menu.update(&UiInput::new()).is_empty());
        assert!(menu.update(&at(0.0, 0.0)).is_empty());
        assert_eq!(
            menu.screen(),
            MenuScreen::Pause,
            "an idle frame changed the screen"
        );

        // The pointer works.
        assert_eq!(
            click_pause(&mut menu, PauseItem::Settings),
            [MenuAction::OpenedSettings]
        );
        // Escape works.
        assert_eq!(
            menu.update(&escape()).to_vec(),
            [MenuAction::ClosedSettings]
        );
    }

    #[test]
    fn opening_a_screen_forgets_the_previous_hover() {
        // Otherwise the button under the pointer in one screen lights up in the next.
        let mut menu = menu();
        menu.open_pause();
        let (x, y) = centre_of_pause(&menu, PauseItem::Settings);
        menu.update(&at(x, y));
        assert_eq!(menu.pause_screen().hovered(), Some(PauseItem::Settings));

        click_pause(&mut menu, PauseItem::Settings);
        assert_eq!(menu.pause_screen().hovered(), None);
        assert_eq!(menu.pause_screen().pressed(), None);
    }

    #[test]
    fn a_closed_menu_ignores_everything_except_the_intent_that_opens_it() {
        let mut menu = menu();
        for input in [press(100.0, 100.0), release(100.0, 100.0), at(50.0, 50.0)] {
            assert!(menu.update(&input).is_empty(), "{input:?} did something");
            assert!(!menu.is_open());
        }
    }

    #[test]
    fn opening_and_closing_repeatedly_leaves_no_state_behind() {
        let mut menu = menu();
        for round in 0..25 {
            menu.update(&escape());
            assert!(menu.is_open(), "round {round} did not open");
            menu.update(&escape());
            assert!(!menu.is_open(), "round {round} did not close");
            assert_eq!(menu.pause_screen().hovered(), None);
        }
    }

    #[test]
    fn the_menu_allocates_nothing_after_the_first_frame() {
        let mut menu = menu();
        menu.open_pause();
        let (x, y) = centre_of_pause(&menu, PauseItem::Quit);
        menu.update(&press(x, y));
        menu.update(&release(x, y));
        let capacity = menu.actions.capacity();

        for _ in 0..1_000 {
            menu.update(&at(x, y));
            menu.update(&press(x, y));
            menu.update(&release(x, y));
        }
        assert_eq!(menu.actions.capacity(), capacity, "the menu reallocated");
    }
}
