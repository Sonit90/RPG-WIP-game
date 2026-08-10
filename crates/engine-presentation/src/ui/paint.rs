//! Drawing the menus: widget state in, [`UiDrawList`] commands out.
//!
//! Separate from the widgets on purpose. A widget is state and a rule; this is the only
//! place that knows what state *looks* like, so restyling the menu touches one file and
//! changes no behaviour, and every widget test runs without a colour in sight.
//!
//! Everything here goes through the existing UI pass: rectangles, outlines and runs of text
//! appended to the same list the HUD builds, which the renderer batches into one vertex
//! buffer and one draw call. A menu is a few dozen more commands in a list that already
//! exists — it creates no texture, no buffer and no pipeline, and opening it allocates
//! nothing after the first time.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect, UiScale};

use super::menu::{MenuScreen, MenuSystem, PauseItem};
use super::widget::{Slider, Widget, WidgetState};

/// How the menus are coloured and sized.
///
/// A struct rather than constants so a test can drive an unusual layout and a future theme
/// can change a colour without touching the code that draws with it — the same reason
/// [`crate::game_ui::UiTheme`] is one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuTheme {
    /// The wash drawn over the whole screen behind a menu.
    pub scrim: UiColor,
    /// The menu panel.
    pub panel: UiColor,
    /// The panel's border.
    pub panel_border: UiColor,
    /// A button at rest.
    pub button: UiColor,
    /// A button under the pointer.
    pub button_hovered: UiColor,
    /// A button being pressed.
    pub button_pressed: UiColor,
    /// A button that cannot be used.
    pub button_disabled: UiColor,
    /// Ordinary text.
    pub text: UiColor,
    /// Text on a disabled widget.
    pub text_disabled: UiColor,
    /// A heading.
    pub title: UiColor,
    /// A slider's track.
    pub track: UiColor,
    /// The filled part of a slider's track.
    pub track_fill: UiColor,
    /// A slider's handle.
    pub handle: UiColor,
    /// A separator between two groups of buttons.
    pub separator: UiColor,
    /// Border thickness in pixels.
    pub border_pixels: f32,
    /// Glyph scale for a title.
    pub title_scale: f32,
    /// Glyph scale for a button label.
    pub label_scale: f32,
    /// Glyph scale for a number beside a slider.
    pub value_scale: f32,
    /// Width of a slider's handle in pixels.
    pub handle_width: f32,
}

impl Default for MenuTheme {
    fn default() -> Self {
        Self {
            // Dark enough that the menu reads as a layer over the game rather than as part
            // of it, translucent enough that the world is still visible behind it.
            scrim: [0.02, 0.03, 0.05, 0.72],
            panel: [0.08, 0.10, 0.14, 0.96],
            panel_border: [0.38, 0.46, 0.60, 1.0],
            button: [0.14, 0.18, 0.24, 1.0],
            button_hovered: [0.22, 0.30, 0.42, 1.0],
            button_pressed: [0.32, 0.44, 0.62, 1.0],
            button_disabled: [0.10, 0.11, 0.13, 1.0],
            text: [0.92, 0.94, 0.98, 1.0],
            text_disabled: [0.45, 0.47, 0.52, 1.0],
            title: [0.98, 0.86, 0.45, 1.0],
            track: [0.10, 0.12, 0.16, 1.0],
            track_fill: [0.35, 0.55, 0.85, 1.0],
            handle: [0.92, 0.94, 0.98, 1.0],
            separator: [0.26, 0.31, 0.40, 1.0],
            border_pixels: 2.0,
            title_scale: 4.0,
            label_scale: 2.0,
            value_scale: 2.0,
            handle_width: 14.0,
        }
    }
}

impl MenuTheme {
    /// The fill for a widget in this state.
    #[must_use]
    fn fill(&self, state: &WidgetState) -> UiColor {
        if !state.enabled {
            self.button_disabled
        } else if state.pressed {
            self.button_pressed
        } else if state.hovered {
            self.button_hovered
        } else {
            self.button
        }
    }

    /// The text colour for a widget in this state.
    #[must_use]
    fn label_color(&self, state: &WidgetState) -> UiColor {
        if state.enabled {
            self.text
        } else {
            self.text_disabled
        }
    }
}

/// Scratch strings the menu formats numbers into.
///
/// Owned by the caller and reused, because formatting a number is the one part of drawing a
/// menu that would otherwise allocate every frame.
#[derive(Debug, Default)]
pub struct MenuScratch {
    minimum: String,
    maximum: String,
    value: String,
}

