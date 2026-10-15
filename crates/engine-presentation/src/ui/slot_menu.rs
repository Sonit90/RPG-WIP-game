//! What a right click on an action bar slot opens: a spell picker and a key binder.
//!
//! ```text
//! right click a slot
//!        |
//!        v
//!    Actions  --CHANGE SPELL-->  Picker   --pick-->  AssignSpell
//!        |    --ASSIGN KEY---->  Capture  --key--->  AssignKey
//!        |                          |               Confirm --REPLACE--> AssignKey
//!        |                          |                       --CANCEL---> Actions
//!        |    --CLEAR SLOT---->  ClearSlot
//!        v
//!      closed
//! ```
//!
//! ## It changes nothing itself
//!
//! Every path out of this menu is a [`SlotMenuOutcome`] the application carries out, exactly
//! as [`MenuAction`](super::menu::MenuAction) already works. The menu reads the action bars
//! to answer "what is in this slot" and "who already has this key", and writes to nothing.
//! That is what keeps the assignment, the conflict rule and the save in one place instead of
//! three.
//!
//! ## Conflicts are resolved before anything changes
//!
//! A key the game itself needs — movement, targeting, escape — is refused outright and the
//! menu says which action holds it. A key another slot holds raises a prompt, and only
//! `REPLACE` produces an outcome. There is no path through this state machine that produces
//! two slots on one key, and [`ActionBarSet::assign_key`] would not allow one anyway.

use engine_core::input::KeyChord;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::input_map::Bindings;
use engine_render_data::ui::{UiDrawList, UiRect, UiScale};

use super::action_bar::{ActionBarSet, KeyConflict, SlotAddress};
use super::input::UiInput;
use super::screen::UiScreen;
use super::widget::{Button, WidgetEvent};

/// Layout constants, in logical pixels.
mod layout {
    /// Width of the actions panel and the confirmation.
    pub const PANEL_WIDTH: f32 = 240.0;
    /// Height of one button.
    pub const ROW_HEIGHT: f32 = 30.0;
    /// Gap between rows.
    pub const ROW_GAP: f32 = 4.0;
    /// Padding inside a panel.
    pub const PADDING: f32 = 8.0;
    /// Height of the heading that names the slot and what is in it.
    pub const HEADING: f32 = 44.0;
    /// Size of one entry in the spell picker.
    pub const PICK_SIZE: f32 = 48.0;
    /// Gap between picker entries.
    pub const PICK_GAP: f32 = 6.0;
    /// Entries per row in the picker.
    pub const PICK_COLUMNS: usize = 5;
}

/// Which action the menu's first screen offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMenuItem {
    /// Opens the spell picker.
    ChangeSpell,
    /// Enters key capture.
    AssignKey,
    /// Empties the slot: spell and key both.
    ClearSlot,
    /// Closes without changing anything.
    Close,
}

/// The confirmation's two buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmItem {
    /// Leave the existing binding alone.
    Cancel,
    /// Take the key from whoever has it.
    Replace,
}

/// What the menu is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMenuMode {
    /// The three actions.
    Actions,
    /// A grid of every assignable spell.
    Picker,
    /// `PRESS A KEY...`, waiting for one.
    Capture,
    /// A key the game itself needs was pressed. Says which action holds it.
    Refused(&'static str),
    /// Another slot holds the key. Offers to take it.
    Confirm {
        /// The key, or combination, the player pressed.
        key: KeyChord,
        /// The slot that currently has it.
        holder: SlotAddress,
    },
}

/// What the application should do about what the player did in the menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotMenuOutcome {
    /// Nothing happened this frame.
    Idle,
    /// The menu closed. Nothing changed.
    Closed,
    /// Put this spell in this slot, or empty it of its spell if `None`.
    AssignSpell(SlotAddress, Option<FireAbility>),
    /// Give this slot this key or combination, taking it from whoever had it.
    AssignKey(SlotAddress, KeyChord),
    /// Empty this slot: spell and key.
    ClearSlot(SlotAddress),
}

/// Every spell `hero` may put in a slot, in the order the picker lists them.
///
/// The *current* hero's, not the game's: a picker offering a Warrior's cleave to a mage is
/// offering a button that could never be pressed. Read from the hero's own bar rather than
/// restated, so an ability added to a class appears in its picker without this file
/// changing.
pub fn assignable_spells(hero: Hero) -> impl Iterator<Item = FireAbility> {
    hero.abilities()
        .iter()
        .copied()
        .filter(|ability| ability.is_assignable())
}

/// How many spells the picker has to make room for.
#[must_use]
pub fn assignable_count(hero: Hero) -> usize {
    assignable_spells(hero).count()
}

/// The most spells any hero can offer, which is how many buttons the picker is built with.
///
/// Built for the largest rather than rebuilt per hero: a screen in this crate is a list of
/// rectangles created once, and a hero with three abilities simply lays out three of them.
const MAX_ASSIGNABLE: usize = engine_game::firemage::ABILITY_SLOTS;

