//! The DISPLAY screen: resolution and window mode, applied together and kept only when the
//! player says so.
//!
//! ```text
//! ┌──────────── DISPLAY ────────────┐
//! │ RESOLUTION                      │      ┌──────────────────────────────────┐
//! │ [ 3840 x 2160               ▼ ] │      │  KEEP THESE DISPLAY SETTINGS?    │
//! │ WINDOW MODE                     │      │  REVERTING IN 15 SECONDS...      │
//! │ [ BORDERLESS FULLSCREEN     ▼ ] │      │ [ KEEP CHANGES ]  [ REVERT ]     │
//! │ COVERS THE MONITOR AT ITS ...   │      └──────────────────────────────────┘
//! │ NOW: 1920 x 1080, WINDOWED      │          after APPLY, over everything
//! │ [ APPLY ]                       │
//! │ [ BACK  ]                       │
//! └─────────────────────────────────┘
//! ```
//!
//! Choosing in a dropdown changes nothing but the choice: the display is only touched by
//! APPLY, which the application carries out and then confirms through the dialog. The menu
//! never decides what is in force — the application tells it ([`DisplayMenu::set_current`])
//! after applying, reverting, or an Alt+Enter toggle, so the screen always shows the truth.
//!
//! The resolution list is the monitor's, given by the application
//! ([`DisplayMenu::set_options`]); this module only turns it into rows and remembers which
//! one is chosen.

use engine_render_data::ui::{UiRect, UiScale};
use engine_settings::{DisplaySettings, Resolution, WindowMode};

use super::input::UiInput;
use super::screen::UiScreen;
use super::widget::{Button, Dropdown, Widget, WidgetEvent};

/// The DISPLAY screen's items, in layout order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayItem {
    /// The resolution dropdown.
    Resolution,
    /// The window mode dropdown.
    WindowMode,
    /// Applies the choices.
    Apply,
    /// Returns to the settings screen, dropping unapplied choices.
    Back,
}

/// The confirmation dialog's buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmItem {
    /// Keep the new settings.
    Keep,
    /// Go back to the old ones.
    Revert,
}

/// What the DISPLAY screen asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayOutcome {
    /// Apply these settings, then confirm them.
    Apply(DisplaySettings),
    /// The player kept the settings being confirmed.
    Keep,
    /// The player wants the previous settings back.
    Revert,
    /// Leave the screen.
    Back,
}

/// Measurements the screen is laid out with, in physical pixels: the menu's own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayLayout {
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

/// The DISPLAY screen and its confirmation dialog.
#[derive(Debug, Clone)]
pub struct DisplayMenu {
    screen: UiScreen<DisplayItem>,
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
}