impl MenuScratch {
    /// Buffers large enough for the numbers a settings screen prints.
    #[must_use]
    pub fn new() -> Self {
        Self {
            minimum: String::with_capacity(16),
            maximum: String::with_capacity(16),
            value: String::with_capacity(24),
        }
    }
}

/// Appends whichever menu is open to `list`. Draws nothing when none is.
pub fn push_menu(
    list: &mut UiDrawList,
    menu: &MenuSystem,
    theme: &MenuTheme,
    scratch: &mut MenuScratch,
    viewport: (u32, u32),
) {
    // The menu lays itself out at the player's scale, so everything drawn over that
    // layout has to use the same one. Taken from the menu rather than passed in, so the
    // two cannot be given different answers.
    let scale = menu.scale();
    match menu.screen() {
        MenuScreen::None => {}
        MenuScreen::Pause => push_pause(list, menu, theme, viewport, scale),
        MenuScreen::Settings => push_settings(list, menu, theme, scratch, viewport, scale),
    }
}

/// The main menu: a scrim, a panel, the build's version, and three buttons in two groups.
fn push_pause(
    list: &mut UiDrawList,
    menu: &MenuSystem,
    theme: &MenuTheme,
    viewport: (u32, u32),
    scale: UiScale,
) {
    let panel = menu.pause_panel();
    push_backdrop(list, theme, viewport, panel, menu.title(), scale);

    let mut previous_bottom = None;
    for (item, widget) in menu.pause_screen().widgets() {
        let rect = widget.rect();
        // A rule between groups, drawn in the gap rather than as part of either button, so
        // "RESUME is separated" is visible and not just spacing.
        if let (Some(previous), PauseItem::Resume | PauseItem::Quit) = (previous_bottom, item) {
            let gap: f32 = rect.y - previous;
            let midpoint = previous + gap * 0.5;
            list.rect(
                UiRect::new(
                    panel.x + scale.px(40.0),
                    midpoint - scale.px(1.0),
                    panel.width - scale.px(80.0),
                    scale.px(1.0),
                ),
                theme.separator,
            );
        }
        push_widget(list, widget, theme, scale);
        previous_bottom = Some(rect.bottom());
    }
}

/// The settings screen: a panel, the sensitivity slider with its bounds, and BACK.
fn push_settings(
    list: &mut UiDrawList,
    menu: &MenuSystem,
    theme: &MenuTheme,
    scratch: &mut MenuScratch,
    viewport: (u32, u32),
    scale: UiScale,
) {
    let panel = menu.settings_panel();
    push_backdrop(list, theme, viewport, panel, "SETTINGS", scale);

    // Every slider is drawn in full, with its name and bounds. Matching on the widget
    // rather than on which setting it is means adding a setting needs no change here.
    for (_, widget) in menu.settings_screen().widgets() {
        match widget {
            Widget::Slider(slider) => push_slider(list, slider, theme, scratch, scale),
            _ => push_widget(list, widget, theme, scale),
        }
    }
}

/// The scrim, the panel and its title.
fn push_backdrop(
    list: &mut UiDrawList,
    theme: &MenuTheme,
    viewport: (u32, u32),
    panel: UiRect,
    title: &str,
    scale: UiScale,
) {
    list.rect(
        UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
        theme.scrim,
    );
    list.rect(panel, theme.panel);
    list.outline(panel, scale.px(theme.border_pixels), theme.panel_border);

    let (origin, align) = centre_text(panel.x + panel.width * 0.5, panel.y + scale.px(22.0));
    list.text_aligned(
        origin,
        title,
        theme.title,
        scale.text(theme.title_scale),
        align,
    );
}

/// One widget in whatever state it is in.
fn push_widget(list: &mut UiDrawList, widget: &Widget, theme: &MenuTheme, scale: UiScale) {
    match widget {
        Widget::Button(button) => push_labelled_box(
            list,
            theme,
            button.rect,
            button.label,
            &button.state,
            None,
            scale,
        ),
        Widget::Checkbox(checkbox) => push_labelled_box(
            list,
            theme,
            checkbox.rect,
            checkbox.label,
            &checkbox.state,
            Some(checkbox.checked),
            scale,
        ),
        Widget::Slider(slider) => {
            // A slider drawn without its numbers: the settings screen draws the full form,
            // this is the fallback for a screen that only wants the track.
            push_track(list, slider, theme, scale);
        }
    }
}

