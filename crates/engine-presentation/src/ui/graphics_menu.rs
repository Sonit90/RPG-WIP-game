//! The GRAPHICS screen: resolution, window mode and shadow quality, all applied together by
//! APPLY.
//!
//! ```text
//! ┌──────────── GRAPHICS ───────────┐
//! │ RESOLUTION                      │      ┌──────────────────────────────────┐
//! │ [ 3840 x 2160               ▼ ] │      │  KEEP THESE DISPLAY SETTINGS?    │
//! │ WINDOW MODE                     │      │  REVERTING IN 15 SECONDS...      │
//! │ [ BORDERLESS FULLSCREEN     ▼ ] │      │ [ KEEP CHANGES ]  [ REVERT ]     │
//! │ COVERS THE MONITOR AT ITS ...   │      └──────────────────────────────────┘
//! │ NOW: 1920 x 1080, WINDOWED      │          after APPLY, over everything,
//! │ SHADOW QUALITY                  │          when the display changed
//! │ [ HIGH                      ▼ ] │
//! │ SHARPER SHADOWS, AND ...        │
//! │ [ APPLY      ]  [ BACK       ]  │
//! └─────────────────────────────────┘
//! ```
//!
//! Choosing from a dropdown changes nothing but the choice; APPLY carries out every choice at
//! once, and BACK forgets them. A display change can turn the screen black, so it is then
//! confirmed by the dialog; a shadow level cannot strand anybody, so it is simply applied
//! and saved.
//!
//! The menu never decides what is in force — the application tells it
//! ([`GraphicsMenu::set_current`], [`GraphicsMenu::set_shadow_quality`]) after applying,
//! reverting, an Alt+Enter toggle or the shadow key, so the screen always shows the truth.
//! The resolution list is the monitor's, given by the application
//! ([`GraphicsMenu::set_options`]); this module only turns it into rows and remembers which
//! one is chosen.

use engine_render_data::ui::{UiRect, UiScale};
use engine_settings::{DisplaySettings, Resolution, ShadowQuality, WindowMode};

use super::input::UiInput;
use super::screen::UiScreen;
use super::widget::{Button, Dropdown, Widget, WidgetEvent};

/// The GRAPHICS screen's items, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsItem {
    /// The resolution dropdown.
    Resolution,
    /// The window mode dropdown.
    WindowMode,
    /// The shadow quality dropdown.
    Shadows,
    /// Applies every choice. At the bottom, beside BACK.
    Apply,
    /// Returns to the settings screen, dropping unapplied choices.
    Back,
}

/// The three dropdowns, in the order they are laid out.
const DROPDOWNS: [GraphicsItem; 3] = [
    GraphicsItem::Resolution,
    GraphicsItem::WindowMode,
    GraphicsItem::Shadows,
];

/// What a shadow level's row says.
///
/// Upper case like every other label in the menu, and the same word the file uses so a player
/// reading `settings.cfg` sees what they chose.
#[must_use]
pub fn shadow_label(quality: ShadowQuality) -> &'static str {
    match quality {
        ShadowQuality::Off => "OFF",
        ShadowQuality::Low => "LOW",
        ShadowQuality::Medium => "MEDIUM",
        ShadowQuality::High => "HIGH",
        ShadowQuality::Ultra => "ULTRA",
    }
}

/// The confirmation dialog's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmItem {
    /// Keep the new settings.
    Keep,
    /// Go back to the old ones.
    Revert,
}

/// What the GRAPHICS screen asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsOutcome {
    /// APPLY: what changed. The display, when present, is applied and then confirmed; the
    /// shadow level, when present, is applied and saved.
    Apply {
        /// The display settings to try, if they differ from what is in force.
        display: Option<DisplaySettings>,
        /// The shadow level to use, if it differs from the one in force.
        shadows: Option<ShadowQuality>,
    },
    /// The player kept the settings being confirmed.
    Keep,
    /// The player wants the previous settings back.
    Revert,
    /// Leave the screen.
    Back,
}

