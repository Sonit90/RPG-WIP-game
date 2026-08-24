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

use engine_render_data::ui::{UiRect, UiScale};
use engine_settings::{
    BindingScope, CAMERA_SENSITIVITY_MAX, CAMERA_SENSITIVITY_MIN, CAMERA_SENSITIVITY_STEP,
    GameSettings, NameplateSetting, ShadowQuality, UI_SCALE_MAX, UI_SCALE_MIN, UI_SCALE_STEP,
    VOLUME_MAX, VOLUME_MIN, VOLUME_STEP,
};

use engine_game::hero::Hero;

use super::input::UiInput;
use super::screen::UiScreen;
use super::widget::{Button, Checkbox, Slider, Widget, WidgetEvent};

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
    /// The hero roster: which of the playable characters the player is.
    ///
    /// Reached from the pause menu rather than from the settings, because which hero you are
    /// is a fact about the *game* and not a preference about the client — it changes the bar,
    /// the model and what the player can do, and none of that belongs beside a volume
    /// slider.
    Heroes,
    /// The nameplate section of the settings.
    ///
    /// Its own screen rather than six more rows on the settings panel: twelve sliders in one
    /// column is taller than a 720p window, and the reference layout gives nameplates a page
    /// of their own for the same reason.
    Nameplates,
    /// The graphics section of the settings: shadow quality, so far.
    ///
    /// Its own screen for the reason the nameplates have one, and for a second: choosing one
    /// of five levels wants a row per level with a line saying what it buys, which is the
    /// hero roster's shape and not a row on a list of sliders.
    Graphics,
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
    /// Opens the hero roster.
    SwapHero,
    /// Leaves the menu and turns HUD editing on.
    ///
    /// Not a screen of its own: the editor *is* the HUD, so the menu closes rather than
    /// covering the thing the player is about to arrange.
    EditHud,
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
    /// How big the interface is drawn.
    UiScale,
    /// How loud everything is.
    MasterVolume,
    /// How loud the world is.
    SfxVolume,
    /// How loud the interface is.
    UiVolume,
    /// How loud the environment bed is.
    AmbientVolume,
    /// Whether each character remembers its own bars.
    BindingScope,
    /// Opens the graphics section.
    Graphics,
    /// Opens the nameplate section.
    Nameplates,
    /// Returns to the pause menu.
    Back,
}

/// The graphics screen's items, in layout order.
///
/// One row per shadow level plus a way back — the hero roster's shape, because it is the
/// same question: pick exactly one of a short list. A checkbox per level rather than a
/// dropdown, because a dropdown is a widget this framework does not have and five rows fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsItem {
    /// One of the five shadow levels. Ticked when it is the one in force.
    Shadows(ShadowQuality),
    /// Returns to the settings screen.
    Back,
}

/// The hero screen's items, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeroItem {
    /// One of the playable heroes. Ticked when it is the one the player already is.
    Hero(Hero),
    /// Returns to the pause menu.
    Back,
}

/// The nameplate screen's items, in layout order.
///
/// The six sliders are [`NameplateSetting`]s, which is one enum from the file through the
/// screen to the application; only the button that leaves the screen is this screen's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameplateItem {
    /// One of the six settings.
    Setting(NameplateSetting),
    /// Returns to the settings screen.
    Back,
}

/// Which volume a [`MenuAction::VolumeChanged`] is about.
///
/// One action carrying a channel rather than four near-identical actions: the four sliders
/// behave identically, and the application's handling of them is one `match` arm that ends
/// in a different setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeChannel {
    /// Everything, before any category volume.
    Master,
    /// Abilities, impacts, footsteps.
    Sfx,
    /// The interface.
    Ui,
    /// The environment bed.
    Ambient,
}

impl VolumeChannel {
    /// The label its slider prints.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "MASTER VOLUME",
            Self::Sfx => "EFFECTS VOLUME",
            Self::Ui => "INTERFACE VOLUME",
            Self::Ambient => "AMBIENCE VOLUME",
        }
    }
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
    /// The player asked to arrange the HUD. Close the menu and turn edit mode on.
    EditHudRequested,
    /// The player asked to end the session.
    QuitRequested,
    /// Camera sensitivity is being dragged and now reads this. Apply it; do not save it.
    SensitivityChanged(f32),
    /// Camera sensitivity settled on this value. Apply it and persist it.
    SensitivityCommitted(f32),
    /// Interface size is being dragged and now reads this. Apply it; do not save it.
    ///
    /// Applied while dragging on purpose: the menu itself is drawn at this scale, so the
    /// player sees the size they are choosing rather than a number describing it.
    UiScaleChanged(f32),
    /// Interface size settled on this value. Apply it and persist it.
    UiScaleCommitted(f32),
    /// A volume is being dragged and now reads this. Apply it; do not save it.
    ///
    /// Applied while dragging for the same reason the interface size is: the player is
    /// listening to the thing they are setting, and a volume that only took effect on release
    /// would have to be set blind.
    VolumeChanged(VolumeChannel, f32),
    /// A volume settled on this value. Apply it and persist it.
    VolumeCommitted(VolumeChannel, f32),
    /// The hero roster opened.
    OpenedHeroes,
    /// The hero roster closed back to the pause menu.
    ClosedHeroes,
    /// The player chose a hero. Becoming it is the application's to carry out.
    ///
    /// Emitted even for the hero the player already is: the menu reports what was pressed
    /// and the application decides that nothing changed, which is the same division every
    /// other action here follows.
    HeroSelected(Hero),
    /// The player chose where the action bars' spells and keys are kept.
    ///
    /// Committed immediately — a checkbox has no drag to settle — so there is one action
    /// rather than the changed/committed pair the sliders need.
    BindingScopeCommitted(BindingScope),
    /// The nameplate screen opened.
    OpenedNameplates,
    /// The nameplate screen closed back to the settings screen.
    ClosedNameplates,
    /// A nameplate setting is being dragged and now reads this. Apply it; do not save it.
    ///
    /// Applied while dragging, like the interface size and the volumes: every one of these
    /// six changes something the player is looking at while they drag it.
    NameplateChanged(NameplateSetting, f32),
    /// A nameplate setting settled on this value. Apply it and persist it.
    NameplateCommitted(NameplateSetting, f32),
    /// The graphics screen opened.
    OpenedGraphics,
    /// The graphics screen closed back to the settings screen.
    ClosedGraphics,
    /// The player chose a shadow quality. Apply it and persist it.
    ///
    /// Committed immediately — a row of ticks has no drag to settle — so there is one action
    /// rather than the changed/committed pair the sliders need. Emitted even for the level
    /// already in force: the menu reports what was pressed and the application decides that
    /// nothing changed, which is the rule every other action here follows.
    ///
    /// Applying it is not free — the shadow atlas is recreated and the GPU is waited on —
    /// which is exactly why it is a click and not a slider.
    ShadowQualityCommitted(ShadowQuality),
}

/// What a shadow level's row is called.
///
/// Upper case like every other label in the menu, and the same word the file uses so a player
/// reading `settings.cfg` sees what they clicked.
#[must_use]
fn shadow_label(quality: ShadowQuality) -> &'static str {
    match quality {
        ShadowQuality::Off => "SHADOWS: OFF",
        ShadowQuality::Low => "SHADOWS: LOW",
        ShadowQuality::Medium => "SHADOWS: MEDIUM",
        ShadowQuality::High => "SHADOWS: HIGH",
        ShadowQuality::Ultra => "SHADOWS: ULTRA",
    }
}

