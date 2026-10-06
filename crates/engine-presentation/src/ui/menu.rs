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
    DisplaySettings, FrameRateLimit, GameSettings, KeyName, MinimapOrientation, NameplateSetting,
    Resolution, ShadowQuality, UI_SCALE_MAX, UI_SCALE_MIN, UI_SCALE_STEP, VOLUME_MAX, VOLUME_MIN,
    VOLUME_STEP,
};

use engine_game::hero::Hero;

use super::graphics_menu::{GraphicsLayout, GraphicsMenu, GraphicsOutcome};
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
    /// The graphics section of the settings: resolution, window mode and shadow quality.
    ///
    /// Its own screen for the reason the nameplates have one: three dropdowns, APPLY and the
    /// lines saying what each choice does are taller than a row on a list of sliders.
    Graphics,
    /// The map section of the settings: how the minimap turns, and the key that opens the
    /// dungeon map.
    ///
    /// The hero roster's shape — one tick per mode with a line saying what it does —
    /// because it is the same question: pick exactly one of a short list.
    Map,
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
    /// Opens the graphics section: resolution, window mode and shadows.
    Graphics,
    /// Opens the map section.
    Map,
    /// Opens the nameplate section.
    Nameplates,
    /// Returns to the pause menu.
    Back,
}