/// Measurements the screen is laid out with, in physical pixels: the menu's own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GraphicsLayout {
    /// The panel the screen draws in.
    pub panel: UiRect,
    /// Padding inside it.
    pub padding: f32,
    /// The title's height.
    pub title_height: f32,
    /// A label's height above its dropdown.
    pub label_height: f32,
    /// A dropdown's or a button's height.
    pub row_height: f32,
    /// Gap between rows in a group.
    pub gap: f32,
    /// Gap between groups.
    pub group_gap: f32,
    /// A description line's height.
    pub line_height: f32,
    /// The viewport.
    pub viewport: (u32, u32),
    /// The interface scale.
    pub scale: UiScale,
}

/// The GRAPHICS screen and its confirmation dialog.
#[derive(Debug, Clone)]
pub struct GraphicsMenu {
    screen: UiScreen<GraphicsItem>,
    confirm: UiScreen<ConfirmItem>,
    /// What the monitor offers, largest first.
    options: Vec<Resolution>,
    /// Each option as the dropdown prints it.
    labels: Vec<String>,
    /// The desktop's own resolution: what borderless uses.
    desktop: Resolution,
    /// What is in force.
    current: DisplaySettings,
    /// What the dropdowns say.
    pending: DisplaySettings,
    /// Seconds left on the confirmation dialog, while it is up.
    confirming: Option<u32>,
    /// Where the dialog is.
    dialog: UiRect,
    /// The shadow level in force.
    shadows: ShadowQuality,
    /// What the shadow dropdown says.
    pending_shadows: ShadowQuality,
}

impl GraphicsMenu {
    /// The screen with `current` and `shadows` in force and, until the monitor is asked,
    /// only the current resolution on offer.
    #[must_use]
    pub fn new(current: DisplaySettings, shadows: ShadowQuality) -> Self {
        let mut screen = UiScreen::new();
        screen.push(GraphicsItem::Resolution, Dropdown::new("RESOLUTION", 1, 0));
        screen.push(
            GraphicsItem::WindowMode,
            Dropdown::new("WINDOW MODE", WindowMode::ALL.len(), 0),
        );
        screen.push(
            GraphicsItem::Shadows,
            Dropdown::new(
                "SHADOW QUALITY",
                ShadowQuality::ALL.len(),
                shadow_index(shadows),
            ),
        );
        screen.push(GraphicsItem::Apply, Button::new("APPLY"));
        screen.push(GraphicsItem::Back, Button::new("BACK"));
        let mut confirm = UiScreen::new();
        confirm.push(ConfirmItem::Keep, Button::new("KEEP CHANGES"));
        confirm.push(ConfirmItem::Revert, Button::new("REVERT"));
        let mut menu = Self {
            screen,
            confirm,
            options: Vec::new(),
            labels: Vec::new(),
            desktop: current.resolution(),
            current,
            pending: current,
            confirming: None,
            dialog: UiRect::new(0.0, 0.0, 0.0, 0.0),
            shadows,
            pending_shadows: shadows,
        };
        menu.set_options(vec![current.resolution()], current.resolution(), current);
        menu
    }

    /// The monitor's resolutions (largest first), its desktop resolution, and what is in
    /// force. The choices go back to what is in force.
    pub fn set_options(
        &mut self,
        options: Vec<Resolution>,
        desktop: Resolution,
        current: DisplaySettings,
    ) {
        let mut options = options;
        if options.is_empty() {
            options.push(current.resolution());
        }
        self.labels = options
            .iter()
            .map(|resolution| resolution.label())
            .collect();
        self.options = options;
        self.desktop = desktop;
        self.set_current(current);
    }

    /// What is in force now — after an apply, a revert or an Alt+Enter. The choices follow.
    pub fn set_current(&mut self, current: DisplaySettings) {
        self.current = current;
        self.pending = current;
        self.sync_widgets();
    }

    /// What is in force.
    #[must_use]
    pub fn current(&self) -> DisplaySettings {
        self.current
    }

    /// The shadow level in force.
    #[must_use]
    pub fn shadow_quality(&self) -> ShadowQuality {
        self.shadows
    }

    /// The shadow level in force now — after a choice was applied, refused, or changed by
    /// the shadow key. The dropdown follows.
    pub fn set_shadow_quality(&mut self, quality: ShadowQuality) {
        self.shadows = quality;
        self.pending_shadows = quality;
        self.sync_widgets();
    }