/// Every measurement, converted from its design size to physical pixels once.
///
/// The same arrangement as the HUD's and the pause menu's, and for the same reason:
/// converting in one place is what stops a measurement being scaled twice or not at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotMenuMetrics {
    /// The scale these were derived with.
    pub scale: UiScale,
    /// `layout::PANEL_WIDTH` in physical pixels.
    pub panel_width: f32,
    /// `layout::ROW_HEIGHT` in physical pixels.
    pub row_height: f32,
    /// `layout::ROW_GAP` in physical pixels.
    pub row_gap: f32,
    /// `layout::PADDING` in physical pixels.
    pub padding: f32,
    /// `layout::HEADING` in physical pixels.
    pub heading: f32,
    /// `layout::PICK_SIZE` in physical pixels.
    pub pick_size: f32,
    /// `layout::PICK_GAP` in physical pixels.
    pub pick_gap: f32,
}

impl SlotMenuMetrics {
    /// The menu's measurements at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            scale,
            panel_width: scale.px(layout::PANEL_WIDTH),
            row_height: scale.px(layout::ROW_HEIGHT),
            row_gap: scale.px(layout::ROW_GAP),
            padding: scale.px(layout::PADDING),
            heading: scale.px(layout::HEADING),
            pick_size: scale.px(layout::PICK_SIZE),
            pick_gap: scale.px(layout::PICK_GAP),
        }
    }
}

impl Default for SlotMenuMetrics {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
}

/// The slot menu: which slot it is about, what it is showing, and its buttons.
#[derive(Debug)]
pub struct SlotMenu {
    address: Option<SlotAddress>,
    mode: SlotMenuMode,
    /// The slot the menu was opened on, in physical pixels.
    ///
    /// The rectangle rather than a corner, because the panel hangs *upwards* from it and its
    /// height depends on which mode is showing: keeping the slot means the panel can be
    /// placed again from scratch every time the mode changes, instead of the origin having to
    /// be moved by the difference between two heights.
    anchor: UiRect,
    actions: UiScreen<SlotMenuItem>,
    picker: UiScreen<usize>,
    confirm: UiScreen<ConfirmItem>,
    metrics: SlotMenuMetrics,
    viewport: (u32, u32),
    /// Which hero's spells the picker offers.
    hero: Hero,
}

impl Default for SlotMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl SlotMenu {
    /// A closed menu with its buttons built.
    ///
    /// Built once, like every other screen in this crate: opening the menu allocates nothing
    /// and creates no GPU resource, because a screen is a list of rectangles.
    #[must_use]
    pub fn new() -> Self {
        let mut actions = UiScreen::new();
        actions.push(SlotMenuItem::ChangeSpell, Button::new("CHANGE SPELL"));
        actions.push(SlotMenuItem::AssignKey, Button::new("ASSIGN KEY"));
        actions.push(SlotMenuItem::ClearSlot, Button::new("CLEAR SLOT"));
        actions.push(SlotMenuItem::Close, Button::new("CLOSE"));

        let mut picker = UiScreen::new();
        for index in 0..MAX_ASSIGNABLE {
            // No label: the picker draws the spell's art, and a name under a 48-pixel icon
            // in a 5x7 font would be four characters of nothing useful.
            picker.push(index, Button::new(""));
        }

        let mut confirm = UiScreen::new();
        confirm.push(ConfirmItem::Cancel, Button::new("CANCEL"));
        confirm.push(ConfirmItem::Replace, Button::new("REPLACE"));

        Self {
            address: None,
            mode: SlotMenuMode::Actions,
            anchor: UiRect::new(0.0, 0.0, 0.0, 0.0),
            actions,
            picker,
            confirm,
            metrics: SlotMenuMetrics::default(),
            viewport: (0, 0),
            hero: Hero::default(),
        }
    }