/// The value one volume channel currently has.
///
/// The one place the mapping from a channel to a field lives; every read of a volume in this
/// module goes through it.
#[must_use]
pub fn volume_of(settings: GameSettings, channel: VolumeChannel) -> f32 {
    match channel {
        VolumeChannel::Master => settings.master_volume,
        VolumeChannel::Sfx => settings.sfx_volume,
        VolumeChannel::Ui => settings.ui_volume,
        VolumeChannel::Ambient => settings.ambient_volume,
    }
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
    /// Height of the line of description under a hero's row.
    pub const HERO_DESCRIPTION_HEIGHT: f32 = 26.0;
}

/// The sliders on the settings screen, in the order they are stacked.
///
/// One list, read by both the layout and the panel that has to be tall enough to hold them,
/// so adding a setting cannot leave the panel the wrong size for its contents.
const SLIDERS: [SettingsItem; 6] = [
    SettingsItem::CameraSensitivity,
    SettingsItem::UiScale,
    SettingsItem::MasterVolume,
    SettingsItem::SfxVolume,
    SettingsItem::UiVolume,
    SettingsItem::AmbientVolume,
];

/// Which volume each volume slider is, and which field of [`GameSettings`] it shows.
///
/// One table, read by construction, by `apply_settings` and by the update loop, so a fifth
/// volume would be one row here rather than three edits that can disagree.
const VOLUME_SLIDERS: [(SettingsItem, VolumeChannel); 4] = [
    (SettingsItem::MasterVolume, VolumeChannel::Master),
    (SettingsItem::SfxVolume, VolumeChannel::Sfx),
    (SettingsItem::UiVolume, VolumeChannel::Ui),
    (SettingsItem::AmbientVolume, VolumeChannel::Ambient),
];

/// How many sliders the settings panel must make room for.
const SLIDER_COUNT: f32 = SLIDERS.len() as f32;

/// How many sliders the nameplate panel must make room for.
const NAMEPLATE_SLIDER_COUNT: f32 = NameplateSetting::ALL.len() as f32;

/// Every menu measurement, converted from its design size to physical pixels once.
///
/// The same arrangement as [`crate::game_ui::HudMetrics`], and for the same reasons: the
/// constants above are logical pixels, this is what the layout actually positions with, and
/// converting in one place is what stops a measurement being scaled twice or not at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuMetrics {
    /// The scale these were derived with.
    pub scale: UiScale,
    /// `layout::PANEL_WIDTH` in physical pixels.
    pub panel_width: f32,
    /// `layout::BUTTON_HEIGHT` in physical pixels.
    pub button_height: f32,
    /// `layout::BUTTON_GAP` in physical pixels.
    pub button_gap: f32,
    /// `layout::GROUP_GAP` in physical pixels.
    pub group_gap: f32,
    /// `layout::PADDING` in physical pixels.
    pub padding: f32,
    /// `layout::TITLE_HEIGHT` in physical pixels.
    pub title_height: f32,
    /// `layout::TRACK_HEIGHT` in physical pixels.
    pub track_height: f32,
    /// `layout::SLIDER_LABEL_HEIGHT` in physical pixels.
    pub slider_label_height: f32,
    /// `layout::SLIDER_VALUE_HEIGHT` in physical pixels.
    pub slider_value_height: f32,
    /// `layout::BOUND_WIDTH` in physical pixels.
    pub bound_width: f32,
    /// `layout::HERO_DESCRIPTION_HEIGHT` in physical pixels.
    pub hero_description_height: f32,
}

impl MenuMetrics {
    /// The menu's measurements at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            scale,
            panel_width: scale.px(layout::PANEL_WIDTH),
            button_height: scale.px(layout::BUTTON_HEIGHT),
            button_gap: scale.px(layout::BUTTON_GAP),
            group_gap: scale.px(layout::GROUP_GAP),
            padding: scale.px(layout::PADDING),
            title_height: scale.px(layout::TITLE_HEIGHT),
            track_height: scale.px(layout::TRACK_HEIGHT),
            slider_label_height: scale.px(layout::SLIDER_LABEL_HEIGHT),
            slider_value_height: scale.px(layout::SLIDER_VALUE_HEIGHT),
            bound_width: scale.px(layout::BOUND_WIDTH),
            hero_description_height: scale.px(layout::HERO_DESCRIPTION_HEIGHT),
        }
    }
}

impl Default for MenuMetrics {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
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
    graphics: UiScreen<GraphicsItem>,
    /// The shadow level the ticks describe.
    ///
    /// Held for the reason [`MenuSystem::hero`] is: a row of checkboxes is one choice, and
    /// the widget that was clicked is not the authority on what the choice now is — the
    /// application is, because applying a level can fail and leave the old one running.
    shadows: ShadowQuality,
    heroes: UiScreen<HeroItem>,
    nameplates: UiScreen<NameplateItem>,
    /// Which hero the roster shows a tick beside.
    ///
    /// A copy of what the simulation says, pushed in by the application. The menu never
    /// decides it: pressing a hero emits an action, and the tick moves when the game comes
    /// back and says it moved.
    hero: Hero,
    /// This frame's actions. Cleared every update, never freed.
    actions: Vec<MenuAction>,
    /// The viewport the current layout was computed for.
    viewport: (u32, u32),
    /// Every measurement, already converted to physical pixels for the current UI scale.
    metrics: MenuMetrics,
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
        pause.push(PauseItem::SwapHero, Button::new("SWAP HERO"));
        pause.push(PauseItem::EditHud, Button::new("EDIT HUD"));
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
        settings_screen.push(
            SettingsItem::UiScale,
            Slider::new(
                "INTERFACE SIZE",
                UI_SCALE_MIN,
                UI_SCALE_MAX,
                UI_SCALE_STEP,
                settings.ui_scale,
            ),
        );
        for (item, channel) in VOLUME_SLIDERS {
            settings_screen.push(
                item,
                Slider::new(
                    channel.label(),
                    VOLUME_MIN,
                    VOLUME_MAX,
                    VOLUME_STEP,
                    volume_of(settings, channel),
                ),
            );
        }
        settings_screen.push(
            SettingsItem::BindingScope,
            Checkbox::new("KEYBINDS PER CHARACTER")
                .checked(settings.binding_scope.is_per_character()),
        );
        settings_screen.push(SettingsItem::Graphics, Button::new("GRAPHICS"));
        settings_screen.push(SettingsItem::Nameplates, Button::new("NAMEPLATES"));
        settings_screen.push(SettingsItem::Back, Button::new("BACK"));

        // The graphics section. One row per shadow level, built from `ShadowQuality::ALL`,
        // so a sixth level is an entry in that array and nothing here.
        let mut graphics_screen = UiScreen::new();
        for quality in ShadowQuality::ALL {
            graphics_screen.push(
                GraphicsItem::Shadows(quality),
                Checkbox::new(shadow_label(quality))
                    .checked(quality == settings.graphics.shadows),
            );
        }
        graphics_screen.push(GraphicsItem::Back, Button::new("BACK"));

        // The roster. One row per hero, built from `Hero::ALL`, so a fourth playable
        // character is a row in that array and nothing here.
        let mut hero_screen = UiScreen::new();
        for hero in Hero::ALL {
            hero_screen.push(
                HeroItem::Hero(hero),
                Checkbox::new(hero.name()).checked(hero == Hero::default()),
            );
        }
        hero_screen.push(HeroItem::Back, Button::new("BACK"));