    /// What the display dropdowns say.
    #[must_use]
    pub fn pending(&self) -> DisplaySettings {
        self.pending
    }

    /// What the shadow dropdown says.
    #[must_use]
    pub fn pending_shadows(&self) -> ShadowQuality {
        self.pending_shadows
    }

    /// Whether APPLY would change anything.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.has_display_changes() || self.pending_shadows != self.shadows
    }

    /// Whether APPLY would change the display.
    #[must_use]
    pub fn has_display_changes(&self) -> bool {
        if self.pending.window_mode != self.current.window_mode {
            return true;
        }
        // Borderless uses the desktop's resolution whatever the list says.
        self.pending.window_mode.uses_resolution()
            && self.pending.resolution() != self.current.resolution()
    }

    /// The resolutions on offer, largest first.
    #[must_use]
    pub fn options(&self) -> &[Resolution] {
        &self.options
    }

    /// The resolution rows as printed.
    #[must_use]
    pub fn resolution_labels(&self) -> &[String] {
        &self.labels
    }

    /// The desktop's resolution: what borderless fullscreen uses.
    #[must_use]
    pub fn desktop(&self) -> Resolution {
        self.desktop
    }

    /// The screen's widgets.
    #[must_use]
    pub fn screen(&self) -> &UiScreen<GraphicsItem> {
        &self.screen
    }

    /// The dialog's buttons.
    #[must_use]
    pub fn confirm_screen(&self) -> &UiScreen<ConfirmItem> {
        &self.confirm
    }

    /// Where the dialog is.
    #[must_use]
    pub fn dialog(&self) -> UiRect {
        self.dialog
    }

    /// Seconds left on the dialog, while it is up.
    #[must_use]
    pub fn confirming(&self) -> Option<u32> {
        self.confirming
    }

    /// Puts the dialog up with `seconds` on its clock.
    pub fn begin_confirmation(&mut self, seconds: u32) {
        self.confirming = Some(seconds);
        self.close_dropdowns();
        self.confirm.reset();
    }

    /// Updates the dialog's clock.
    pub fn set_confirmation_seconds(&mut self, seconds: u32) {
        if self.confirming.is_some() {
            self.confirming = Some(seconds);
        }
    }

    /// Takes the dialog down.
    pub fn end_confirmation(&mut self) {
        self.confirming = None;
        self.confirm.reset();
    }

    /// Forgets unapplied choices and any open list: the screen is being left.
    pub fn discard_changes(&mut self) {
        self.pending = self.current;
        self.pending_shadows = self.shadows;
        self.sync_widgets();
        self.screen.reset();
    }

    fn close_dropdowns(&mut self) {
        for item in DROPDOWNS {
            if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(item) {
                dropdown.close();
            }
        }
    }

    /// The dropdowns and APPLY made to show the pending choices.
    fn sync_widgets(&mut self) {
        let selected = self
            .options
            .iter()
            .position(|resolution| *resolution == self.pending.resolution())
            .or_else(|| {
                engine_settings::closest_resolution(self.pending.resolution(), &self.options)
                    .and_then(|near| self.options.iter().position(|option| *option == near))
            })
            .unwrap_or(0);
        if let Some(found) = self.options.get(selected) {
            self.pending.resolution_width = found.width;
            self.pending.resolution_height = found.height;
        }
        let uses_resolution = self.pending.window_mode.uses_resolution();
        if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(GraphicsItem::Resolution) {
            if dropdown.len != self.options.len() || dropdown.selected != selected {
                dropdown.set_options(self.options.len(), selected);
            }
            dropdown.state.enabled = uses_resolution;
        }
        let mode = WindowMode::ALL
            .iter()
            .position(|mode| *mode == self.pending.window_mode)
            .unwrap_or(0);
        if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(GraphicsItem::WindowMode) {
            dropdown.selected = mode;
        }
        let shadows = shadow_index(self.pending_shadows);
        if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(GraphicsItem::Shadows) {
            dropdown.selected = shadows;
        }
        let changes = self.has_changes();
        if let Some(Widget::Button(button)) = self.screen.widget_mut(GraphicsItem::Apply) {
            button.state.enabled = changes;
        }
    }

    /// Places everything inside `layout.panel`.
    pub fn layout(&mut self, layout: &GraphicsLayout) {
        let x = layout.panel.x + layout.padding;
        let width = layout.panel.width - layout.padding * 2.0;
        let mut y = layout.panel.y + layout.padding + layout.title_height;
        for item in [GraphicsItem::Resolution, GraphicsItem::WindowMode] {
            y += layout.label_height;
            self.screen
                .set_rect(item, UiRect::new(x, y, width, layout.row_height));
            y += layout.row_height + layout.gap;
        }
        // The mode's description and the line saying what is in force.
        y += layout.line_height * 2.0 + layout.group_gap - layout.gap;
        // The shadows, a group of their own: a label, the box, the level's description.
        y += layout.label_height;
        self.screen.set_rect(
            GraphicsItem::Shadows,
            UiRect::new(x, y, width, layout.row_height),
        );
        y += layout.row_height + layout.line_height + layout.group_gap;
        // APPLY and BACK side by side at the bottom: the two ways off the screen.
        let half = (width - layout.gap) * 0.5;
        self.screen.set_rect(
            GraphicsItem::Apply,
            UiRect::new(x, y, half, layout.row_height),
        );
        self.screen.set_rect(
            GraphicsItem::Back,
            UiRect::new(
                x + half + layout.gap,
                y,
                width - half - layout.gap,
                layout.row_height,
            ),
        );
        let screen_height = layout.viewport.1 as f32;
        let margin = layout.scale.px(8.0);
        for item in DROPDOWNS {
            if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(item) {
                dropdown.fit_list(screen_height, margin);
            }
        }

        // The dialog: centred on the screen, two buttons side by side.
        let (vw, vh) = (layout.viewport.0 as f32, layout.viewport.1 as f32);
        let dialog_width = layout.scale.px(560.0).min(vw - margin * 2.0);
        let dialog_height = layout.title_height
            + layout.line_height * 2.0
            + layout.row_height
            + layout.padding * 2.0
            + layout.group_gap;
        self.dialog = UiRect::new(
            (vw - dialog_width) * 0.5,
            (vh - dialog_height) * 0.5,
            dialog_width,
            dialog_height,
        );
        let inner = dialog_width - layout.padding * 2.0;
        let half = (inner - layout.gap) * 0.5;
        let button_y = self.dialog.bottom() - layout.padding - layout.row_height;
        self.confirm.set_rect(
            ConfirmItem::Keep,
            UiRect::new(
                self.dialog.x + layout.padding,
                button_y,
                half,
                layout.row_height,
            ),
        );
        self.confirm.set_rect(
            ConfirmItem::Revert,
            UiRect::new(
                self.dialog.x + layout.padding + half + layout.gap,
                button_y,
                half,
                layout.row_height,
            ),
        );
    }

    /// The panel height this screen needs, for the menu to centre it.
    #[must_use]
    pub fn content_height(
        padding: f32,
        title_height: f32,
        label_height: f32,
        row_height: f32,
        gap: f32,
        group_gap: f32,
        line_height: f32,
    ) -> f32 {
        padding * 2.0
            + title_height
            // RESOLUTION and WINDOW MODE, then the two lines under them.
            + (label_height + row_height + gap) * 2.0
            + line_height * 2.0
            + group_gap
            - gap
            // SHADOW QUALITY and its line.
            + label_height
            + row_height
            + line_height
            + group_gap
            // APPLY and BACK.
            + row_height
    }

    /// Whether a dropdown's list is showing.
    #[must_use]
    pub fn has_open_list(&self) -> bool {
        self.screen
            .widgets()
            .any(|(_, widget)| matches!(widget, Widget::Dropdown(dropdown) if dropdown.is_open()))
    }

    /// Answers one frame of input.
    ///
    /// While the dialog is up only its buttons answer, and escape means REVERT. While a list
    /// is open only that dropdown answers, so a click on its rows never also lands on what
    /// is under them. Otherwise escape means BACK.
    pub fn update(&mut self, input: &UiInput) -> Option<GraphicsOutcome> {
        if self.confirming.is_some() {
            if input.cancel {
                return Some(GraphicsOutcome::Revert);
            }
            let mut outcome = None;
            for (item, event) in self.confirm.update(input) {
                if *event == WidgetEvent::Activated {
                    outcome = Some(match item {
                        ConfirmItem::Keep => GraphicsOutcome::Keep,
                        ConfirmItem::Revert => GraphicsOutcome::Revert,
                    });
                }
            }
            return outcome;
        }

        let open = DROPDOWNS
            .into_iter()
            .find(|item| {
                matches!(self.screen.widget(*item), Some(Widget::Dropdown(dropdown)) if dropdown.is_open())
            });
        let mut chosen: Option<(GraphicsItem, usize)> = None;
        if let Some(item) = open {
            if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(item)
                && let Some(WidgetEvent::Chosen(index)) = dropdown.update(input)
            {
                chosen = Some((item, index));
            }
        } else {
            if input.cancel {
                return Some(GraphicsOutcome::Back);
            }
            let mut apply = false;
            let mut back = false;
            for (item, event) in self.screen.update(input) {
                match (item, event) {
                    (GraphicsItem::Apply, WidgetEvent::Activated) => apply = true,
                    (GraphicsItem::Back, WidgetEvent::Activated) => back = true,
                    (item, WidgetEvent::Chosen(index)) => chosen = Some((*item, *index)),
                    _ => {}
                }
            }
            if back {
                return Some(GraphicsOutcome::Back);
            }
            if apply && self.has_changes() {
                return Some(GraphicsOutcome::Apply {
                    display: self.has_display_changes().then_some(self.pending),
                    shadows: (self.pending_shadows != self.shadows).then_some(self.pending_shadows),
                });
            }
        }
        if let Some((item, index)) = chosen {
            match item {
                GraphicsItem::Resolution => {
                    if let Some(resolution) = self.options.get(index) {
                        self.pending.resolution_width = resolution.width;
                        self.pending.resolution_height = resolution.height;
                    }
                }
                GraphicsItem::Shadows => {
                    if let Some(quality) = ShadowQuality::ALL.get(index) {
                        self.pending_shadows = *quality;
                    }
                }
                GraphicsItem::WindowMode => {
                    if let Some(mode) = WindowMode::ALL.get(index) {
                        self.pending.window_mode = *mode;
                    }
                }
                GraphicsItem::Apply | GraphicsItem::Back => {}
            }
            self.sync_widgets();
        }
        None
    }
}