/// A filled box with a centred label, and the tick of a checkbox if there is one.
fn push_labelled_box(
    list: &mut UiDrawList,
    theme: &MenuTheme,
    rect: UiRect,
    label: &str,
    state: &WidgetState,
    checked: Option<bool>,
    scale: UiScale,
) {
    list.rect(rect, theme.fill(state));

    match checked {
        Some(checked) => {
            // Box on the left, label beside it: the shape every options screen uses.
            let size = (rect.height - scale.px(16.0)).max(scale.px(8.0));
            let box_rect = UiRect::new(
                rect.x + scale.px(12.0),
                rect.y + (rect.height - size) * 0.5,
                size,
                size,
            );
            list.outline(box_rect, scale.px(theme.border_pixels), theme.panel_border);
            if checked {
                list.rect(box_rect.inset(scale.px(5.0)), theme.track_fill);
            }
            list.text_at(
                (
                    box_rect.right() + scale.px(14.0),
                    text_baseline(rect, scale.text(theme.label_scale)),
                ),
                label,
                theme.label_color(state),
                scale.text(theme.label_scale),
            );
        }
        None => {
            let (origin, align) = centre_text(
                rect.x + rect.width * 0.5,
                text_baseline(rect, scale.text(theme.label_scale)),
            );
            list.text_aligned(
                origin,
                label,
                theme.label_color(state),
                scale.text(theme.label_scale),
                align,
            );
        }
    }
}

/// A slider's track, fill and handle.
fn push_track(list: &mut UiDrawList, slider: &Slider, theme: &MenuTheme, scale: UiScale) {
    let track = slider.rect;
    list.rect(track, theme.track);
    list.rect(track.fraction_of_width(slider.fraction()), theme.track_fill);

    let handle = handle_rect(slider, theme, scale);
    list.rect(handle, theme.handle);
}

/// A slider with everything the player needs to read it: name, bounds, handle and value.
///
/// ```text
/// CAMERA SENSITIVITY
/// 0.10 [=====O--------] 5.00
///              1.25
/// ```
fn push_slider(
    list: &mut UiDrawList,
    slider: &Slider,
    theme: &MenuTheme,
    scratch: &mut MenuScratch,
    scale: UiScale,
) {
    let track = slider.rect;

    list.text_at(
        (track.x - scale.px(62.0), track.y - scale.px(26.0)),
        slider.label,
        theme.label_color(&slider.state),
        scale.text(theme.label_scale),
    );

    push_track(list, slider, theme, scale);

    // The bounds, printed at the ends of the thing they bound.
    write_hundredths(&mut scratch.minimum, slider.minimum());
    write_hundredths(&mut scratch.maximum, slider.maximum());
    let baseline = track.y + (track.height - 7.0 * scale.text(theme.value_scale)) * 0.5;
    list.text_aligned(
        (track.x - scale.px(10.0), baseline),
        &scratch.minimum,
        theme.text,
        scale.text(theme.value_scale),
        TextAlign::Right,
    );
    list.text_at(
        (track.right() + scale.px(10.0), baseline),
        &scratch.maximum,
        theme.text,
        scale.text(theme.value_scale),
    );

    // The current value, under the handle, so the number and the position agree.
    write_hundredths(&mut scratch.value, slider.value());
    let handle = handle_rect(slider, theme, scale);
    let (origin, align) = centre_text(
        handle.x + handle.width * 0.5,
        track.bottom() + scale.px(12.0),
    );
    list.text_aligned(
        origin,
        &scratch.value,
        theme.title,
        scale.text(theme.value_scale),
        align,
    );
}

/// Where the handle sits, clamped so it never hangs off either end of its track.
fn handle_rect(slider: &Slider, theme: &MenuTheme, scale: UiScale) -> UiRect {
    let width = scale.px(theme.handle_width).min(slider.rect.width.max(1.0));
    let travel = (slider.rect.width - width).max(0.0);
    UiRect::new(
        slider.rect.x + travel * slider.fraction(),
        slider.rect.y - scale.px(4.0),
        width,
        slider.rect.height + scale.px(8.0),
    )
}

/// The y a line of text sits at to be vertically centred in `rect`.
fn text_baseline(rect: UiRect, scale: f32) -> f32 {
    // The font is 7 pixels tall at scale 1.
    rect.y + (rect.height - 7.0 * scale) * 0.5
}

/// An origin and alignment that centre a line of text on `x`.
fn centre_text(x: f32, y: f32) -> ((f32, f32), TextAlign) {
    ((x, y), TextAlign::Center)
}