impl DisplayMenu {
    /// The screen with `current` in force and, until the monitor is asked, only its
    /// resolution on offer.
    #[must_use]
    pub fn new(current: DisplaySettings) -> Self {
        let mut screen = UiScreen::new();
        screen.push(DisplayItem::Resolution, Dropdown::new("RESOLUTION", 1, 0));
        screen.push(
            DisplayItem::WindowMode,
            Dropdown::new("WINDOW MODE", WindowMode::ALL.len(), 0),
        );
        screen.push(DisplayItem::Apply, Button::new("APPLY"));
        screen.push(DisplayItem::Back, Button::new("BACK"));
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

    /// What the dropdowns say.
    #[must_use]
    pub fn pending(&self) -> DisplaySettings {
        self.pending
    }

    /// Whether APPLY would change anything.
    #[must_use]
    pub fn has_changes(&self) -> bool {
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
    pub fn screen(&self) -> &UiScreen<DisplayItem> {
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
        self.sync_widgets();
        self.screen.reset();
    }

    fn close_dropdowns(&mut self) {
        for item in [DisplayItem::Resolution, DisplayItem::WindowMode] {
            if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(item) {
                dropdown.close();
            }
        }
    }

    /// The dropdowns and APPLY made to show `pending`.
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
        if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(DisplayItem::Resolution) {
            if dropdown.len != self.options.len() || dropdown.selected != selected {
                dropdown.set_options(self.options.len(), selected);
            }
            dropdown.state.enabled = uses_resolution;
        }
        let mode = WindowMode::ALL
            .iter()
            .position(|mode| *mode == self.pending.window_mode)
            .unwrap_or(0);
        if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(DisplayItem::WindowMode) {
            dropdown.selected = mode;
        }
        let changes = self.has_changes();
        if let Some(Widget::Button(button)) = self.screen.widget_mut(DisplayItem::Apply) {
            button.state.enabled = changes;
        }
    }

    /// Places everything inside `layout.panel`.
    pub fn layout(&mut self, layout: &DisplayLayout) {
        let x = layout.panel.x + layout.padding;
        let width = layout.panel.width - layout.padding * 2.0;
        let mut y = layout.panel.y + layout.padding + layout.title_height;
        for item in [DisplayItem::Resolution, DisplayItem::WindowMode] {
            y += layout.label_height;
            self.screen
                .set_rect(item, UiRect::new(x, y, width, layout.row_height));
            y += layout.row_height + layout.gap;
        }
        // The mode's description and the line saying what is in force.
        y += layout.line_height * 2.0 + layout.group_gap - layout.gap;
        self.screen.set_rect(
            DisplayItem::Apply,
            UiRect::new(x, y, width, layout.row_height),
        );
        y += layout.row_height + layout.gap;
        self.screen.set_rect(
            DisplayItem::Back,
            UiRect::new(x, y, width, layout.row_height),
        );
        let screen_height = layout.viewport.1 as f32;
        let margin = layout.scale.px(8.0);
        for item in [DisplayItem::Resolution, DisplayItem::WindowMode] {
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
            + (label_height + row_height + gap) * 2.0
            + line_height * 2.0
            + group_gap
            - gap
            + row_height * 2.0
            + gap
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
    pub fn update(&mut self, input: &UiInput) -> Option<DisplayOutcome> {
        if self.confirming.is_some() {
            if input.cancel {
                return Some(DisplayOutcome::Revert);
            }
            let mut outcome = None;
            for (item, event) in self.confirm.update(input) {
                if *event == WidgetEvent::Activated {
                    outcome = Some(match item {
                        ConfirmItem::Keep => DisplayOutcome::Keep,
                        ConfirmItem::Revert => DisplayOutcome::Revert,
                    });
                }
            }
            return outcome;
        }

        let open = [DisplayItem::Resolution, DisplayItem::WindowMode]
            .into_iter()
            .find(|item| {
                matches!(self.screen.widget(*item), Some(Widget::Dropdown(dropdown)) if dropdown.is_open())
            });
        let mut chosen: Option<(DisplayItem, usize)> = None;
        if let Some(item) = open {
            if let Some(Widget::Dropdown(dropdown)) = self.screen.widget_mut(item)
                && let Some(WidgetEvent::Chosen(index)) = dropdown.update(input)
            {
                chosen = Some((item, index));
            }
        } else {
            if input.cancel {
                return Some(DisplayOutcome::Back);
            }
            let mut apply = false;
            let mut back = false;
            for (item, event) in self.screen.update(input) {
                match (item, event) {
                    (DisplayItem::Apply, WidgetEvent::Activated) => apply = true,
                    (DisplayItem::Back, WidgetEvent::Activated) => back = true,
                    (item, WidgetEvent::Chosen(index)) => chosen = Some((*item, *index)),
                    _ => {}
                }
            }
            if back {
                return Some(DisplayOutcome::Back);
            }
            if apply && self.has_changes() {
                return Some(DisplayOutcome::Apply(self.pending));
            }
        }
        if let Some((item, index)) = chosen {
            match item {
                DisplayItem::Resolution => {
                    if let Some(resolution) = self.options.get(index) {
                        self.pending.resolution_width = resolution.width;
                        self.pending.resolution_height = resolution.height;
                    }
                }
                DisplayItem::WindowMode => {
                    if let Some(mode) = WindowMode::ALL.get(index) {
                        self.pending.window_mode = *mode;
                    }
                }
                DisplayItem::Apply | DisplayItem::Back => {}
            }
            self.sync_widgets();
        }
        None
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn layout() -> DisplayLayout {
        let scale = UiScale::default();
        DisplayLayout {
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

    fn menu() -> DisplayMenu {
        let mut menu = DisplayMenu::new(DisplaySettings::default());
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

    fn click(menu: &mut DisplayMenu, rect: UiRect) -> Option<DisplayOutcome> {
        let first = menu.update(&press_at(rect));
        let mut release = press_at(rect);
        release.pointer_pressed = false;
        release.pointer_down = false;
        release.pointer_released = true;
        first.or(menu.update(&release))
    }

    fn dropdown(menu: &DisplayMenu, item: DisplayItem) -> Dropdown {
        match menu.screen().widget(item) {
            Some(Widget::Dropdown(dropdown)) => *dropdown,
            _ => panic!("{item:?} is a dropdown"),
        }
    }

    fn choose(menu: &mut DisplayMenu, item: DisplayItem, index: usize) {
        let closed = dropdown(menu, item);
        menu.update(&press_at(closed.rect));
        let open = dropdown(menu, item);
        assert!(open.is_open());
        menu.update(&press_at(open.option_rect(index)));
    }

    #[test]
    fn the_screen_opens_on_what_is_in_force_with_nothing_to_apply() {
        let menu = menu();
        assert_eq!(dropdown(&menu, DisplayItem::Resolution).selected, 2);
        assert_eq!(dropdown(&menu, DisplayItem::WindowMode).selected, 0);
        assert!(!menu.has_changes());
        assert_eq!(menu.resolution_labels()[0], "3840 x 2160");
        let apply = menu.screen().widget(DisplayItem::Apply).expect("apply");
        assert!(!apply.is_enabled());
    }

    #[test]
    fn choosing_marks_pending_and_apply_asks_for_exactly_that() {
        let mut menu = menu();
        choose(&mut menu, DisplayItem::Resolution, 1);
        choose(&mut menu, DisplayItem::WindowMode, 2);
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
            .widget(DisplayItem::Apply)
            .expect("apply")
            .rect();
        assert_eq!(click(&mut menu, apply), Some(DisplayOutcome::Apply(wanted)));
    }

    #[test]
    fn borderless_ignores_the_resolution_and_disables_its_list() {
        let mut menu = menu();
        choose(&mut menu, DisplayItem::WindowMode, 1);
        assert_eq!(menu.pending().window_mode, WindowMode::BorderlessFullscreen);
        assert!(!dropdown(&menu, DisplayItem::Resolution).state.enabled);
        assert!(menu.has_changes());
        // In force already as borderless at another resolution: nothing to apply.
        let mut menu = menu_with(DisplaySettings::new(
            Resolution::new(1280, 720),
            WindowMode::BorderlessFullscreen,
        ));
        menu.pending.resolution_width = 1920;
        assert!(!menu.has_changes());
    }

    fn menu_with(current: DisplaySettings) -> DisplayMenu {
        let mut menu = menu();
        menu.set_current(current);
        menu
    }

    #[test]
    fn an_open_list_takes_the_click_from_what_is_under_it() {
        let mut menu = menu();
        let resolution = dropdown(&menu, DisplayItem::Resolution);
        menu.update(&press_at(resolution.rect));
        assert!(menu.has_open_list());
        // The window-mode box sits under the open resolution list.
        let mode_box = dropdown(&menu, DisplayItem::WindowMode).rect;
        menu.update(&press_at(mode_box));
        assert!(!dropdown(&menu, DisplayItem::WindowMode).is_open());
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
        assert_eq!(click(&mut menu, keep), Some(DisplayOutcome::Keep));
        let revert = menu
            .confirm_screen()
            .widget(ConfirmItem::Revert)
            .expect("revert")
            .rect();
        assert_eq!(click(&mut menu, revert), Some(DisplayOutcome::Revert));
        let escape = UiInput {
            cancel: true,
            ..UiInput::default()
        };
        assert_eq!(menu.update(&escape), Some(DisplayOutcome::Revert));
        // While it is up, the screen under it does not answer.
        let back = menu
            .screen()
            .widget(DisplayItem::Back)
            .expect("back")
            .rect();
        assert_eq!(click(&mut menu, back), None);
        menu.end_confirmation();
        assert_eq!(menu.confirming(), None);
        assert_eq!(click(&mut menu, back), Some(DisplayOutcome::Back));
    }

    #[test]
    fn leaving_drops_unapplied_choices_and_the_truth_wins() {
        let mut menu = menu();
        choose(&mut menu, DisplayItem::Resolution, 0);
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
        assert_eq!(dropdown(&menu, DisplayItem::WindowMode).selected, 1);
        assert_eq!(menu.pending(), toggled);
    }

    #[test]
    fn a_saved_size_the_monitor_lacks_shows_as_the_closest_it_has() {
        let mut menu = DisplayMenu::new(DisplaySettings::default());
        menu.set_options(
            vec![Resolution::new(2560, 1440), Resolution::new(1600, 900)],
            Resolution::new(2560, 1440),
            DisplaySettings::new(Resolution::new(1920, 1080), WindowMode::Windowed),
        );
        assert_eq!(menu.pending().resolution(), Resolution::new(1600, 900));
    }
}