    /// Whether the menu is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.address.is_some()
    }

    /// Which slot the menu is about.
    #[must_use]
    pub fn address(&self) -> Option<SlotAddress> {
        self.address
    }

    /// What the menu is showing.
    #[must_use]
    pub fn mode(&self) -> SlotMenuMode {
        self.mode
    }

    /// The measurements the menu is laid out with.
    #[must_use]
    pub fn metrics(&self) -> &SlotMenuMetrics {
        &self.metrics
    }

    /// Lays the menu out at a new interface size from here on.
    pub fn set_scale(&mut self, scale: UiScale) {
        if scale == self.metrics.scale {
            return;
        }
        self.metrics = SlotMenuMetrics::new(scale);
        self.layout();
    }

    /// The actions screen's widgets, for drawing.
    #[must_use]
    pub fn actions_screen(&self) -> &UiScreen<SlotMenuItem> {
        &self.actions
    }

    /// The picker's widgets, for drawing.
    #[must_use]
    pub fn picker_screen(&self) -> &UiScreen<usize> {
        &self.picker
    }

    /// The confirmation's widgets, for drawing.
    #[must_use]
    pub fn confirm_screen(&self) -> &UiScreen<ConfirmItem> {
        &self.confirm
    }

    /// Opens the menu for `address`, with its panel centred over `slot` and rising from it.
    ///
    /// Upwards rather than downwards: a bar is nearly always at the bottom of the screen,
    /// which is where the genre puts it and where this game's default layout has it, so a
    /// panel that grew downwards from the slot would spend its whole life clamped against the
    /// bottom edge and covering the bar it was opened from. The panel's bottom edge sits on
    /// the slot's top edge and its centre on the slot's, so the menu reads as belonging to the
    /// cell that was clicked and leaves that cell visible. It is still clamped into the
    /// viewport afterwards, so a bar dragged to the top opens downwards instead of off-screen.
    pub fn open(&mut self, address: SlotAddress, slot: UiRect, viewport: (u32, u32)) {
        if !address.is_valid() {
            return;
        }
        self.address = Some(address);
        self.mode = SlotMenuMode::Actions;
        self.viewport = viewport;
        self.anchor = slot;
        self.actions.reset();
        self.picker.reset();
        self.confirm.reset();
        self.layout();
    }

    /// Which hero's spells the picker is offering.
    #[must_use]
    pub fn hero(&self) -> Hero {
        self.hero
    }

    /// Offers `hero`'s spells from here on, and closes the menu if one is open.
    ///
    /// Closed rather than relaid out: the menu is opened on a slot, and a hero swap has just
    /// changed what that slot holds. Re-opening it is one right click.
    pub fn set_hero(&mut self, hero: Hero) {
        if self.hero == hero {
            return;
        }
        self.hero = hero;
        self.close();
        self.layout();
    }

    /// Closes the menu.
    pub fn close(&mut self) {
        self.address = None;
        self.mode = SlotMenuMode::Actions;
        self.actions.reset();
        self.picker.reset();
        self.confirm.reset();
    }

    /// The panel the current mode draws in.
    #[must_use]
    pub fn panel(&self) -> UiRect {
        let metrics = &self.metrics;
        let (width, height) = match self.mode {
            SlotMenuMode::Actions => (
                metrics.panel_width,
                metrics.heading
                    + metrics.row_height * 4.0
                    + metrics.row_gap * 3.0
                    + metrics.padding * 2.0,
            ),
            SlotMenuMode::Picker => {
                let rows = assignable_count(self.hero).div_ceil(layout::PICK_COLUMNS) as f32;
                let columns = layout::PICK_COLUMNS as f32;
                (
                    metrics.padding * 2.0
                        + columns * metrics.pick_size
                        + (columns - 1.0) * metrics.pick_gap,
                    metrics.padding * 2.0
                        + metrics.heading
                        + rows * metrics.pick_size
                        + (rows - 1.0) * metrics.pick_gap,
                )
            }
            SlotMenuMode::Capture | SlotMenuMode::Refused(_) => (
                metrics.panel_width,
                metrics.heading + metrics.row_height + metrics.padding * 2.0,
            ),
            SlotMenuMode::Confirm { .. } => (
                metrics.panel_width,
                metrics.heading
                    + metrics.row_height * 2.0
                    + metrics.row_gap
                    + metrics.padding * 2.0,
            ),
        };
        super::hud_layout::clamp_on_screen(
            UiRect::new(
                self.anchor.x + (self.anchor.width - width) * 0.5,
                self.anchor.y - height,
                width,
                height,
            ),
            self.viewport,
        )
    }

    /// Positions every widget for the current mode.
    fn layout(&mut self) {
        let panel = self.panel();
        let metrics = self.metrics;
        let x = panel.x + metrics.padding;
        let width = panel.width - metrics.padding * 2.0;
        let top = panel.y + metrics.padding + metrics.heading;

        for (index, item) in [
            SlotMenuItem::ChangeSpell,
            SlotMenuItem::AssignKey,
            SlotMenuItem::ClearSlot,
            SlotMenuItem::Close,
        ]
        .into_iter()
        .enumerate()
        {
            let y = top + index as f32 * (metrics.row_height + metrics.row_gap);
            self.actions
                .set_rect(item, UiRect::new(x, y, width, metrics.row_height));
        }

        for index in 0..assignable_count(self.hero) {
            let column = index % layout::PICK_COLUMNS;
            let row = index / layout::PICK_COLUMNS;
            self.picker.set_rect(
                index,
                UiRect::new(
                    x + column as f32 * (metrics.pick_size + metrics.pick_gap),
                    top + row as f32 * (metrics.pick_size + metrics.pick_gap),
                    metrics.pick_size,
                    metrics.pick_size,
                ),
            );
        }

        let half = (width - metrics.row_gap) * 0.5;
        self.confirm.set_rect(
            ConfirmItem::Cancel,
            UiRect::new(x, top, half, metrics.row_height),
        );
        self.confirm.set_rect(
            ConfirmItem::Replace,
            UiRect::new(x + half + metrics.row_gap, top, half, metrics.row_height),
        );
    }

    /// Applies one frame of input and returns what the application should do about it.
    ///
    /// `bars` and `bindings` are read, never written: the menu asks them what is in the slot
    /// and who holds a key, and hands the answer back as an outcome.
    pub fn update(
        &mut self,
        input: &UiInput,
        bars: &ActionBarSet,
        bindings: &Bindings,
    ) -> SlotMenuOutcome {
        let Some(address) = self.address else {
            return SlotMenuOutcome::Idle;
        };
        // Escape backs out of exactly one thing, the way it does everywhere else in this
        // interface: out of the picker or the capture to the actions, and out of the actions
        // to the game.
        if input.cancel {
            return match self.mode {
                SlotMenuMode::Actions => {
                    self.close();
                    SlotMenuOutcome::Closed
                }
                _ => {
                    self.mode = SlotMenuMode::Actions;
                    self.layout();
                    SlotMenuOutcome::Idle
                }
            };
        }

        // A click outside the panel dismisses the menu, the way every context menu in the
        // genre behaves. It is checked before the modes so that no mode has to repeat it, and
        // after escape so that the two ways out stay one decision apart.
        if (input.pointer_pressed || input.secondary_pressed)
            && !self.panel().contains(input.pointer.0, input.pointer.1)
        {
            self.close();
            return SlotMenuOutcome::Closed;
        }

        match self.mode {
            SlotMenuMode::Actions => self.update_actions(input, address),
            SlotMenuMode::Picker => self.update_picker(input, address),
            SlotMenuMode::Capture => self.update_capture(input, address, bars, bindings),
            SlotMenuMode::Refused(_) => {
                // Any click dismisses the refusal and goes back to the actions.
                if input.pointer_pressed {
                    self.mode = SlotMenuMode::Actions;
                    self.layout();
                }
                SlotMenuOutcome::Idle
            }
            SlotMenuMode::Confirm { key, .. } => self.update_confirm(input, address, key),
        }
    }

    fn update_actions(&mut self, input: &UiInput, address: SlotAddress) -> SlotMenuOutcome {
        let mut chosen = None;
        for (item, event) in self.actions.update(input) {
            if *event == WidgetEvent::Activated {
                chosen = Some(*item);
            }
        }
        match chosen {
            Some(SlotMenuItem::ChangeSpell) => {
                self.mode = SlotMenuMode::Picker;
                self.picker.reset();
                self.layout();
                SlotMenuOutcome::Idle
            }
            Some(SlotMenuItem::AssignKey) => {
                self.mode = SlotMenuMode::Capture;
                self.layout();
                SlotMenuOutcome::Idle
            }
            Some(SlotMenuItem::ClearSlot) => {
                self.close();
                SlotMenuOutcome::ClearSlot(address)
            }
            Some(SlotMenuItem::Close) => {
                self.close();
                SlotMenuOutcome::Closed
            }
            None => SlotMenuOutcome::Idle,
        }
    }

    fn update_picker(&mut self, input: &UiInput, address: SlotAddress) -> SlotMenuOutcome {
        let mut chosen = None;
        for (index, event) in self.picker.update(input) {
            if *event == WidgetEvent::Activated {
                chosen = Some(*index);
            }
        }
        let Some(index) = chosen else {
            return SlotMenuOutcome::Idle;
        };
        let Some(ability) = assignable_spells(self.hero).nth(index) else {
            return SlotMenuOutcome::Idle;
        };
        self.close();
        SlotMenuOutcome::AssignSpell(address, Some(ability))
    }

    fn update_capture(
        &mut self,
        input: &UiInput,
        address: SlotAddress,
        bars: &ActionBarSet,
        bindings: &Bindings,
    ) -> SlotMenuOutcome {
        let Some(key) = input.captured_key else {
            return SlotMenuOutcome::Idle;
        };
        match bars.conflict(address, key, bindings) {
            Some(KeyConflict::Reserved(what)) => {
                self.mode = SlotMenuMode::Refused(what);
                self.layout();
                SlotMenuOutcome::Idle
            }
            Some(KeyConflict::Slot(holder)) => {
                self.mode = SlotMenuMode::Confirm { key, holder };
                self.confirm.reset();
                self.layout();
                SlotMenuOutcome::Idle
            }
            None => {
                self.close();
                SlotMenuOutcome::AssignKey(address, key)
            }
        }
    }

    fn update_confirm(
        &mut self,
        input: &UiInput,
        address: SlotAddress,
        key: KeyChord,
    ) -> SlotMenuOutcome {
        let mut chosen = None;
        for (item, event) in self.confirm.update(input) {
            if *event == WidgetEvent::Activated {
                chosen = Some(*item);
            }
        }
        match chosen {
            Some(ConfirmItem::Replace) => {
                self.close();
                SlotMenuOutcome::AssignKey(address, key)
            }
            Some(ConfirmItem::Cancel) => {
                self.mode = SlotMenuMode::Actions;
                self.layout();
                SlotMenuOutcome::Idle
            }
            None => SlotMenuOutcome::Idle,
        }
    }
}