/// Where `quality` is in [`ShadowQuality::ALL`]: its row in the dropdown.
fn shadow_index(quality: ShadowQuality) -> usize {
    ShadowQuality::ALL
        .iter()
        .position(|candidate| *candidate == quality)
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn layout() -> GraphicsLayout {
        let scale = UiScale::default();
        GraphicsLayout {
            panel: UiRect::new(750.0, 200.0, 420.0, 520.0),
            padding: 28.0,
            title_height: 56.0,
            label_height: 30.0,
            row_height: 52.0,
            gap: 12.0,
            group_gap: 34.0,
            line_height: 26.0,
            viewport: VIEWPORT,
            scale,
        }
    }

    fn menu() -> GraphicsMenu {
        let mut menu = GraphicsMenu::new(DisplaySettings::default(), ShadowQuality::High);
        menu.set_options(
            vec![
                Resolution::new(3840, 2160),
                Resolution::new(2560, 1440),
                Resolution::new(1920, 1080),
                Resolution::new(1280, 720),
            ],
            Resolution::new(3840, 2160),
            DisplaySettings::default(),
        );
        menu.layout(&layout());
        menu
    }

    fn press_at(rect: UiRect) -> UiInput {
        UiInput {
            pointer: (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5),
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::default()
        }
    }

    fn click(menu: &mut GraphicsMenu, rect: UiRect) -> Option<GraphicsOutcome> {
        let first = menu.update(&press_at(rect));
        let mut release = press_at(rect);
        release.pointer_pressed = false;
        release.pointer_down = false;
        release.pointer_released = true;
        first.or(menu.update(&release))
    }

    fn dropdown(menu: &GraphicsMenu, item: GraphicsItem) -> Dropdown {
        match menu.screen().widget(item) {
            Some(Widget::Dropdown(dropdown)) => *dropdown,
            _ => panic!("{item:?} is a dropdown"),
        }
    }

    fn choose(menu: &mut GraphicsMenu, item: GraphicsItem, index: usize) {
        let closed = dropdown(menu, item);
        menu.update(&press_at(closed.rect));
        let open = dropdown(menu, item);
        assert!(open.is_open());
        menu.update(&press_at(open.option_rect(index)));
    }

    #[test]
    fn the_screen_opens_on_what_is_in_force_with_nothing_to_apply() {
        let menu = menu();
        assert_eq!(dropdown(&menu, GraphicsItem::Resolution).selected, 2);
        assert_eq!(dropdown(&menu, GraphicsItem::WindowMode).selected, 0);
        assert!(!menu.has_changes());
        assert_eq!(menu.resolution_labels()[0], "3840 x 2160");
        let apply = menu.screen().widget(GraphicsItem::Apply).expect("apply");
        assert!(!apply.is_enabled());
    }

    #[test]
    fn choosing_marks_pending_and_apply_asks_for_exactly_that() {
        let mut menu = menu();
        choose(&mut menu, GraphicsItem::Resolution, 1);
        choose(&mut menu, GraphicsItem::WindowMode, 2);
        let wanted = DisplaySettings::new(Resolution::new(2560, 1440), WindowMode::Fullscreen);
        assert_eq!(menu.pending(), wanted);
        assert_eq!(
            menu.current(),
            DisplaySettings::default(),
            "nothing applied yet"
        );
        assert!(menu.has_changes());
        let apply = menu
            .screen()
            .widget(GraphicsItem::Apply)
            .expect("apply")
            .rect();
        assert_eq!(
            click(&mut menu, apply),
            Some(GraphicsOutcome::Apply {
                display: Some(wanted),
                shadows: None,
            })
        );
    }

    #[test]
    fn a_shadow_level_waits_for_apply_like_the_display() {
        let mut menu = menu();
        let apply = menu
            .screen()
            .widget(GraphicsItem::Apply)
            .expect("apply")
            .rect();
        assert!(!menu.has_changes());
        // Low is the second level.
        choose(&mut menu, GraphicsItem::Shadows, 1);
        assert_eq!(menu.pending_shadows(), ShadowQuality::Low);
        assert_eq!(
            menu.shadow_quality(),
            ShadowQuality::High,
            "chosen is not applied"
        );
        assert!(menu.has_changes());
        assert!(!menu.has_display_changes());
        assert_eq!(
            click(&mut menu, apply),
            Some(GraphicsOutcome::Apply {
                display: None,
                shadows: Some(ShadowQuality::Low),
            })
        );
        // Once applied, nothing is left to apply.
        menu.set_shadow_quality(ShadowQuality::Low);
        assert!(!menu.has_changes());
        // Both at once: one APPLY carries both.
        choose(&mut menu, GraphicsItem::Shadows, 4);
        choose(&mut menu, GraphicsItem::WindowMode, 2);
        let Some(GraphicsOutcome::Apply { display, shadows }) = click(&mut menu, apply) else {
            panic!("APPLY asks for both");
        };
        assert_eq!(shadows, Some(ShadowQuality::Ultra));
        assert_eq!(display.map(|d| d.window_mode), Some(WindowMode::Fullscreen));
        // BACK forgets an unapplied level.
        menu.discard_changes();
        assert_eq!(menu.pending_shadows(), ShadowQuality::Low);
        assert_eq!(dropdown(&menu, GraphicsItem::Shadows).selected, 1);
    }

    #[test]
    fn apply_and_back_share_the_bottom_row() {
        let menu = menu();
        let rect = |item| menu.screen().widget(item).expect("item").rect();
        let (apply, back) = (rect(GraphicsItem::Apply), rect(GraphicsItem::Back));
        assert_eq!(apply.y, back.y);
        assert!(apply.right() < back.x, "APPLY is left of BACK");
        for item in [
            GraphicsItem::Resolution,
            GraphicsItem::WindowMode,
            GraphicsItem::Shadows,
        ] {
            assert!(
                rect(item).bottom() < apply.y,
                "{item:?} is below the buttons"
            );
        }
    }

    #[test]
    fn borderless_ignores_the_resolution_and_disables_its_list() {
        let mut menu = menu();
        choose(&mut menu, GraphicsItem::WindowMode, 1);
        assert_eq!(menu.pending().window_mode, WindowMode::BorderlessFullscreen);
        assert!(!dropdown(&menu, GraphicsItem::Resolution).state.enabled);
        assert!(menu.has_changes());
        // In force already as borderless at another resolution: nothing to apply.
        let mut menu = menu_with(DisplaySettings::new(
            Resolution::new(1280, 720),
            WindowMode::BorderlessFullscreen,
        ));
        menu.pending.resolution_width = 1920;
        assert!(!menu.has_changes());
    }

    fn menu_with(current: DisplaySettings) -> GraphicsMenu {
        let mut menu = menu();
        menu.set_current(current);
        menu
    }

    #[test]
    fn an_open_list_takes_the_click_from_what_is_under_it() {
        let mut menu = menu();
        let resolution = dropdown(&menu, GraphicsItem::Resolution);
        menu.update(&press_at(resolution.rect));
        assert!(menu.has_open_list());
        // The window-mode box sits under the open resolution list.
        let mode_box = dropdown(&menu, GraphicsItem::WindowMode).rect;
        menu.update(&press_at(mode_box));
        assert!(!dropdown(&menu, GraphicsItem::WindowMode).is_open());
    }

    #[test]
    fn the_dialog_keeps_reverts_and_counts_down() {
        let mut menu = menu();
        menu.begin_confirmation(15);
        assert_eq!(menu.confirming(), Some(15));
        menu.set_confirmation_seconds(9);
        assert_eq!(menu.confirming(), Some(9));
        let keep = menu
            .confirm_screen()
            .widget(ConfirmItem::Keep)
            .expect("keep")
            .rect();
        assert_eq!(click(&mut menu, keep), Some(GraphicsOutcome::Keep));
        let revert = menu
            .confirm_screen()
            .widget(ConfirmItem::Revert)
            .expect("revert")
            .rect();
        assert_eq!(click(&mut menu, revert), Some(GraphicsOutcome::Revert));
        let escape = UiInput {
            cancel: true,
            ..UiInput::default()
        };
        assert_eq!(menu.update(&escape), Some(GraphicsOutcome::Revert));
        // While it is up, the screen under it does not answer.
        let back = menu
            .screen()
            .widget(GraphicsItem::Back)
            .expect("back")
            .rect();
        assert_eq!(click(&mut menu, back), None);
        menu.end_confirmation();
        assert_eq!(menu.confirming(), None);
        assert_eq!(click(&mut menu, back), Some(GraphicsOutcome::Back));
    }

    #[test]
    fn leaving_drops_unapplied_choices_and_the_truth_wins() {
        let mut menu = menu();
        choose(&mut menu, GraphicsItem::Resolution, 0);
        assert!(menu.has_changes());
        menu.discard_changes();
        assert!(!menu.has_changes());
        assert_eq!(menu.pending(), DisplaySettings::default());
        // An Alt+Enter while the screen is open: the application says what is in force.
        let toggled = DisplaySettings::new(
            Resolution::new(1920, 1080),
            WindowMode::BorderlessFullscreen,
        );
        menu.set_current(toggled);
        assert_eq!(dropdown(&menu, GraphicsItem::WindowMode).selected, 1);
        assert_eq!(menu.pending(), toggled);
    }

    #[test]
    fn a_saved_size_the_monitor_lacks_shows_as_the_closest_it_has() {
        let mut menu = GraphicsMenu::new(DisplaySettings::default(), ShadowQuality::High);
        menu.set_options(
            vec![Resolution::new(2560, 1440), Resolution::new(1600, 900)],
            Resolution::new(2560, 1440),
            DisplaySettings::new(Resolution::new(1920, 1080), WindowMode::Windowed),
        );
        assert_eq!(menu.pending().resolution(), Resolution::new(1600, 900));
    }
}