/// The map screen's items, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapItem {
    /// One of the two minimap modes. Ticked when it is the one in force.
    Orientation(MinimapOrientation),
    /// Replace the map's transitions with instant changes.
    ReducedMotion,
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
    /// Whether plates show the buffs and debuffs on their enemies.
    Auras,
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
    /// The player turned the buff and debuff icons under nameplates on or off. Apply it and
    /// persist it; a tick has no drag to settle.
    NameplateAurasCommitted(bool),
    /// The graphics screen opened. The application refreshes what the monitor offers.
    OpenedGraphics,
    /// The graphics screen closed back to the settings screen.
    ClosedGraphics,
    /// The player applied a shadow quality. Apply it and persist it.
    ///
    /// Emitted by APPLY on the graphics screen, only when the chosen level differs from the
    /// one in force — choosing from the dropdown alone changes nothing. Applying it is not
    /// free: the shadow atlas is recreated and the GPU is waited on.
    ShadowQualityCommitted(ShadowQuality),
    /// The player applied a frame-rate limit. Apply it and persist it.
    ///
    /// Emitted by APPLY like a shadow level, and as cheap to carry out as it is to undo: the
    /// loop is held to the new limit from the very next frame.
    FrameRateCommitted(FrameRateLimit),
    /// The player applied background energy saving on or off. Apply it and persist it.
    PreserveEnergyCommitted(bool),
    /// The map screen opened.
    OpenedMap,
    /// The map screen closed back to the settings screen.
    ClosedMap,
    /// The player chose how the minimap turns. Apply it and persist it.
    ///
    /// Committed immediately, like a shadow level, and applied on the very next frame: the
    /// minimap is on screen behind the menu, so the player sees the mode they picked.
    MinimapOrientationCommitted(MinimapOrientation),
    /// The player turned the map's reduced motion on or off. Apply it and persist it.
    ReducedMotionCommitted(bool),
    /// The player pressed APPLY. Apply these, then start the confirmation.
    ///
    /// Not saved: settings that turned the screen black must not be what the next launch
    /// starts with. The confirmation decides.
    DisplayApplyRequested(DisplaySettings),
    /// The player kept the display settings being confirmed. Save them.
    DisplayKeepRequested,
    /// The player wants the previous display settings back — by REVERT or by escape.
    DisplayRevertRequested,
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
    /// Width of the graphics panel: wider, so PRESERVE ENERGY IN BACKGROUND fits beside its
    /// box at the menu's glyph size.
    pub const GRAPHICS_PANEL_WIDTH: f32 = 520.0;
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
    map: UiScreen<MapItem>,
    /// The minimap mode the map screen's ticks describe.
    ///
    /// Held for the reason [`MenuSystem::hero`] is: a row of checkboxes is one choice, and
    /// the widget that was clicked is not the authority on what the choice now is — the
    /// application is.
    orientation: MinimapOrientation,
    /// The key that opens the dungeon map, for the map screen to print.
    map_key: KeyName,
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
    /// The graphics section and its confirmation dialog.
    graphics: GraphicsMenu,
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
        settings_screen.push(SettingsItem::Map, Button::new("MAP"));
        settings_screen.push(SettingsItem::Nameplates, Button::new("NAMEPLATES"));
        settings_screen.push(SettingsItem::Back, Button::new("BACK"));

        // The map section: one tick per minimap mode, built from `MinimapOrientation::ALL`.
        let mut map_screen = UiScreen::new();
        for mode in MinimapOrientation::ALL {
            map_screen.push(
                MapItem::Orientation(mode),
                Checkbox::new(mode.label()).checked(mode == settings.map.minimap_orientation),
            );
        }
        map_screen.push(
            MapItem::ReducedMotion,
            Checkbox::new("REDUCE MAP MOTION").checked(settings.map.reduced_motion),
        );
        map_screen.push(MapItem::Back, Button::new("BACK"));

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
        nameplate_screen.push(
            NameplateItem::Auras,
            Checkbox::new("SHOW BUFFS AND DEBUFFS").checked(settings.nameplates.auras),
        );
        nameplate_screen.push(NameplateItem::Back, Button::new("BACK"));

        let mut system = Self {
            screen: MenuScreen::None,
            title,
            pause,
            settings: settings_screen,
            heroes: hero_screen,
            nameplates: nameplate_screen,
            map: map_screen,
            orientation: settings.map.minimap_orientation,
            map_key: settings.map.toggle_key,
            hero: Hero::default(),
            actions: Vec::with_capacity(4),
            viewport: (0, 0),
            metrics: MenuMetrics::new(UiScale::new(settings.ui_scale)),
            graphics: GraphicsMenu::new(settings.display, settings.graphics.shadows),
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

    /// Says where QUIT goes: out of the game, or back to the editor the game was played from.
    ///
    /// Only the label. The menu still emits [`MenuAction::QuitRequested`] either way; which
    /// of the two a quit means is the application's to decide, since only it knows whether
    /// there is an editor to go back to.
    pub fn set_quit_to_editor(&mut self, to_editor: bool) {
        if let Some(Widget::Button(button)) = self.pause.widget_mut(PauseItem::Quit) {
            button.label = if to_editor { "QUIT TO EDITOR" } else { "QUIT" };
        }
    }

    /// The graphics section: its widgets, its choices and its dialog, for drawing.
    #[must_use]
    pub fn graphics_menu(&self) -> &GraphicsMenu {
        &self.graphics
    }

    /// The graphics section, to tell it what is in force, what the monitor offers, and how
    /// long the confirmation has left.
    pub fn graphics_menu_mut(&mut self) -> &mut GraphicsMenu {
        &mut self.graphics
    }

    /// Tells the graphics section what the monitor offers and what is in force, and lays it
    /// out again for the new number of rows.
    pub fn set_display_options(
        &mut self,
        options: Vec<Resolution>,
        desktop: Resolution,
        current: DisplaySettings,
    ) {
        self.graphics.set_options(options, desktop, current);
        self.layout_graphics(self.viewport);
    }

    /// The shadow level the graphics screen is showing as chosen.
    #[must_use]
    pub fn shadow_quality(&self) -> ShadowQuality {
        self.graphics.shadow_quality()
    }

    /// Shows `quality` as the level in force without emitting anything.
    ///
    /// What the application calls once it has actually applied a level. The menu never moves
    /// its own choice on a click: applying a shadow level recreates the atlas and can fail,
    /// and a screen that already showed the new level would be lying about what is running.
    pub fn set_shadow_quality(&mut self, quality: ShadowQuality) {
        self.graphics.set_shadow_quality(quality);
    }

    /// The map screen's widgets, for drawing.
    #[must_use]
    pub fn map_screen(&self) -> &UiScreen<MapItem> {
        &self.map
    }

    /// The minimap mode the map screen is showing as chosen.
    #[must_use]
    pub fn minimap_orientation(&self) -> MinimapOrientation {
        self.orientation
    }

    /// The key the map screen says opens the dungeon map.
    #[must_use]
    pub fn map_key(&self) -> &str {
        self.map_key.as_str()
    }

    /// Moves the tick to `mode` without emitting anything: what the application calls once
    /// it has applied a mode, exactly as for a shadow level.
    pub fn set_minimap_orientation(&mut self, mode: MinimapOrientation) {
        self.orientation = mode;
        for candidate in MinimapOrientation::ALL {
            if let Some(Widget::Checkbox(tick)) =
                self.map.widget_mut(MapItem::Orientation(candidate))
            {
                tick.set_checked(candidate == mode);
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
        if let Some(Widget::Checkbox(tick)) = self.nameplates.widget_mut(NameplateItem::Auras) {
            tick.set_checked(settings.nameplates.auras);
        }
        self.set_shadow_quality(settings.graphics.shadows);
        self.graphics.set_frame_rate(
            settings.graphics.max_fps,
            settings.graphics.preserve_energy_in_background,
        );
        self.set_minimap_orientation(settings.map.minimap_orientation);
        self.map_key = settings.map.toggle_key;
        if let Some(Widget::Checkbox(tick)) = self.map.widget_mut(MapItem::ReducedMotion) {
            tick.set_checked(settings.map.reduced_motion);
        }
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
        self.layout_map(viewport);
        self.layout_graphics(viewport);
    }

    /// The panel the graphics screen draws in.
    #[must_use]
    pub fn graphics_panel(&self) -> UiRect {
        let m = &self.metrics;
        let height = GraphicsMenu::content_height(
            m.padding,
            m.title_height,
            m.slider_label_height,
            m.button_height,
            m.button_gap,
            m.group_gap,
            m.hero_description_height,
        );
        let mut panel = self.centred_panel(self.viewport, height);
        // Wider than the others and still centred, never wider than the window.
        let width = m
            .scale
            .px(layout::GRAPHICS_PANEL_WIDTH)
            .min(self.viewport.0 as f32);
        panel.x = ((self.viewport.0 as f32 - width) * 0.5).max(0.0);
        panel.width = width;
        panel
    }

    fn layout_graphics(&mut self, viewport: (u32, u32)) {
        let panel = self.graphics_panel();
        let m = self.metrics;
        self.graphics.layout(&GraphicsLayout {
            panel,
            padding: m.padding,
            title_height: m.title_height,
            label_height: m.slider_label_height,
            row_height: m.button_height,
            gap: m.button_gap,
            group_gap: m.group_gap,
            line_height: m.hero_description_height,
            viewport,
            scale: m.scale,
        });
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
            + self.metrics.button_height * 5.0
            + self.metrics.button_gap * 4.0;
        self.centred_panel(self.viewport, content + self.metrics.padding * 2.0)
    }

    /// The panel the map screen draws in: a row per minimap mode with its description, the
    /// map key's line, and a separated BACK.
    #[must_use]
    pub fn map_panel(&self) -> UiRect {
        let row = self.metrics.button_height
            + self.metrics.hero_description_height
            + self.metrics.button_gap;
        let content = self.metrics.title_height
            + row * MinimapOrientation::ALL.len() as f32
            + self.metrics.hero_description_height * 2.0
            + self.metrics.button_height
            + self.metrics.button_gap
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
            + self.metrics.button_height
            + self.metrics.button_gap
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
            SettingsItem::Map,
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

    fn layout_map(&mut self, viewport: (u32, u32)) {
        self.viewport = viewport;
        let panel = self.map_panel();
        let x = panel.x + self.metrics.padding;
        let width = panel.width - self.metrics.padding * 2.0;
        let mut y = panel.y + self.metrics.padding + self.metrics.title_height;
        for mode in MinimapOrientation::ALL {
            self.map.set_rect(
                MapItem::Orientation(mode),
                UiRect::new(x, y, width, self.metrics.button_height),
            );
            y += self.metrics.button_height
                + self.metrics.hero_description_height
                + self.metrics.button_gap;
        }
        self.map.set_rect(
            MapItem::ReducedMotion,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;
        // Two lines for the key, reserved here and printed by the painter.
        y += self.metrics.hero_description_height * 2.0;
        self.map.set_rect(
            MapItem::Back,
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
            NameplateItem::Auras,
            UiRect::new(x, y, width, self.metrics.button_height),
        );
        y += self.metrics.button_height + self.metrics.button_gap;

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
            MenuScreen::Map => self.update_map(input),
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
        let mut open_map = false;
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
                (SettingsItem::Map, WidgetEvent::Activated) => open_map = true,
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
                            WidgetEvent::Activated
                            | WidgetEvent::Toggled(_)
                            | WidgetEvent::Chosen(_) => {}
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
        if open_map {
            self.screen = MenuScreen::Map;
            self.settings.reset();
            self.map.reset();
            self.actions.push(MenuAction::OpenedMap);
            return;
        }
        if open_graphics {
            self.screen = MenuScreen::Graphics;
            self.settings.reset();
            self.graphics.discard_changes();
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
        let mut auras = None;
        let mut changes: [Option<(NameplateSetting, f32, bool)>; NameplateSetting::ALL.len() * 2] =
            [None; NameplateSetting::ALL.len() * 2];
        let mut count = 0;
        for (item, event) in self.nameplates.update(input) {
            match (item, event) {
                (NameplateItem::Back, WidgetEvent::Activated) => back = true,
                (NameplateItem::Auras, WidgetEvent::Toggled(checked)) => auras = Some(*checked),
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
        if let Some(shown) = auras {
            self.actions
                .push(MenuAction::NameplateAurasCommitted(shown));
        }
        if back {
            self.back_to_settings();
        }
    }

    /// The map section: one tick per minimap mode, and a way back. The tick goes back to
    /// what the application says until it says otherwise.
    fn update_map(&mut self, input: &UiInput) {
        if input.cancel {
            self.back_to_settings_from_map();
            return;
        }
        let mut back = false;
        let mut chosen = None;
        let mut motion = None;
        for (item, event) in self.map.update(input) {
            match (item, event) {
                (MapItem::Back, WidgetEvent::Activated) => back = true,
                (MapItem::ReducedMotion, WidgetEvent::Toggled(checked)) => motion = Some(*checked),
                (MapItem::Orientation(mode), WidgetEvent::Toggled(_) | WidgetEvent::Activated) => {
                    chosen = Some(*mode);
                }
                _ => {}
            }
        }
        let current = self.orientation;
        self.set_minimap_orientation(current);
        if let Some(mode) = chosen {
            self.actions
                .push(MenuAction::MinimapOrientationCommitted(mode));
        }
        if let Some(reduced) = motion {
            self.actions
                .push(MenuAction::ReducedMotionCommitted(reduced));
        }
        if back {
            self.back_to_settings_from_map();
        }
    }

    /// Escape and BACK from the map section return to the settings screen.
    fn back_to_settings_from_map(&mut self) {
        self.screen = MenuScreen::Settings;
        self.map.reset();
        self.settings.reset();
        self.actions.push(MenuAction::ClosedMap);
    }

    /// The graphics section: four dropdowns, a box, APPLY and BACK, and the dialog that follows
    /// APPLY. Everything it decides leaves as an action.
    ///
    /// APPLY does **not** close the menu — the point is to look at the result and try
    /// something else if it is not right. Escape and BACK return to the settings screen, not
    /// to the game — the rule every section of the settings follows.
    fn update_graphics(&mut self, input: &UiInput) {
        match self.graphics.update(input) {
            Some(GraphicsOutcome::Apply {
                display,
                shadows,
                max_fps,
                preserve_energy,
            }) => {
                // The shadows and the frame rate first: the display change starts a
                // confirmation, and a level applied after it would land in the middle of the
                // dialog.
                if let Some(quality) = shadows {
                    self.actions
                        .push(MenuAction::ShadowQualityCommitted(quality));
                }
                if let Some(limit) = max_fps {
                    self.actions.push(MenuAction::FrameRateCommitted(limit));
                }
                if let Some(preserve) = preserve_energy {
                    self.actions
                        .push(MenuAction::PreserveEnergyCommitted(preserve));
                }
                if let Some(settings) = display {
                    self.actions
                        .push(MenuAction::DisplayApplyRequested(settings));
                }
            }
            Some(GraphicsOutcome::Keep) => self.actions.push(MenuAction::DisplayKeepRequested),
            Some(GraphicsOutcome::Revert) => self.actions.push(MenuAction::DisplayRevertRequested),
            Some(GraphicsOutcome::Back) => {
                self.graphics.discard_changes();
                self.screen = MenuScreen::Settings;
                self.settings.reset();
                self.actions.push(MenuAction::ClosedGraphics);
            }
            None => {}
        }
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
    use crate::ui::graphics_menu::GraphicsItem;

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
    fn quit_says_where_it_goes_but_asks_the_same_thing_either_way() {
        let mut menu = menu();
        let label = |menu: &MenuSystem| match menu.pause_screen().widget(PauseItem::Quit) {
            Some(Widget::Button(button)) => button.label,
            _ => panic!("QUIT is a button"),
        };
        assert_eq!(label(&menu), "QUIT");
        menu.set_quit_to_editor(true);
        assert_eq!(label(&menu), "QUIT TO EDITOR");
        menu.open_pause();
        assert_eq!(
            click_pause(&mut menu, PauseItem::Quit),
            [MenuAction::QuitRequested]
        );
        menu.set_quit_to_editor(false);
        assert_eq!(label(&menu), "QUIT");
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
    fn the_graphics_screen_applies_the_display_through_an_action() {
        use engine_settings::{DisplaySettings, Resolution, WindowMode};
        let mut menu = menu();
        menu.open_pause();
        click_pause(&mut menu, PauseItem::Settings);
        let actions = click_settings(&mut menu, SettingsItem::Graphics);
        assert!(actions.contains(&MenuAction::OpenedGraphics), "{actions:?}");
        assert_eq!(menu.screen(), MenuScreen::Graphics);
        menu.set_display_options(
            vec![Resolution::new(2560, 1440), Resolution::new(1920, 1080)],
            Resolution::new(2560, 1440),
            DisplaySettings::default(),
        );
        // Every widget of the screen is inside its panel, at every size the menu is tested at.
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160)] {
            menu.layout(viewport);
            let panel = menu.graphics_panel();
            for (_, widget) in menu.graphics_menu().screen().widgets() {
                let rect = widget.rect();
                assert!(
                    rect.x >= panel.x && rect.right() <= panel.right() + 0.5,
                    "{viewport:?}: {rect:?} outside {panel:?}"
                );
                assert!(
                    rect.bottom() <= panel.bottom() + 0.5,
                    "{viewport:?}: {rect:?}"
                );
            }
        }
        menu.layout((1920, 1080));
        // Choose fullscreen and apply.
        let mode = menu
            .graphics_menu()
            .screen()
            .widget(GraphicsItem::WindowMode)
            .expect("mode")
            .rect();
        let at = |rect: UiRect| (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        let (x, y) = at(mode);
        menu.update(&press(x, y));
        let Some(crate::ui::widget::Widget::Dropdown(open)) = menu
            .graphics_menu()
            .screen()
            .widget(GraphicsItem::WindowMode)
            .copied()
        else {
            panic!("a dropdown");
        };
        let (x, y) = at(open.option_rect(2));
        menu.update(&press(x, y));
        menu.update(&release(x, y));
        let apply = menu
            .graphics_menu()
            .screen()
            .widget(GraphicsItem::Apply)
            .expect("apply")
            .rect();
        let (x, y) = at(apply);
        menu.update(&press(x, y));
        let actions = menu.update(&release(x, y)).to_vec();
        assert!(
            actions.contains(&MenuAction::DisplayApplyRequested(DisplaySettings::new(
                Resolution::new(1920, 1080),
                WindowMode::Fullscreen
            ))),
            "{actions:?}"
        );
        // Escape while the dialog is up reverts rather than leaving the screen.
        menu.graphics_menu_mut().begin_confirmation(15);
        let actions = menu.update(&escape()).to_vec();
        assert_eq!(actions, vec![MenuAction::DisplayRevertRequested]);
        assert_eq!(menu.screen(), MenuScreen::Graphics);
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
        let Some(Widget::Checkbox(checkbox)) =
            menu.settings_screen().widget(SettingsItem::BindingScope)
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
    fn the_nameplate_page_turns_the_buff_and_debuff_icons_off_and_on() {
        let mut menu = menu();
        open_nameplates(&mut menu);
        let rect = menu
            .nameplate_screen()
            .widget(NameplateItem::Auras)
            .expect("the page has the tick")
            .rect();
        let panel = menu.nameplate_panel();
        assert!(
            rect.y >= panel.y && rect.bottom() <= panel.bottom(),
            "{rect:?} {panel:?}"
        );
        let (x, y) = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);

        // On by default, so the first click turns it off, and the second back on.
        menu.update(&press(x, y));
        let off = menu.update(&release(x, y)).to_vec();
        assert!(
            matches!(off.as_slice(), [MenuAction::NameplateAurasCommitted(false)]),
            "{off:?}"
        );
        menu.update(&press(x, y));
        let on = menu.update(&release(x, y)).to_vec();
        assert!(
            matches!(on.as_slice(), [MenuAction::NameplateAurasCommitted(true)]),
            "{on:?}"
        );
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
            .graphics_menu()
            .screen()
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

    /// Opens the shadow dropdown and picks `quality` from its list.
    fn choose_shadows(menu: &mut MenuSystem, quality: ShadowQuality) -> Vec<MenuAction> {
        let (x, y) = centre_of_graphics(menu, GraphicsItem::Shadows);
        menu.update(&press(x, y));
        menu.update(&release(x, y));
        let Some(Widget::Dropdown(open)) = menu
            .graphics_menu()
            .screen()
            .widget(GraphicsItem::Shadows)
            .copied()
        else {
            panic!("the shadows are a dropdown");
        };
        assert!(open.is_open(), "the click did not open the list");
        let index = ShadowQuality::ALL
            .iter()
            .position(|candidate| *candidate == quality)
            .expect("every level is listed");
        let row = open.option_rect(index);
        let (x, y) = (row.x + row.width * 0.5, row.y + row.height * 0.5);
        // A list chooses on the press, so both frames are reported.
        let mut actions = menu.update(&press(x, y)).to_vec();
        actions.extend_from_slice(menu.update(&release(x, y)));
        actions
    }

    fn open_graphics(menu: &mut MenuSystem) {
        menu.open_pause();
        click_pause(menu, PauseItem::Settings);
        click_settings(menu, SettingsItem::Graphics);
    }

    fn settings_with_shadows(shadows: ShadowQuality) -> GameSettings {
        GameSettings {
            graphics: engine_settings::GraphicsSettings {
                shadows,
                ..engine_settings::GraphicsSettings::default()
            },
            ..GameSettings::default()
        }
    }

    /// The level the shadow dropdown shows.
    fn shown_shadows(menu: &MenuSystem) -> ShadowQuality {
        match menu.graphics_menu().screen().widget(GraphicsItem::Shadows) {
            Some(Widget::Dropdown(dropdown)) => ShadowQuality::ALL[dropdown.selected],
            _ => panic!("the shadows are a dropdown"),
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
    fn the_graphics_screen_shows_the_shadow_level_in_force() {
        let mut menu = MenuSystem::new(settings_with_shadows(ShadowQuality::Medium), "0.1.0");
        menu.layout((1280, 720));
        assert_eq!(shown_shadows(&menu), ShadowQuality::Medium);
        assert_eq!(menu.shadow_quality(), ShadowQuality::Medium);
    }

    #[test]
    fn choosing_a_level_changes_nothing_until_apply() {
        let mut menu = menu();
        open_graphics(&mut menu);
        assert_eq!(menu.shadow_quality(), ShadowQuality::High);

        let actions = choose_shadows(&mut menu, ShadowQuality::Ultra);
        assert!(
            actions.is_empty(),
            "choosing applied something: {actions:?}"
        );
        assert_eq!(shown_shadows(&menu), ShadowQuality::Ultra);
        assert_eq!(menu.shadow_quality(), ShadowQuality::High);

        // APPLY reports it, and only it: the display did not change. The menu stays open.
        let actions = click_graphics(&mut menu, GraphicsItem::Apply);
        assert_eq!(
            actions,
            [MenuAction::ShadowQualityCommitted(ShadowQuality::Ultra)]
        );
        assert_eq!(menu.screen(), MenuScreen::Graphics);
        assert_eq!(
            menu.shadow_quality(),
            ShadowQuality::High,
            "the menu moved its own level before the application applied anything"
        );

        // Once the application has applied it, the dropdown shows it and APPLY has nothing
        // left to do.
        menu.set_shadow_quality(ShadowQuality::Ultra);
        assert_eq!(menu.shadow_quality(), ShadowQuality::Ultra);
        assert_eq!(shown_shadows(&menu), ShadowQuality::Ultra);
        assert!(click_graphics(&mut menu, GraphicsItem::Apply).is_empty());
    }

    #[test]
    fn back_forgets_an_unapplied_level() {
        let mut menu = menu();
        open_graphics(&mut menu);
        choose_shadows(&mut menu, ShadowQuality::Low);
        let actions = click_graphics(&mut menu, GraphicsItem::Back);
        assert_eq!(actions, [MenuAction::ClosedGraphics]);
        click_settings(&mut menu, SettingsItem::Graphics);
        assert_eq!(shown_shadows(&menu), ShadowQuality::High);
    }

    #[test]
    fn one_apply_carries_the_shadows_and_the_display() {
        use engine_settings::{DisplaySettings, WindowMode};
        let mut menu = menu();
        open_graphics(&mut menu);
        choose_shadows(&mut menu, ShadowQuality::Off);
        let Some(Widget::Dropdown(mode)) = menu
            .graphics_menu()
            .screen()
            .widget(GraphicsItem::WindowMode)
            .copied()
        else {
            panic!("a dropdown");
        };
        let at = |rect: UiRect| (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        let (x, y) = at(mode.rect);
        menu.update(&press(x, y));
        menu.update(&release(x, y));
        let Some(Widget::Dropdown(open)) = menu
            .graphics_menu()
            .screen()
            .widget(GraphicsItem::WindowMode)
            .copied()
        else {
            panic!("a dropdown");
        };
        let (x, y) = at(open.option_rect(1));
        menu.update(&press(x, y));
        menu.update(&release(x, y));
        let actions = click_graphics(&mut menu, GraphicsItem::Apply);
        assert_eq!(
            actions,
            [
                MenuAction::ShadowQualityCommitted(ShadowQuality::Off),
                MenuAction::DisplayApplyRequested(DisplaySettings {
                    window_mode: WindowMode::BorderlessFullscreen,
                    ..DisplaySettings::default()
                }),
            ]
        );
    }

    #[test]
    fn loaded_settings_show_the_level_they_name() {
        let mut menu = menu();
        menu.apply_settings(settings_with_shadows(ShadowQuality::Off));
        assert_eq!(menu.shadow_quality(), ShadowQuality::Off);
        assert_eq!(shown_shadows(&menu), ShadowQuality::Off);
    }

    #[test]
    fn every_graphics_widget_stays_inside_its_panel_without_overlapping() {
        let mut menu = menu();
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160)] {
            menu.layout(viewport);
            let panel = menu.graphics_panel();
            let rects: Vec<_> = menu
                .graphics_menu()
                .screen()
                .widgets()
                .map(|(item, widget)| (item, widget.rect()))
                .collect();
            for (i, (item, rect)) in rects.iter().enumerate() {
                for (other, them) in &rects[i + 1..] {
                    let apart = rect.right() <= them.x
                        || them.right() <= rect.x
                        || rect.bottom() <= them.y
                        || them.bottom() <= rect.y;
                    assert!(apart, "{viewport:?}: {item:?} overlaps {other:?}");
                }
            }
            for (item, widget) in menu.graphics_menu().screen().widgets() {
                let rect = widget.rect();
                assert!(rect.x >= panel.x, "{viewport:?}: {item:?} escaped left");
                assert!(
                    rect.right() <= panel.right() + 0.01,
                    "{viewport:?}: {item:?} escaped right"
                );
                assert!(
                    rect.y >= panel.y && rect.bottom() <= panel.bottom() + 0.01,
                    "{viewport:?}: {item:?} escaped the panel"
                );
            }
        }
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