/// Draws the slot menu: its panel, its heading, and whichever mode it is in.
pub fn push_slot_menu(
    list: &mut UiDrawList,
    menu: &SlotMenu,
    bars: &ActionBarSet,
    theme: &super::paint::MenuTheme,
    icon_theme: &crate::game_ui::UiTheme,
    scratch: &mut String,
    scale: UiScale,
) {
    let Some(address) = menu.address() else {
        return;
    };
    let panel = menu.panel();
    let border = scale.px(theme.border_pixels);
    let text = scale.text(theme.label_scale);
    list.rect(panel, theme.panel);
    list.outline(panel, border, theme.panel_border);

    // The heading: which slot this is, and what is in it. Two lines, because a player who
    // right-clicked the wrong slot needs to be able to see that before they change it.
    let inset = border * 3.0;
    address.write_label(scratch);
    list.text_at(
        (panel.x + inset, panel.y + inset),
        scratch,
        theme.title,
        text,
    );

    let contents = bars.slot(address).unwrap_or_default();
    scratch.clear();
    match contents.ability {
        Some(ability) => scratch.push_str(ability.definition().name),
        None => scratch.push_str("EMPTY"),
    }
    scratch.push_str("  KEY ");
    match contents.key {
        Some(key) => key.write_label(scratch),
        None => scratch.push_str("NONE"),
    }
    list.text_at(
        (panel.x + inset, panel.y + inset + text * 9.0),
        scratch,
        theme.text,
        text,
    );

    match menu.mode() {
        SlotMenuMode::Actions => {
            for (_, widget) in menu.actions_screen().widgets() {
                super::paint::push_widget(list, widget, theme, scale);
            }
        }
        SlotMenuMode::Picker => {
            for (index, ability) in assignable_spells(menu.hero()).enumerate() {
                let Some(widget) = menu.picker_screen().widget(index) else {
                    continue;
                };
                let rect = widget.rect();
                let state = widget.state();
                list.rect(rect, theme.button);
                if let Some(icon) = crate::game_ui::icon_for(ability) {
                    list.icon(rect.inset(border), icon, icon_theme.icon);
                }
                list.outline(
                    rect,
                    border,
                    if state.hovered {
                        theme.title
                    } else {
                        theme.panel_border
                    },
                );
            }
        }
        SlotMenuMode::Capture => {
            list.text_at(
                (panel.x + inset, panel.y + scale.px(layout::HEADING)),
                "PRESS A KEY...",
                theme.title,
                text,
            );
            // Smaller, and on its own line: the whole sentence at the heading's size is
            // wider than the panel, and the panel is the width every other mode is.
            list.text_at(
                (
                    panel.x + inset,
                    panel.y + scale.px(layout::HEADING) + text * 10.0,
                ),
                "HOLD CTRL/SHIFT/ALT TOO",
                theme.text,
                text * 0.75,
            );
        }
        SlotMenuMode::Refused(what) => {
            scratch.clear();
            scratch.push_str("USED BY ");
            scratch.push_str(what);
            list.text_at(
                (panel.x + inset, panel.y + scale.px(layout::HEADING)),
                scratch,
                theme.title,
                text,
            );
        }
        SlotMenuMode::Confirm { key, holder } => {
            scratch.clear();
            key.write_label(scratch);
            scratch.push_str(" IS ON ");
            let mut owner = String::new();
            holder.write_label(&mut owner);
            scratch.push_str(&owner);
            list.text_at(
                (panel.x + inset, panel.y + scale.px(layout::HEADING) * 0.6),
                scratch,
                theme.title,
                text,
            );
            for (_, widget) in menu.confirm_screen().widgets() {
                super::paint::push_widget(list, widget, theme, scale);
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::input::{KeyCode, Modifiers};

    const FHD: (u32, u32) = (1920, 1080);
    const SLOT: UiRect = UiRect {
        x: 800.0,
        y: 900.0,
        width: 48.0,
        height: 48.0,
    };

    fn menu() -> SlotMenu {
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(0, 0), SLOT, FHD);
        menu
    }

    /// One frame of a click on `rect`: press, then release, both inside it.
    fn click(menu: &mut SlotMenu, rect: UiRect, bars: &ActionBarSet) -> SlotMenuOutcome {
        let bindings = Bindings::default();
        let centre = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        let press = UiInput {
            pointer: centre,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        };
        menu.update(&press, bars, &bindings);
        let release = UiInput {
            pointer: centre,
            pointer_released: true,
            ..UiInput::new()
        };
        menu.update(&release, bars, &bindings)
    }

    /// Clicks one action button by name.
    fn click_action(
        menu: &mut SlotMenu,
        item: SlotMenuItem,
        bars: &ActionBarSet,
    ) -> SlotMenuOutcome {
        let rect = action_rect(menu, item);
        click(menu, rect, bars)
    }

    /// The rectangle of one action button.
    fn action_rect(menu: &SlotMenu, item: SlotMenuItem) -> UiRect {
        menu.actions_screen()
            .widget(item)
            .expect("the action exists")
            .rect()
    }

    fn press_key(menu: &mut SlotMenu, key: KeyChord, bars: &ActionBarSet) -> SlotMenuOutcome {
        let input = UiInput {
            captured_key: Some(key),
            ..UiInput::new()
        };
        menu.update(&input, bars, &Bindings::default())
    }

    #[test]
    fn the_menu_opens_on_a_slot_and_closes_again() {
        let mut menu = SlotMenu::new();
        assert!(!menu.is_open());

        menu.open(SlotAddress::new(2, 5), SLOT, FHD);
        assert!(menu.is_open());
        assert_eq!(menu.address(), Some(SlotAddress::new(2, 5)));
        assert_eq!(menu.mode(), SlotMenuMode::Actions);

        menu.close();
        assert!(!menu.is_open());
        assert_eq!(menu.address(), None);
    }

    #[test]
    fn a_closed_menu_ignores_everything() {
        let mut menu = SlotMenu::new();
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        assert_eq!(
            click(&mut menu, SLOT, &bars),
            SlotMenuOutcome::Idle,
            "a closed menu acted on a click"
        );
        assert_eq!(
            press_key(&mut menu, KeyChord::plain(KeyCode::F1), &bars),
            SlotMenuOutcome::Idle
        );
    }

    #[test]
    fn a_click_outside_the_panel_closes_the_menu() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        for mode_opener in [None, Some(SlotMenuItem::ChangeSpell)] {
            let mut menu = menu();
            if let Some(item) = mode_opener {
                click_action(&mut menu, item, &bars);
            }
            let panel = menu.panel();
            let outside = UiInput {
                pointer: (panel.x - 40.0, panel.y - 40.0),
                pointer_down: true,
                pointer_pressed: true,
                ..UiInput::new()
            };
            assert_eq!(
                menu.update(&outside, &bars, &Bindings::default()),
                SlotMenuOutcome::Closed
            );
            assert!(!menu.is_open(), "a click outside left the menu open");
        }
    }

    #[test]
    fn a_right_click_outside_the_panel_closes_the_menu_too() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        let panel = menu.panel();
        let outside = UiInput {
            pointer: (panel.right() + 40.0, panel.bottom() + 40.0),
            secondary_pressed: true,
            ..UiInput::new()
        };
        assert_eq!(
            menu.update(&outside, &bars, &Bindings::default()),
            SlotMenuOutcome::Closed
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn a_click_inside_the_panel_but_on_no_button_keeps_the_menu_open() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        let panel = menu.panel();
        // The heading, which is inside the panel and belongs to no widget.
        let inside = UiInput {
            pointer: (panel.x + panel.width * 0.5, panel.y + 2.0),
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        };
        assert_eq!(
            menu.update(&inside, &bars, &Bindings::default()),
            SlotMenuOutcome::Idle
        );
        assert!(menu.is_open(), "a click on the panel itself closed it");
    }

    #[test]
    fn the_panel_rises_from_the_slot_rather_than_hanging_below_it() {
        // A slot near the bottom of the screen, where a bar actually lives.
        let slot = UiRect::new(800.0, 900.0, 48.0, 48.0);
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(0, 0), slot, FHD);
        let panel = menu.panel();
        assert!(
            panel.y < slot.y,
            "the panel started at {} rather than above the slot at {}",
            panel.y,
            slot.y
        );
        assert!(
            (panel.bottom() - slot.y).abs() < 0.5,
            "the panel's bottom edge left the slot's top"
        );
        assert!(
            (panel.x + panel.width * 0.5 - (slot.x + slot.width * 0.5)).abs() < 0.5,
            "the panel is not centred on the slot"
        );
    }

    #[test]
    fn the_panel_still_rises_from_the_slot_after_the_mode_changes_its_height() {
        let slot = UiRect::new(800.0, 900.0, 48.0, 48.0);
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(0, 0), slot, FHD);
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let actions = menu.panel();
        click_action(&mut menu, SlotMenuItem::ChangeSpell, &bars);
        let picker = menu.panel();
        assert_eq!(menu.mode(), SlotMenuMode::Picker);
        assert!(
            (picker.bottom() - actions.bottom()).abs() < 0.5,
            "the taller picker grew downwards instead of upwards"
        );
    }

    #[test]
    fn a_combination_is_captured_with_the_key_that_was_pressed() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);
        assert_eq!(menu.mode(), SlotMenuMode::Capture);

        let chord = KeyChord::new(KeyCode::F9, Modifiers::new(true, true, false));
        assert_eq!(
            press_key(&mut menu, chord, &bars),
            SlotMenuOutcome::AssignKey(SlotAddress::new(0, 0), chord)
        );
    }

    #[test]
    fn a_combination_over_a_key_another_slot_holds_bare_is_not_a_conflict() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(1, 0), SLOT, FHD);
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);

        // Bar one slot one holds plain `1`; `CTRL+1` is a different binding entirely.
        let chord = KeyChord::new(KeyCode::Digit1, Modifiers::new(true, false, false));
        assert_eq!(
            press_key(&mut menu, chord, &bars),
            SlotMenuOutcome::AssignKey(SlotAddress::new(1, 0), chord)
        );
    }

    #[test]
    fn the_menu_never_opens_off_the_edge_of_the_screen() {
        let mut menu = SlotMenu::new();
        // The last slot of a bar pushed into the bottom-right corner.
        menu.open(
            SlotAddress::new(0, 9),
            UiRect::new(1900.0, 1060.0, 48.0, 48.0),
            FHD,
        );
        let panel = menu.panel();
        assert!(panel.x < FHD.0 as f32, "{panel:?}");
        assert!(panel.y < FHD.1 as f32, "{panel:?}");
        // And its buttons came with it.
        let close = action_rect(&menu, SlotMenuItem::Close);
        assert!(
            close.x >= panel.x && close.right() <= panel.right(),
            "{close:?}"
        );
    }

    #[test]
    fn picking_a_spell_assigns_it_and_closes() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::ChangeSpell, &bars);
        assert_eq!(menu.mode(), SlotMenuMode::Picker);

        let third = assignable_spells(Hero::default())
            .nth(2)
            .expect("a third spell");
        let rect = menu
            .picker_screen()
            .widget(2)
            .expect("a picker entry")
            .rect();
        assert_eq!(
            click(&mut menu, rect, &bars),
            SlotMenuOutcome::AssignSpell(SlotAddress::new(0, 0), Some(third))
        );
        assert!(!menu.is_open(), "the menu stayed open after a choice");
    }

    #[test]
    fn the_picker_offers_every_castable_spell_and_nothing_else() {
        let listed: Vec<_> = assignable_spells(Hero::default()).collect();
        for ability in Hero::default().abilities() {
            assert!(listed.contains(ability), "{ability:?} is not offered");
        }
        assert!(!listed.contains(&FireAbility::Ignite));
        assert!(!listed.contains(&FireAbility::FireBeam));
        assert_eq!(listed.len(), assignable_count(Hero::default()));
        // Every entry the picker draws corresponds to one of them.
        let menu = menu();
        assert_eq!(menu.picker_screen().len(), listed.len());
    }

    #[test]
    fn every_picker_entry_is_inside_the_panel_and_none_overlap() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::ChangeSpell, &bars);
        let panel = menu.panel();

        let rects: Vec<UiRect> = (0..assignable_count(Hero::default()))
            .map(|index| {
                menu.picker_screen()
                    .widget(index)
                    .expect("a picker entry")
                    .rect()
            })
            .collect();
        for (index, rect) in rects.iter().enumerate() {
            assert!(
                rect.x >= panel.x - 0.01
                    && rect.y >= panel.y - 0.01
                    && rect.right() <= panel.right() + 0.01
                    && rect.bottom() <= panel.bottom() + 0.01,
                "entry {index} at {rect:?} escapes {panel:?}"
            );
            for (other_index, other) in rects.iter().enumerate().skip(index + 1) {
                let overlaps = rect.x < other.right()
                    && other.x < rect.right()
                    && rect.y < other.bottom()
                    && other.y < rect.bottom();
                assert!(!overlaps, "entries {index} and {other_index} overlap");
            }
        }
    }

    #[test]
    fn clearing_a_slot_reports_it_and_closes() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        assert_eq!(
            click_action(&mut menu, SlotMenuItem::ClearSlot, &bars),
            SlotMenuOutcome::ClearSlot(SlotAddress::new(0, 0))
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn closing_changes_nothing() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        assert_eq!(
            click_action(&mut menu, SlotMenuItem::Close, &bars),
            SlotMenuOutcome::Closed
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn capturing_a_free_key_assigns_it() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);
        assert_eq!(menu.mode(), SlotMenuMode::Capture);

        assert_eq!(
            press_key(&mut menu, KeyChord::plain(KeyCode::F9), &bars),
            SlotMenuOutcome::AssignKey(SlotAddress::new(0, 0), KeyChord::plain(KeyCode::F9))
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn escape_backs_out_of_a_capture_without_binding_anything() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);

        let escape = UiInput {
            cancel: true,
            ..UiInput::new()
        };
        assert_eq!(
            menu.update(&escape, &bars, &Bindings::default()),
            SlotMenuOutcome::Idle
        );
        assert_eq!(menu.mode(), SlotMenuMode::Actions, "escape bound the key");
        assert!(menu.is_open(), "escape closed the whole menu at once");

        // A second escape leaves the menu, which is the chain the rest of the UI uses.
        assert_eq!(
            menu.update(&escape, &bars, &Bindings::default()),
            SlotMenuOutcome::Closed
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn a_key_the_game_needs_is_refused_and_says_why() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);

        assert_eq!(
            press_key(&mut menu, KeyChord::plain(KeyCode::W), &bars),
            SlotMenuOutcome::Idle
        );
        match menu.mode() {
            SlotMenuMode::Refused(what) => assert!(!what.is_empty(), "no reason given"),
            other => panic!("{other:?} rather than a refusal"),
        }
        assert!(menu.is_open(), "the refusal closed the menu");
    }

    #[test]
    fn a_key_another_slot_holds_asks_before_taking_it() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(1, 0), SLOT, FHD);
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);

        // `1` belongs to bar one, slot one.
        assert_eq!(
            press_key(&mut menu, KeyChord::plain(KeyCode::Digit1), &bars),
            SlotMenuOutcome::Idle,
            "the key was taken without asking"
        );
        assert_eq!(
            menu.mode(),
            SlotMenuMode::Confirm {
                key: KeyChord::plain(KeyCode::Digit1),
                holder: SlotAddress::new(0, 0),
            }
        );
    }

    #[test]
    fn cancelling_the_prompt_leaves_the_binding_where_it_was() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(1, 0), SLOT, FHD);
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);
        press_key(&mut menu, KeyChord::plain(KeyCode::Digit1), &bars);

        let cancel = menu
            .confirm_screen()
            .widget(ConfirmItem::Cancel)
            .expect("a cancel button")
            .rect();
        assert_eq!(click(&mut menu, cancel, &bars), SlotMenuOutcome::Idle);
        assert_eq!(menu.mode(), SlotMenuMode::Actions);
        // And the model was never touched, because the menu does not touch it.
        assert_eq!(
            bars.slot_for_key(KeyChord::plain(KeyCode::Digit1)),
            Some(SlotAddress::new(0, 0))
        );
    }

    #[test]
    fn replacing_hands_the_key_over_exactly_once() {
        let mut bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = SlotMenu::new();
        menu.open(SlotAddress::new(1, 0), SLOT, FHD);
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);
        press_key(&mut menu, KeyChord::plain(KeyCode::Digit1), &bars);

        let replace = menu
            .confirm_screen()
            .widget(ConfirmItem::Replace)
            .expect("a replace button")
            .rect();
        let outcome = click(&mut menu, replace, &bars);
        assert_eq!(
            outcome,
            SlotMenuOutcome::AssignKey(SlotAddress::new(1, 0), KeyChord::plain(KeyCode::Digit1))
        );
        assert!(!menu.is_open());

        // The application carries it out, and the model refuses to hold a duplicate.
        if let SlotMenuOutcome::AssignKey(address, key) = outcome {
            assert!(bars.assign_key(address, key, &Bindings::default()));
        }
        assert_eq!(
            bars.slot_for_key(KeyChord::plain(KeyCode::Digit1)),
            Some(SlotAddress::new(1, 0))
        );
        assert_eq!(
            bars.slot(SlotAddress::new(0, 0)).expect("bar one").key,
            None
        );
    }

    #[test]
    fn rebinding_a_slot_to_the_key_it_already_has_is_not_a_conflict() {
        let bars = ActionBarSet::with_default_bindings(Hero::default());
        let mut menu = menu();
        click_action(&mut menu, SlotMenuItem::AssignKey, &bars);
        assert_eq!(
            press_key(&mut menu, KeyChord::plain(KeyCode::Digit1), &bars),
            SlotMenuOutcome::AssignKey(SlotAddress::new(0, 0), KeyChord::plain(KeyCode::Digit1))
        );
    }

    #[test]
    fn the_menu_follows_the_interface_size() {
        let mut menu = menu();
        let small = menu.panel();
        menu.set_scale(UiScale::new(2.0));
        let large = menu.panel();
        assert!(large.width > small.width, "{small:?} -> {large:?}");
        // And its buttons moved with it rather than staying where they were drawn.
        let close = action_rect(&menu, SlotMenuItem::Close);
        assert!(
            close.x >= large.x && close.bottom() <= large.bottom() + 0.01,
            "{close:?} is not inside {large:?}"
        );
    }
}