        // The nameplate section. Every slider is built from the setting itself — its label,
        // its bounds, its step and how many decimals it prints — so adding a seventh is a
        // row in `NameplateSetting::ALL` and nothing here.
        let mut nameplate_screen = UiScreen::new();
        for setting in NameplateSetting::ALL {
            nameplate_screen.push(
                NameplateItem::Setting(setting),
                Slider::new(
                    setting.label(),
                    setting.minimum(),
                    setting.maximum(),
                    setting.step(),
                    settings.nameplates.get(setting),
                )
                .with_decimals(setting.decimals()),
            );
        }
        nameplate_screen.push(NameplateItem::Back, Button::new("BACK"));

        let mut system = Self {
            screen: MenuScreen::None,
            title,
            pause,
            settings: settings_screen,
            heroes: hero_screen,
            nameplates: nameplate_screen,
            graphics: graphics_screen,
            shadows: settings.graphics.shadows,
            hero: Hero::default(),
            actions: Vec::with_capacity(4),
            viewport: (0, 0),
            metrics: MenuMetrics::new(UiScale::new(settings.ui_scale)),
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

    /// The hero roster's widgets, for drawing.
    #[must_use]
    pub fn hero_screen(&self) -> &UiScreen<HeroItem> {
        &self.heroes
    }

    /// Which hero the roster is ticking.
    #[must_use]
    pub fn hero(&self) -> Hero {
        self.hero
    }

    /// Tells the menu which hero the player now is.
    ///
    /// The application calls this after the simulation has actually swapped, which is what
    /// keeps the tick a report rather than a promise.
    pub fn set_hero(&mut self, hero: Hero) {
        self.hero = hero;
        for candidate in Hero::ALL {
            if let Some(Widget::Checkbox(tick)) = self.heroes.widget_mut(HeroItem::Hero(candidate))
            {
                tick.set_checked(candidate == hero);
            }
        }
    }

    /// The graphics screen's widgets, for drawing.
    #[must_use]
    pub fn graphics_screen(&self) -> &UiScreen<GraphicsItem> {
        &self.graphics
    }

    /// The shadow level the graphics screen is showing as chosen.
    #[must_use]
    pub fn shadow_quality(&self) -> ShadowQuality {
        self.shadows
    }

    /// Moves the tick to `quality` without emitting anything.
    ///
    /// What the application calls once it has actually applied a level. The menu never moves
    /// its own tick on a click: applying a shadow level recreates the atlas and can fail, and
    /// a screen that had already ticked the new row would be lying about what is running.
    pub fn set_shadow_quality(&mut self, quality: ShadowQuality) {
        self.shadows = quality;
        for candidate in ShadowQuality::ALL {
            if let Some(Widget::Checkbox(tick)) =
                self.graphics.widget_mut(GraphicsItem::Shadows(candidate))
            {
                tick.set_checked(candidate == quality);
            }
        }
    }

    /// The nameplate screen's widgets, for drawing.
    #[must_use]
    pub fn nameplate_screen(&self) -> &UiScreen<NameplateItem> {
        &self.nameplates
    }

    /// One nameplate setting's slider.
    #[must_use]
    pub fn nameplate_slider(&self, setting: NameplateSetting) -> Option<&Slider> {
        match self.nameplates.widget(NameplateItem::Setting(setting)) {
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
        if let Some(Widget::Slider(slider)) = self.settings.widget_mut(SettingsItem::UiScale) {
            slider.set_value(settings.ui_scale);
        }
        for (item, channel) in VOLUME_SLIDERS {
            if let Some(Widget::Slider(slider)) = self.settings.widget_mut(item) {
                slider.set_value(volume_of(settings, channel));
            }
        }
        if let Some(Widget::Checkbox(checkbox)) =
            self.settings.widget_mut(SettingsItem::BindingScope)
        {
            checkbox.set_checked(settings.binding_scope.is_per_character());
        }
        for setting in NameplateSetting::ALL {
            if let Some(Widget::Slider(slider)) =
                self.nameplates.widget_mut(NameplateItem::Setting(setting))
            {
                slider.set_value(settings.nameplates.get(setting));
            }
        }
        self.set_shadow_quality(settings.graphics.shadows);
        self.set_scale(UiScale::new(settings.ui_scale));
    }

    /// Lays the menu out at a new scale from here on.
    ///
    /// Forces the next [`MenuSystem::layout`] to run even at an unchanged viewport: the
    /// widget rectangles depend on the scale as well as on the window, and skipping the
    /// recompute would leave the menu drawn at one size and clickable at another.
    pub fn set_scale(&mut self, scale: UiScale) {
        if scale == self.metrics.scale {
            return;
        }
        self.metrics = MenuMetrics::new(scale);
        let viewport = self.viewport;
        self.viewport = (0, 0);
        self.layout(viewport);
    }

    /// The scale the menu is laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.metrics.scale
    }

    /// The menu's measurements in physical pixels.
    #[must_use]
    pub fn metrics(&self) -> &MenuMetrics {
        &self.metrics
    }

    /// The interface-size slider, for a caller that wants to draw it fully.
    #[must_use]
    pub fn ui_scale_slider(&self) -> Option<&Slider> {
        match self.settings.widget(SettingsItem::UiScale) {
            Some(Widget::Slider(slider)) => Some(slider),
            _ => None,
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
        self.heroes.reset();
        self.nameplates.reset();
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
        self.layout_heroes(viewport);
        self.layout_nameplates(viewport);
        self.layout_graphics(viewport);
    }

    /// The panel the pause menu draws in.
    #[must_use]
    pub fn pause_panel(&self) -> UiRect {
        // A group of three, then QUIT, then a separated RESUME.
        let content = self.metrics.title_height
            + self.metrics.button_height * 5.0
            + self.metrics.button_gap * 2.0
            + self.metrics.group_gap * 2.0;
        self.centred_panel(self.viewport, content + self.metrics.padding * 2.0)
    }

    /// The panel the hero roster draws in.
    ///
    /// One row per hero with a line of description under each, then a separated BACK.
    #[must_use]
    pub fn hero_panel(&self) -> UiRect {
        let row = self.metrics.button_height
            + self.metrics.hero_description_height
            + self.metrics.button_gap;
        let content = self.metrics.title_height
            + row * Hero::ALL.len() as f32
            + self.metrics.group_gap
            + self.metrics.button_height;
        self.centred_panel(self.viewport, content + self.metrics.padding * 2.0)
    }

    /// The panel the settings screen draws in.
    #[must_use]
    pub fn settings_panel(&self) -> UiRect {
        let slider = self.metrics.slider_label_height
            + self.metrics.track_height
            + self.metrics.slider_value_height;
        // The sliders, then the one checkbox, then the three buttons: the checkbox is a row
        // of button height like the ones under it, and the panel has to reserve every row or
        // the last would be drawn off the bottom of the thing it belongs to.
        let content = self.metrics.title_height
            + slider * SLIDER_COUNT
            + self.metrics.group_gap
            + self.metrics.button_height * 4.0
            + self.metrics.button_gap * 3.0;
        self.centred_panel(self.viewport, content + self.metrics.padding * 2.0)
    }

    /// The panel the graphics screen draws in.
    ///
    /// One row per shadow level with a line of description under each, then a separated
    /// BACK — the hero roster's shape, because it answers the same kind of question.
    #[must_use]
    pub fn graphics_panel(&self) -> UiRect {
        let row = self.metrics.button_height
            + self.metrics.hero_description_height
            + self.metrics.button_gap;
        let content = self.metrics.title_height
            + row * ShadowQuality::ALL.len() as f32
            + self.metrics.group_gap
            + self.metrics.button_height;
        self.centred_panel(self.viewport, content + self.metrics.padding * 2.0)
    }

    /// The panel the nameplate screen draws in.
    #[must_use]
    pub fn nameplate_panel(&self) -> UiRect {
        let slider = self.metrics.slider_label_height
            + self.metrics.track_height
            + self.metrics.slider_value_height;
        let content = self.metrics.title_height
            + slider * NAMEPLATE_SLIDER_COUNT
            + self.metrics.group_gap
            + self.metrics.button_height;
        self.centred_panel(self.viewport, content + self.metrics.padding * 2.0)
    }

    /// A panel of a given height, centred in the viewport.
    fn centred_panel(&self, viewport: (u32, u32), height: f32) -> UiRect {
        let (width, view_height) = (viewport.0 as f32, viewport.1 as f32);
        UiRect::new(
            ((width - self.metrics.panel_width) * 0.5).max(0.0),
            ((view_height - height) * 0.5).max(0.0),
            self.metrics.panel_width,
            height,
        )
    }

    fn layout_pause(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.pause_panel();
        let x = panel.x + self.metrics.padding;
        let width = panel.width - self.metrics.padding * 2.0;
        let mut y = panel.y + self.metrics.padding + self.metrics.title_height;

        // Group one: the things that change the game without leaving it.
        self.pause.set_rect(
            PauseItem::SwapHero,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;
        self.pause.set_rect(
            PauseItem::EditHud,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;
        self.pause.set_rect(
            PauseItem::Settings,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.group_gap;

        // Group two: leaving the game.
        self.pause.set_rect(
            PauseItem::Quit,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.group_gap;

        // Group three: the way back into the game, separated from everything above it.
        self.pause.set_rect(
            PauseItem::Resume,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
    }

    fn layout_settings(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.settings_panel();
        let x = panel.x + self.metrics.padding;
        let width = panel.width - self.metrics.padding * 2.0;
        let mut y = panel.y + self.metrics.padding + self.metrics.title_height;

        // One row per slider, stacked in the order they are listed. The track sits between
        // its printed minimum and maximum, so the numbers at either end are the ends of the
        // thing they label.
        let track = UiRect::new(
            x + self.metrics.bound_width,
            0.0,
            (width - self.metrics.bound_width * 2.0).max(0.0),
            self.metrics.track_height,
        );
        for item in SLIDERS {
            y += self.metrics.slider_label_height;
            self.settings.set_rect(item, UiRect { y, ..track });
            y += self.metrics.track_height + self.metrics.slider_value_height;
        }

        // Where the bars are kept, then the two sections, then the way out of the screen.
        let mut y = y + self.metrics.group_gap;
        self.settings.set_rect(
            SettingsItem::BindingScope,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;
        self.settings.set_rect(
            SettingsItem::Graphics,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;
        self.settings.set_rect(
            SettingsItem::Nameplates,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;
        self.settings.set_rect(
            SettingsItem::Back,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
    }

    fn layout_heroes(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.hero_panel();
        let x = panel.x + self.metrics.padding;
        let width = panel.width - self.metrics.padding * 2.0;
        let mut y = panel.y + self.metrics.padding + self.metrics.title_height;

        for hero in Hero::ALL {
            self.heroes.set_rect(
                HeroItem::Hero(hero),
                UiRect::new(x, y, width, self.metrics.button_height),
            );
            // The gap under a row is the description's line plus the ordinary spacing: the
            // text is drawn in it, so the layout reserves it rather than the painter
            // borrowing the next row's.
            y += self.metrics.button_height
                + self.metrics.hero_description_height
                + self.metrics.button_gap;
        }

        self.heroes.set_rect(
            HeroItem::Back,
            UiRect::new(
                x,
                y - self.metrics.button_gap + self.metrics.group_gap,
                width,
                self.metrics.button_height,
            ),
        );
    }

    fn layout_graphics(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.graphics_panel();
        let x = panel.x + self.metrics.padding;
        let width = panel.width - self.metrics.padding * 2.0;
        let mut y = panel.y + self.metrics.padding + self.metrics.title_height;

        for quality in ShadowQuality::ALL {
            self.graphics.set_rect(
                GraphicsItem::Shadows(quality),
                UiRect::new(x, y, width, self.metrics.button_height),
            );
            // The description's line is reserved by the layout rather than borrowed from the
            // next row by the painter, exactly as the hero roster does it.
            y += self.metrics.button_height
                + self.metrics.hero_description_height
                + self.metrics.button_gap;
        }

        self.graphics.set_rect(
            GraphicsItem::Back,
            UiRect::new(
                x,
                y - self.metrics.button_gap + self.metrics.group_gap,
                width,
                self.metrics.button_height,
            ),
        );
    }

    fn layout_nameplates(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.nameplate_panel();
        let x = panel.x + self.metrics.padding;
        let width = panel.width - self.metrics.padding * 2.0;
        let mut y = panel.y + self.metrics.padding + self.metrics.title_height;

        let track = UiRect::new(
            x + self.metrics.bound_width,
            0.0,
            (width - self.metrics.bound_width * 2.0).max(0.0),
            self.metrics.track_height,
        );
        for setting in NameplateSetting::ALL {
            y += self.metrics.slider_label_height;
            self.nameplates
                .set_rect(NameplateItem::Setting(setting), UiRect { y, ..track });
            y += self.metrics.track_height + self.metrics.slider_value_height;
        }

        self.nameplates.set_rect(
            NameplateItem::Back,
            UiRect::new(
                x,
                y + self.metrics.group_gap,
                width,
                self.metrics.button_height,
            ),
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
            MenuScreen::Heroes => self.update_heroes(input),
            MenuScreen::Nameplates => self.update_nameplates(input),
            MenuScreen::Graphics => self.update_graphics(input),
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
        let mut open_heroes = false;
        for (item, event) in self.pause.update(input) {
            if *event != WidgetEvent::Activated {
                continue;
            }
            match item {
                PauseItem::SwapHero => open_heroes = true,
                PauseItem::EditHud => {
                    // The menu gets out of the way: what the player is about to arrange is
                    // underneath it.
                    self.close();
                    self.actions.push(MenuAction::EditHudRequested);
                    return;
                }
                PauseItem::Settings => open_settings = true,
                PauseItem::Quit => self.actions.push(MenuAction::QuitRequested),
                PauseItem::Resume => {
                    self.close();
                    return;
                }
            }
        }
        if open_heroes {
            self.screen = MenuScreen::Heroes;
            self.pause.reset();
            self.heroes.reset();
            self.actions.push(MenuAction::OpenedHeroes);
            return;
        }
        if open_settings {
            self.screen = MenuScreen::Settings;
            self.pause.reset();
            self.settings.reset();
            self.actions.push(MenuAction::OpenedSettings);
        }
    }

    /// The hero roster: one row per hero, and a way back.
    ///
    /// Choosing a hero closes the menu. A swap changes the bar, the model and everything the
    /// player is holding, and a panel still covering the screen afterwards would hide the
    /// only thing that just happened.
    fn update_heroes(&mut self, input: &UiInput) {
        if input.cancel {
            self.back_to_pause_from_heroes();
            return;
        }
        let mut chosen = None;
        let mut back = false;
        for (item, event) in self.heroes.update(input) {
            match (item, event) {
                (HeroItem::Back, WidgetEvent::Activated) => back = true,
                // A checkbox reports the tick it just moved. Which hero was pressed is what
                // matters; the tick itself is put back where the game says it belongs below.
                (HeroItem::Hero(hero), WidgetEvent::Toggled(_) | WidgetEvent::Activated) => {
                    chosen = Some(*hero);
                }
                _ => {}
            }
        }
        let hero = self.hero;
        self.set_hero(hero);
        if let Some(hero) = chosen {
            self.actions.push(MenuAction::HeroSelected(hero));
            self.close();
            return;
        }
        if back {
            self.back_to_pause_from_heroes();
        }
    }

    fn back_to_pause_from_heroes(&mut self) {
        self.screen = MenuScreen::Pause;
        self.heroes.reset();
        self.pause.reset();
        self.actions.push(MenuAction::ClosedHeroes);
    }

    fn update_settings(&mut self, input: &UiInput) {
        if input.cancel {
            self.back_to_pause();
            return;
        }
        let mut back = false;
        let mut open_nameplates = false;
        let mut open_graphics = false;
        let mut binding_scope = None;
        // Collected first: the borrow of the screen's event buffer ends here, and the
        // actions below are pushed onto this system's own.
        let mut sensitivity = (None, None);
        let mut ui_scale = (None, None);
        let mut volumes: Vec<(VolumeChannel, f32, bool)> = Vec::new();
        for (item, event) in self.settings.update(input) {
            match (item, event) {
                (SettingsItem::Back, WidgetEvent::Activated) => back = true,
                (SettingsItem::Nameplates, WidgetEvent::Activated) => open_nameplates = true,
                (SettingsItem::Graphics, WidgetEvent::Activated) => open_graphics = true,
                (SettingsItem::BindingScope, WidgetEvent::Toggled(checked)) => {
                    binding_scope = Some(BindingScope::from_per_character(*checked));
                }
                (SettingsItem::CameraSensitivity, WidgetEvent::ValueChanged(value)) => {
                    sensitivity.0 = Some(*value);
                }
                (SettingsItem::CameraSensitivity, WidgetEvent::ValueCommitted(value)) => {
                    sensitivity.1 = Some(*value);
                }
                (SettingsItem::UiScale, WidgetEvent::ValueChanged(value)) => {
                    ui_scale.0 = Some(*value);
                }
                (SettingsItem::UiScale, WidgetEvent::ValueCommitted(value)) => {
                    ui_scale.1 = Some(*value);
                }
                (item, event) => {
                    // The four volumes behave identically, so they are handled as a group:
                    // which one it was is data, not a branch.
                    if let Some((_, channel)) =
                        VOLUME_SLIDERS.iter().find(|(slider, _)| slider == item)
                    {
                        match event {
                            WidgetEvent::ValueChanged(value) => {
                                volumes.push((*channel, *value, false));
                            }
                            WidgetEvent::ValueCommitted(value) => {
                                volumes.push((*channel, *value, true));
                            }
                            WidgetEvent::Activated | WidgetEvent::Toggled(_) => {}
                        }
                    }
                }
            }
        }
        for (channel, value, committed) in volumes {
            self.actions.push(if committed {
                MenuAction::VolumeCommitted(channel, value)
            } else {
                MenuAction::VolumeChanged(channel, value)
            });
        }
        if let Some(value) = sensitivity.0 {
            self.actions.push(MenuAction::SensitivityChanged(value));
        }
        if let Some(value) = sensitivity.1 {
            self.actions.push(MenuAction::SensitivityCommitted(value));
        }
        if let Some(value) = ui_scale.0 {
            self.actions.push(MenuAction::UiScaleChanged(value));
        }
        if let Some(value) = ui_scale.1 {
            self.actions.push(MenuAction::UiScaleCommitted(value));
        }
        if let Some(scope) = binding_scope {
            self.actions.push(MenuAction::BindingScopeCommitted(scope));
        }
        if open_graphics {
            self.screen = MenuScreen::Graphics;
            self.settings.reset();
            self.graphics.reset();
            self.actions.push(MenuAction::OpenedGraphics);
            return;
        }
        if open_nameplates {
            self.screen = MenuScreen::Nameplates;
            self.settings.reset();
            self.nameplates.reset();
            self.actions.push(MenuAction::OpenedNameplates);
            return;
        }
        if back {
            self.back_to_pause();
        }
    }

    /// The nameplate section: six sliders that all behave the same way, and a way back.
    ///
    /// One loop rather than six branches — which setting a value belongs to is data the
    /// widget is tagged with, so a seventh setting needs nothing here.
    fn update_nameplates(&mut self, input: &UiInput) {
        if input.cancel {
            self.back_to_settings();
            return;
        }
        let mut back = false;
        let mut changes: [Option<(NameplateSetting, f32, bool)>; NameplateSetting::ALL.len() * 2] =
            [None; NameplateSetting::ALL.len() * 2];
        let mut count = 0;
        for (item, event) in self.nameplates.update(input) {
            match (item, event) {
                (NameplateItem::Back, WidgetEvent::Activated) => back = true,
                (NameplateItem::Setting(setting), WidgetEvent::ValueChanged(value))
                    if count < changes.len() =>
                {
                    changes[count] = Some((*setting, *value, false));
                    count += 1;
                }
                (NameplateItem::Setting(setting), WidgetEvent::ValueCommitted(value))
                    if count < changes.len() =>
                {
                    changes[count] = Some((*setting, *value, true));
                    count += 1;
                }
                _ => {}
            }
        }
        for change in changes.iter().take(count).flatten() {
            let (setting, value, committed) = *change;
            self.actions.push(if committed {
                MenuAction::NameplateCommitted(setting, value)
            } else {
                MenuAction::NameplateChanged(setting, value)
            });
        }
        if back {
            self.back_to_settings();
        }
    }

    /// The graphics section: one tick per shadow level, and a way back.
    ///
    /// The tick the click moved is put straight back where the application says it belongs,
    /// exactly as the hero roster does: the screen reports what was pressed, and what is
    /// actually running is not its to decide. Choosing a level does **not** close the menu —
    /// unlike choosing a hero — because the whole point of the screen is to look at the
    /// result and try the next one down if it is not fast enough.
    fn update_graphics(&mut self, input: &UiInput) {
        if input.cancel {
            self.back_to_settings_from_graphics();
            return;
        }
        let mut back = false;
        let mut chosen = None;
        for (item, event) in self.graphics.update(input) {
            match (item, event) {
                (GraphicsItem::Back, WidgetEvent::Activated) => back = true,
                (
                    GraphicsItem::Shadows(quality),
                    WidgetEvent::Toggled(_) | WidgetEvent::Activated,
                ) => {
                    chosen = Some(*quality);
                }
                _ => {}
            }
        }
        let current = self.shadows;
        self.set_shadow_quality(current);
        if let Some(quality) = chosen {
            self.actions.push(MenuAction::ShadowQualityCommitted(quality));
        }
        if back {
            self.back_to_settings_from_graphics();
        }
    }

    /// Escape and BACK from the graphics section return to the settings screen, not to the
    /// game — the rule every section of the settings follows.
    fn back_to_settings_from_graphics(&mut self) {
        self.screen = MenuScreen::Settings;
        self.graphics.reset();
        self.settings.reset();
        self.actions.push(MenuAction::ClosedGraphics);
    }

    fn back_to_pause(&mut self) {
        self.screen = MenuScreen::Pause;
        self.settings.reset();
        self.pause.reset();
        self.actions.push(MenuAction::ClosedSettings);
    }

    /// Escape and BACK from the nameplate section return to the settings screen it was
    /// opened from, not to the game — the same rule the settings screen itself follows.
    fn back_to_settings(&mut self) {
        self.screen = MenuScreen::Settings;
        self.nameplates.reset();
        self.settings.reset();
        self.actions.push(MenuAction::ClosedNameplates);
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

    fn centre_of_hero(menu: &MenuSystem, item: HeroItem) -> (f32, f32) {
        let rect = menu
            .hero_screen()
            .widget(item)
            .expect("the item exists")
            .rect();
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    fn click_hero(menu: &mut MenuSystem, item: HeroItem) -> Vec<MenuAction> {
        let (x, y) = centre_of_hero(menu, item);
        menu.update(&press(x, y));
        menu.update(&release(x, y)).to_vec()
    }

    #[test]
    fn the_pause_menu_offers_the_hero_roster_and_escape_comes_back_from_it() {
        let mut menu = menu();
        menu.open_pause();
        let actions = click_pause(&mut menu, PauseItem::SwapHero);
        assert_eq!(actions, [MenuAction::OpenedHeroes]);
        assert_eq!(menu.screen(), MenuScreen::Heroes);

        let actions = menu.update(&escape()).to_vec();
        assert_eq!(actions, [MenuAction::ClosedHeroes]);
        assert_eq!(menu.screen(), MenuScreen::Pause);
    }

    #[test]
    fn the_roster_lists_every_hero_and_ticks_the_one_you_are() {
        let mut menu = menu();
        for hero in Hero::ALL {
            let widget = menu
                .hero_screen()
                .widget(HeroItem::Hero(hero))
                .expect("every hero has a row");
            let Widget::Checkbox(tick) = widget else {
                panic!("a hero's row is a tick box");
            };
            assert_eq!(tick.label, hero.name());
            assert_eq!(tick.checked, hero == Hero::default());
        }

        menu.set_hero(Hero::Druid);
        assert_eq!(menu.hero(), Hero::Druid);
        for hero in Hero::ALL {
            let Some(Widget::Checkbox(tick)) = menu.hero_screen().widget(HeroItem::Hero(hero))
            else {
                panic!("a hero's row is a tick box");
            };
            assert_eq!(tick.checked, hero == Hero::Druid, "{hero:?}");
        }
    }

    #[test]
    fn choosing_a_hero_reports_it_and_leaves_the_menu() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::SwapHero);
        let actions = click_hero(&mut menu, HeroItem::Hero(Hero::Warrior));
        assert_eq!(
            actions,
            [MenuAction::HeroSelected(Hero::Warrior), MenuAction::Closed]
        );
        assert!(!menu.is_open());

        // The menu did not decide anything: until the application says the swap happened,
        // the tick is still on the hero the game says the player is.
        assert_eq!(menu.hero(), Hero::default());
        let Some(Widget::Checkbox(tick)) = menu.hero_screen().widget(HeroItem::Hero(Hero::Warrior))
        else {
            panic!("a hero's row is a tick box");
        };
        assert!(
            !tick.checked,
            "the menu ticked a hero the game has not made"
        );
    }

    #[test]
    fn the_roster_leaves_room_for_every_row_it_draws() {
        // The same rule the settings panel follows: a panel that is not tall enough for its
        // contents puts the last row outside the box it is drawn in.
        let menu = menu();
        let panel = menu.hero_panel();
        let back = menu
            .hero_screen()
            .widget(HeroItem::Back)
            .expect("the way back exists")
            .rect();
        assert!(
            back.bottom() <= panel.bottom() + 0.5,
            "BACK ends at {} in a panel that ends at {}",
            back.bottom(),
            panel.bottom()
        );
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
    fn ticking_the_keybind_box_reports_the_scope_the_player_chose() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);

        let ticked = click_settings(&mut menu, SettingsItem::BindingScope);
        assert!(
            matches!(
                ticked.as_slice(),
                [MenuAction::BindingScopeCommitted(BindingScope::Character)]
            ),
            "{ticked:?}"
        );

        // And back off again: the box is a view of the setting, so unticking it asks for the
        // account-wide arrangement rather than repeating the same answer.
        let unticked = click_settings(&mut menu, SettingsItem::BindingScope);
        assert!(
            matches!(
                unticked.as_slice(),
                [MenuAction::BindingScopeCommitted(BindingScope::Account)]
            ),
            "{unticked:?}"
        );
    }

    #[test]
    fn the_keybind_box_shows_what_the_settings_file_says() {
        // Loaded settings reach the widget through `apply_settings`, and must not come back
        // out of it as an action: nothing was pressed.
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);
        menu.apply_settings(GameSettings {
            binding_scope: BindingScope::Character,
            ..GameSettings::default()
        });
        let Some(Widget::Checkbox(checkbox)) = menu.settings_screen().widget(SettingsItem::BindingScope)
        else {
            panic!("the settings screen has no keybind box");
        };
        assert!(checkbox.checked);
        assert!(menu.update(&at(0.0, 0.0)).is_empty());
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
    fn the_settings_screen_stacks_both_sliders_inside_its_panel_without_overlapping() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);
        let panel = menu.settings_panel();

        let mut previous: Option<UiRect> = None;
        for item in SLIDERS {
            let rect = menu
                .settings_screen()
                .widget(item)
                .expect("every slider is on the screen")
                .rect();
            assert!(rect.y >= panel.y, "{item:?} escaped the top of the panel");
            assert!(
                rect.bottom() <= panel.bottom(),
                "{item:?} escaped the bottom of the panel"
            );
            if let Some(above) = previous {
                assert!(
                    rect.y > above.bottom(),
                    "{item:?} overlaps the slider above it"
                );
            }
            previous = Some(rect);
        }

        // BACK still sits below both of them, inside the panel.
        let back = menu
            .settings_screen()
            .widget(SettingsItem::Back)
            .expect("BACK exists")
            .rect();
        assert!(back.y > previous.expect("a slider").bottom());
        assert!(back.bottom() <= panel.bottom());
    }

    #[test]
    fn changing_the_interface_size_relays_out_the_menu_that_is_showing_it() {
        // The slider is on the screen it resizes, so the layout has to follow the value
        // even though the window has not changed — the case a viewport-only check misses.
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);
        let before = menu.settings_panel();

        menu.set_scale(UiScale::new(2.0));
        let after = menu.settings_panel();
        assert!(
            after.height > before.height,
            "the panel did not grow: {before:?} then {after:?}"
        );

        // And the widgets moved with it, rather than staying where they were drawn.
        let rect = menu
            .settings_screen()
            .widget(SettingsItem::Back)
            .expect("BACK exists")
            .rect();
        assert!(rect.bottom() <= after.bottom());
        assert!(rect.height > 0.0);
    }

    #[test]
    fn dragging_the_interface_size_reports_its_own_action_and_not_the_sensitivity() {
        // Two sliders on one screen: each must report as itself, or changing the size would
        // silently turn the camera.
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);
        let track = menu
            .settings_screen()
            .widget(SettingsItem::UiScale)
            .expect("the interface slider exists")
            .rect();
        let (x, y) = (track.x + track.width * 0.5, track.y + track.height * 0.5);

        let actions = menu.update(&press(x, y)).to_vec();
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, MenuAction::UiScaleChanged(_))),
            "{actions:?}"
        );
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, MenuAction::SensitivityChanged(_))),
            "the other slider moved too: {actions:?}"
        );

        let actions = menu.update(&release(x, y)).to_vec();
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, MenuAction::UiScaleCommitted(_))),
            "{actions:?}"
        );
    }

    #[test]
    fn loaded_settings_reach_the_slider_without_reporting_a_change() {
        let mut menu = menu();
        menu.apply_settings(GameSettings {
            camera_sensitivity: 2.5,
            ..GameSettings::default()
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

    /// Opens the pause menu, the settings screen and then the nameplate section.
    fn open_nameplates(menu: &mut MenuSystem) {
        menu.open_pause();
        click_pause(menu, PauseItem::Settings);
        click_settings(menu, SettingsItem::Nameplates);
    }

    fn nameplate_track(menu: &MenuSystem, setting: NameplateSetting) -> UiRect {
        menu.nameplate_screen()
            .widget(NameplateItem::Setting(setting))
            .expect("every setting has a slider")
            .rect()
    }

    #[test]
    fn the_settings_screen_opens_the_nameplate_section_and_escape_returns_to_it() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);

        let opened = click_settings(&mut menu, SettingsItem::Nameplates);
        assert_eq!(opened, [MenuAction::OpenedNameplates]);
        assert_eq!(menu.screen(), MenuScreen::Nameplates);

        // Escape goes back one screen, not out to the game: the same rule the settings
        // screen itself follows.
        let closed = menu.update(&escape()).to_vec();
        assert_eq!(closed, [MenuAction::ClosedNameplates]);
        assert_eq!(menu.screen(), MenuScreen::Settings);

        // And so does BACK.
        click_settings(&mut menu, SettingsItem::Nameplates);
        let (x, y) = {
            let rect = nameplate_track(&menu, NameplateSetting::Size);
            let _ = rect;
            let back = menu
                .nameplate_screen()
                .widget(NameplateItem::Back)
                .expect("BACK exists")
                .rect();
            (back.x + back.width * 0.5, back.y + back.height * 0.5)
        };
        menu.update(&press(x, y));
        let closed = menu.update(&release(x, y)).to_vec();
        assert_eq!(closed, [MenuAction::ClosedNameplates]);
        assert_eq!(menu.screen(), MenuScreen::Settings);
    }

    #[test]
    fn every_nameplate_setting_has_a_slider_showing_its_bounds_and_its_value() {
        let mut menu = menu();
        open_nameplates(&mut menu);
        for setting in NameplateSetting::ALL {
            let slider = menu.nameplate_slider(setting).expect("a slider");
            assert_eq!(slider.minimum(), setting.minimum(), "{setting:?}");
            assert_eq!(slider.maximum(), setting.maximum(), "{setting:?}");
            assert_eq!(slider.step(), setting.step(), "{setting:?}");
            assert_eq!(slider.value(), setting.default_value(), "{setting:?}");
            assert_eq!(slider.decimals(), setting.decimals(), "{setting:?}");
        }
    }

    #[test]
    fn dragging_a_nameplate_slider_reports_that_setting_and_no_other() {
        let mut menu = menu();
        open_nameplates(&mut menu);
        let track = nameplate_track(&menu, NameplateSetting::HeightAboveHead);
        let (x, y) = (track.x + track.width * 0.75, track.y + track.height * 0.5);

        let dragged = menu.update(&press(x, y)).to_vec();
        assert!(
            dragged.iter().any(|action| matches!(
                action,
                MenuAction::NameplateChanged(NameplateSetting::HeightAboveHead, _)
            )),
            "{dragged:?}"
        );
        assert_eq!(dragged.len(), 1, "another slider moved too: {dragged:?}");

        let released = menu.update(&release(x, y)).to_vec();
        assert!(
            released.iter().any(|action| matches!(
                action,
                MenuAction::NameplateCommitted(NameplateSetting::HeightAboveHead, _)
            )),
            "{released:?}"
        );
    }

    #[test]
    fn a_nameplate_slider_never_leaves_its_settings_range() {
        let mut menu = menu();
        open_nameplates(&mut menu);
        let track = nameplate_track(&menu, NameplateSetting::TopSafeZone);
        let y = track.y + track.height * 0.5;

        menu.update(&press(track.x, y));
        for x in [-9_000.0, 0.0, track.x, track.right(), 9_000.0] {
            menu.update(&hold(x, y));
            let value = menu
                .nameplate_slider(NameplateSetting::TopSafeZone)
                .expect("a slider")
                .value();
            assert!(
                (NameplateSetting::TopSafeZone.minimum()..=NameplateSetting::TopSafeZone.maximum())
                    .contains(&value),
                "{value} at x={x}"
            );
        }
    }

    #[test]
    fn loaded_nameplate_settings_reach_their_sliders_without_reporting_a_change() {
        let mut menu = menu();
        let settings = GameSettings {
            nameplates: engine_settings::NameplateSettings::default()
                .with(NameplateSetting::Size, 18.0)
                .with(NameplateSetting::VerticalMargin, 1.5),
            ..GameSettings::default()
        };
        menu.apply_settings(settings);

        assert_eq!(
            menu.nameplate_slider(NameplateSetting::Size)
                .expect("a slider")
                .value(),
            18.0
        );
        assert_eq!(
            menu.nameplate_slider(NameplateSetting::VerticalMargin)
                .expect("a slider")
                .value(),
            1.5
        );
        assert!(
            menu.update(&at(0.0, 0.0)).is_empty(),
            "applying settings emitted"
        );
    }


    fn centre_of_graphics(menu: &MenuSystem, item: GraphicsItem) -> (f32, f32) {
        let rect = menu
            .graphics_screen()
            .widget(item)
            .expect("the item exists")
            .rect();
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    fn click_graphics(menu: &mut MenuSystem, item: GraphicsItem) -> Vec<MenuAction> {
        let (x, y) = centre_of_graphics(menu, item);
        menu.update(&press(x, y));
        menu.update(&release(x, y)).to_vec()
    }

    fn open_graphics(menu: &mut MenuSystem) {
        menu.open_pause();
        click_pause(menu, PauseItem::Settings);
        click_settings(menu, SettingsItem::Graphics);
    }

    fn settings_with_shadows(shadows: ShadowQuality) -> GameSettings {
        GameSettings {
            graphics: engine_settings::GraphicsSettings { shadows },
            ..GameSettings::default()
        }
    }

    fn is_ticked(menu: &MenuSystem, quality: ShadowQuality) -> bool {
        match menu.graphics_screen().widget(GraphicsItem::Shadows(quality)) {
            Some(Widget::Checkbox(tick)) => tick.checked,
            _ => panic!("{} has no row", quality.as_str()),
        }
    }

    #[test]
    fn the_settings_screen_opens_the_graphics_section_and_escape_returns_to_it() {
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);

        let opened = click_settings(&mut menu, SettingsItem::Graphics);
        assert_eq!(opened, [MenuAction::OpenedGraphics]);
        assert_eq!(menu.screen(), MenuScreen::Graphics);

        // Escape goes back to the settings, not out to the game.
        let closed = menu.update(&escape()).to_vec();
        assert_eq!(closed, [MenuAction::ClosedGraphics]);
        assert_eq!(menu.screen(), MenuScreen::Settings);

        // And so does BACK.
        click_settings(&mut menu, SettingsItem::Graphics);
        let closed = click_graphics(&mut menu, GraphicsItem::Back);
        assert_eq!(closed, [MenuAction::ClosedGraphics]);
        assert_eq!(menu.screen(), MenuScreen::Settings);
    }

    #[test]
    fn the_graphics_screen_offers_every_shadow_level_with_the_current_one_ticked() {
        let mut menu = MenuSystem::new(settings_with_shadows(ShadowQuality::Medium), "0.1.0");
        menu.layout((1280, 720));

        for quality in ShadowQuality::ALL {
            assert_eq!(
                is_ticked(&menu, quality),
                quality == ShadowQuality::Medium,
                "{} is ticked wrongly",
                quality.as_str()
            );
        }
        assert_eq!(menu.shadow_quality(), ShadowQuality::Medium);
    }

    #[test]
    fn choosing_a_level_reports_it_and_leaves_the_tick_where_the_game_says() {
        // The rule the hero roster follows, and for a stronger reason: applying a shadow
        // level recreates the atlas and can fail, so the screen must not tick a row it has
        // no evidence is running.
        let mut menu = menu();
        open_graphics(&mut menu);
        assert_eq!(menu.shadow_quality(), ShadowQuality::High);

        let actions = click_graphics(&mut menu, GraphicsItem::Shadows(ShadowQuality::Ultra));
        assert_eq!(
            actions,
            [MenuAction::ShadowQualityCommitted(ShadowQuality::Ultra)]
        );
        assert_eq!(
            menu.shadow_quality(),
            ShadowQuality::High,
            "the menu moved its own tick before the application applied anything"
        );
        assert!(
            !is_ticked(&menu, ShadowQuality::Ultra),
            "the clicked row ticked itself"
        );
        assert!(is_ticked(&menu, ShadowQuality::High));

        // Once the application has applied it, the tick moves.
        menu.set_shadow_quality(ShadowQuality::Ultra);
        assert_eq!(menu.shadow_quality(), ShadowQuality::Ultra);
        assert!(is_ticked(&menu, ShadowQuality::Ultra));
        assert!(!is_ticked(&menu, ShadowQuality::High));
    }

    #[test]
    fn choosing_a_level_does_not_close_the_menu() {
        // Unlike choosing a hero. The point of the screen is to look at the result and try
        // the next level down if it is not fast enough, which a screen that closed itself
        // would turn into five round trips through two menus.
        let mut menu = menu();
        open_graphics(&mut menu);
        click_graphics(&mut menu, GraphicsItem::Shadows(ShadowQuality::Low));
        assert_eq!(menu.screen(), MenuScreen::Graphics);
    }

    #[test]
    fn choosing_the_level_already_in_force_still_reports_it() {
        // The menu reports what was pressed; deciding that nothing changed is the
        // application's, which is the division every other action here follows.
        let mut menu = menu();
        open_graphics(&mut menu);
        let actions = click_graphics(&mut menu, GraphicsItem::Shadows(ShadowQuality::High));
        assert_eq!(
            actions,
            [MenuAction::ShadowQualityCommitted(ShadowQuality::High)]
        );
    }

    #[test]
    fn loaded_settings_tick_the_level_they_name() {
        let mut menu = menu();
        menu.apply_settings(settings_with_shadows(ShadowQuality::Off));
        assert_eq!(menu.shadow_quality(), ShadowQuality::Off);
        assert!(is_ticked(&menu, ShadowQuality::Off));
        assert!(!is_ticked(&menu, ShadowQuality::High));
    }

    #[test]
    fn every_graphics_row_stays_inside_its_panel() {
        let mut menu = menu();
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160), (640, 480)] {
            menu.layout(viewport);
            let panel = menu.graphics_panel();
            for (item, widget) in menu.graphics_screen().widgets() {
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
        }
    }

    #[test]
    fn the_graphics_rows_are_listed_cheapest_first_and_do_not_overlap() {
        // The order the player reads them in is the order they cost, so "the next one down"
        // is the row above.
        let menu = menu();
        let mut previous_bottom: Option<f32> = None;
        for quality in ShadowQuality::ALL {
            let rect = menu
                .graphics_screen()
                .widget(GraphicsItem::Shadows(quality))
                .expect("every level has a row")
                .rect();
            if let Some(bottom) = previous_bottom {
                assert!(
                    rect.y > bottom,
                    "{} overlaps the row above it",
                    quality.as_str()
                );
            }
            previous_bottom = Some(rect.bottom());
        }
        let back = menu
            .graphics_screen()
            .widget(GraphicsItem::Back)
            .expect("BACK exists")
            .rect();
        assert!(
            back.y > previous_bottom.unwrap_or(0.0),
            "BACK is not below the last level"
        );
    }

    #[test]
    fn the_nameplate_screen_stacks_its_sliders_inside_its_panel_without_overlapping() {
        let mut menu = menu();
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160)] {
            menu.layout(viewport);
            let panel = menu.nameplate_panel();
            let mut previous: Option<UiRect> = None;
            for setting in NameplateSetting::ALL {
                let rect = nameplate_track(&menu, setting);
                assert!(rect.y >= panel.y, "{setting:?} escaped the top");
                assert!(rect.bottom() <= panel.bottom(), "{setting:?} escaped below");
                assert!(rect.x >= panel.x, "{setting:?} escaped left");
                assert!(
                    rect.right() <= panel.right() + 0.01,
                    "{setting:?} escaped right"
                );
                if let Some(above) = previous {
                    assert!(
                        rect.y > above.bottom(),
                        "{setting:?} overlaps the one above"
                    );
                }
                previous = Some(rect);
            }
            let back = menu
                .nameplate_screen()
                .widget(NameplateItem::Back)
                .expect("BACK exists")
                .rect();
            assert!(back.y > previous.expect("a slider").bottom());
            assert!(back.bottom() <= panel.bottom());
        }
    }

    #[test]
    fn edit_hud_closes_the_menu_and_asks_the_application_for_edit_mode() {
        let mut menu = MenuSystem::new(GameSettings::default(), "TEST");
        menu.layout((1920, 1080));
        menu.open_pause();

        let actions = click_pause(&mut menu, PauseItem::EditHud);

        assert!(
            actions.contains(&MenuAction::EditHudRequested),
            "{actions:?}"
        );
        assert!(!menu.is_open(), "the menu stayed over the HUD being edited");
    }

    #[test]
    fn the_pause_menu_still_reads_top_to_bottom_with_the_new_button() {
        let mut menu = MenuSystem::new(GameSettings::default(), "TEST");
        menu.layout((1920, 1080));
        let rect = |item| {
            menu.pause_screen()
                .widget(item)
                .expect("the button exists")
                .rect()
        };
        let edit = rect(PauseItem::EditHud);
        let settings = rect(PauseItem::Settings);
        let quit = rect(PauseItem::Quit);
        let resume = rect(PauseItem::Resume);
        assert!(edit.y < settings.y);
        assert!(settings.y < quit.y);
        assert!(quit.y < resume.y);
        // EDIT HUD and SETTINGS are one group; RESUME is still separated from QUIT.
        assert!(settings.y - edit.bottom() < quit.y - settings.bottom());
        assert!(resume.y - quit.bottom() >= quit.y - settings.bottom());
        // And the panel still contains everything in it.
        let panel = menu.pause_panel();
        assert!(
            resume.bottom() <= panel.bottom() + 0.01,
            "{resume:?} {panel:?}"
        );
    }
}