/// Writes a value with two decimal places into `out`, replacing what was there.
///
/// `format!` would allocate a `String` per number, and the settings screen prints three of
/// them every frame it is open.
pub fn write_hundredths(out: &mut String, value: f32) {
    use std::fmt::Write as _;
    out.clear();
    let _ = write!(out, "{value:.2}");
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::ui::input::UiInput;
    use crate::ui::menu::MenuSystem;
    use engine_render_data::ui::UiCommand;
    use engine_settings::{CAMERA_SENSITIVITY_MAX, CAMERA_SENSITIVITY_MIN, GameSettings};

    fn menu() -> MenuSystem {
        let mut menu = MenuSystem::new(GameSettings::default(), "0.1.0");
        menu.layout((1280, 720));
        menu
    }

    /// Clicks SETTINGS, the only way into the settings screen.
    fn open_settings(menu: &mut MenuSystem) {
        let rect = menu
            .pause_screen()
            .widget(PauseItem::Settings)
            .expect("SETTINGS exists")
            .rect();
        let (x, y) = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        menu.update(&UiInput {
            pointer: (x, y),
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        });
        menu.update(&UiInput {
            pointer: (x, y),
            pointer_released: true,
            ..UiInput::new()
        });
    }

    fn render(menu: &MenuSystem) -> UiDrawList {
        let mut list = UiDrawList::new();
        let mut scratch = MenuScratch::new();
        push_menu(
            &mut list,
            menu,
            &MenuTheme::default(),
            &mut scratch,
            (1280, 720),
        );
        list
    }

    /// Every string the list draws, in order.
    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } => Some(list.text_of(*range).to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_closed_menu_draws_nothing_at_all() {
        // The property the whole gameplay frame budget depends on.
        let list = render(&menu());
        assert!(list.is_empty(), "{} commands with no menu", list.len());
    }

    #[test]
    fn the_menu_draws_its_three_actions_over_a_scrim() {
        let mut menu = menu();
        menu.open_pause();
        let list = render(&menu);

        let drawn = texts(&list);
        assert!(drawn.contains(&"0.1.0".to_owned()), "no version: {drawn:?}");
        assert!(
            !drawn.iter().any(|line| line.contains("PAUSED")),
            "the menu still claims the game is paused: {drawn:?}"
        );
        assert!(drawn.contains(&"SETTINGS".to_owned()), "{drawn:?}");
        assert!(drawn.contains(&"QUIT".to_owned()), "{drawn:?}");
        assert!(drawn.contains(&"RESUME".to_owned()), "{drawn:?}");

        // The first command covers the whole viewport: the menu is a layer, not a widget
        // floating in the middle of a live game.
        let UiCommand::Rect { rect, color } = list.commands()[0] else {
            panic!("the first command is the scrim");
        };
        assert_eq!((rect.width, rect.height), (1280.0, 720.0));
        assert!(
            color[3] > 0.5 && color[3] < 1.0,
            "the scrim is opaque or absent"
        );
    }

    #[test]
    fn the_settings_screen_shows_the_name_the_bounds_and_the_current_value() {
        let mut menu = menu();
        menu.open_pause();
        open_settings(&mut menu);
        menu.apply_settings(GameSettings {
            camera_sensitivity: 1.25,
            ..GameSettings::default()
        });

        let drawn = texts(&render(&menu));
        assert!(drawn.contains(&"SETTINGS".to_owned()), "{drawn:?}");
        assert!(
            drawn.contains(&"CAMERA SENSITIVITY".to_owned()),
            "{drawn:?}"
        );
        assert!(drawn.contains(&"0.10".to_owned()), "no minimum: {drawn:?}");
        assert!(drawn.contains(&"5.00".to_owned()), "no maximum: {drawn:?}");
        assert!(drawn.contains(&"1.25".to_owned()), "no value: {drawn:?}");
        assert!(drawn.contains(&"BACK".to_owned()), "{drawn:?}");
    }

    #[test]
    fn the_printed_bounds_are_the_sliders_bounds() {
        let mut minimum = String::new();
        let mut maximum = String::new();
        write_hundredths(&mut minimum, CAMERA_SENSITIVITY_MIN);
        write_hundredths(&mut maximum, CAMERA_SENSITIVITY_MAX);
        assert_eq!(minimum, "0.10");
        assert_eq!(maximum, "5.00");
    }

    #[test]
    fn the_handle_stays_on_its_track_at_both_ends() {
        let mut menu = menu();
        menu.open_pause();
        open_settings(&mut menu);
        let theme = MenuTheme::default();

        for value in [CAMERA_SENSITIVITY_MIN, 1.0, CAMERA_SENSITIVITY_MAX] {
            menu.apply_settings(GameSettings {
                camera_sensitivity: value,
                ..GameSettings::default()
            });
            let slider = menu.sensitivity_slider().expect("a slider");
            let handle = handle_rect(slider, &theme, menu.scale());
            assert!(
                handle.x >= slider.rect.x - 0.01,
                "{value} hangs off the left"
            );
            assert!(
                handle.right() <= slider.rect.right() + 0.01,
                "{value} hangs off the right"
            );
        }
    }

    #[test]
    fn a_hovered_button_is_drawn_differently_from_a_resting_one() {
        let mut menu = menu();
        menu.open_pause();
        let resting = render(&menu);

        let rect = menu
            .pause_screen()
            .widget(PauseItem::Quit)
            .expect("QUIT exists")
            .rect();
        menu.update(&UiInput::new().with_pointer(rect.x + 10.0, rect.y + 10.0));
        let hovered = render(&menu);

        assert_eq!(
            resting.len(),
            hovered.len(),
            "hovering changed the geometry"
        );
        assert_ne!(
            resting.commands(),
            hovered.commands(),
            "hovering changed nothing that is drawn"
        );
    }

    #[test]
    fn the_only_outline_on_the_menu_is_its_panel_border() {
        // Nothing is focused, because there is no focus: hover is the only state a button
        // shows, and it shows it by its fill.
        let mut menu = menu();
        menu.open_pause();
        let list = render(&menu);
        let outlines = list
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Outline { .. }))
            .count();
        assert_eq!(outlines, 1, "something other than the panel is outlined");
    }

    #[test]
    fn the_pause_menu_draws_a_rule_between_its_groups() {
        let mut menu = menu();
        menu.open_pause();
        let list = render(&menu);
        let thin_rules = list
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Rect { rect, .. } if rect.height == 1.0))
            .count();
        assert_eq!(thin_rules, 2, "one rule above QUIT and one above RESUME");
    }

    #[test]
    fn drawing_the_menu_repeatedly_allocates_nothing_after_the_first_frame() {
        let mut menu = menu();
        menu.open_pause();
        open_settings(&mut menu);

        let mut list = UiDrawList::with_capacity(64, 512);
        let mut scratch = MenuScratch::new();
        let theme = MenuTheme::default();
        for _ in 0..4 {
            list.clear();
            push_menu(&mut list, &menu, &theme, &mut scratch, (1280, 720));
        }
        let commands = list.len();
        let text = list.text().len();
        let scratch_capacity = (
            scratch.minimum.capacity(),
            scratch.maximum.capacity(),
            scratch.value.capacity(),
        );

        for _ in 0..1_000 {
            list.clear();
            push_menu(&mut list, &menu, &theme, &mut scratch, (1280, 720));
        }
        assert_eq!(list.len(), commands, "the command count drifted");
        assert_eq!(list.text().len(), text, "the text drifted");
        assert_eq!(
            (
                scratch.minimum.capacity(),
                scratch.maximum.capacity(),
                scratch.value.capacity()
            ),
            scratch_capacity,
            "a number reallocated its buffer"
        );
    }

    #[test]
    fn every_character_the_menu_draws_has_a_glyph() {
        // The renderer's 5x7 font covers exactly this set; anything else is a hollow box.
        const ALLOWED: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        let mut menu = menu();
        menu.open_pause();
        for screen in 0..2 {
            if screen == 1 {
                open_settings(&mut menu);
            }
            for line in texts(&render(&menu)) {
                for character in line.chars() {
                    assert!(ALLOWED.contains(character), "{character:?} in {line:?}");
                }
            }
        }
    }

    #[test]
    fn a_checkbox_draws_its_box_and_fills_it_only_when_checked() {
        // Nothing in the game uses one yet; the widget is drawn and tested so the next
        // settings screen is a line of layout rather than a new renderer path.
        use crate::ui::widget::Checkbox;
        let theme = MenuTheme::default();
        let mut list = UiDrawList::new();
        let mut checkbox = Checkbox::new("VSYNC");
        checkbox.rect = UiRect::new(0.0, 0.0, 200.0, 40.0);

        push_widget(
            &mut list,
            &Widget::from(checkbox),
            &theme,
            UiScale::default(),
        );
        let unchecked = list.len();

        list.clear();
        push_widget(
            &mut list,
            &Widget::from(checkbox.checked(true)),
            &theme,
            UiScale::default(),
        );
        assert_eq!(list.len(), unchecked + 1, "the tick is one more rectangle");
        assert!(texts(&list).contains(&"VSYNC".to_owned()));
    }
}
