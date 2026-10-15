//! The Modifier panel: the key's modifiers while a dungeon runs, and the reroll.
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │ Key Modifiers  · level 5               [x]│
//! │ [icon] Rotating Beams  +0%   [ Reroll ]  │
//! │        Beams sweep out from the centre …  │
//! │ [icon] Plague Explosion +0%  [ Reroll ]  │
//! │ ▓▓▓▓▓▓▓▓▓░░░░░░|░░░░░ Dungeon cleared 42% │
//! │ Reroll unlocks at 50%                     │
//! │ +25% loot bonus active                    │
//! │ ROLLING...  60% Strengthen / 40% Weaken   │  ← then "Result: +17%  Strengthened"
//! └──────────────────────────────────────────┘
//! ```
//!
//! The panel owns nothing the server decides. The application hands it a [`ModifierPanelView`]
//! built from the realm's rows (the instance's modifiers, the kill progress, the player's
//! currency) and it draws that. A press on a Reroll button leaves as
//! [`ModifierOutcome::Reroll`]; the server decides whether it happens, rolls the dice and
//! writes the answer into the instance row. When that row's reroll counter grows the
//! application calls [`ModifierWindow::start_reveal`] with the server's result, and the window
//! only animates it: a short "rolling" phase, then the result it was told.
//!
//! Like Recount it is a floating window, not a modal one: [`ModifierWindow::update`] says
//! `Consumed` while the pointer is over it so a click does not also land in the world.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};

use super::input::UiInput;
use super::kit::{self, ButtonVariant, Interaction, Kit};
use super::map_style::{color, radius, type_scale};

/// Layout, in design pixels.
mod layout {
    pub const WIDTH: f32 = 380.0;
    pub const PADDING: f32 = 12.0;
    pub const HEADER: f32 = 34.0;
    pub const ROW: f32 = 76.0;
    pub const ROW_GAP: f32 = 6.0;
    pub const EMPTY_ROW: f32 = 34.0;
    pub const ICON: f32 = 44.0;
    pub const BUTTON_WIDTH: f32 = 76.0;
    pub const BUTTON_HEIGHT: f32 = 24.0;
    pub const BAR: f32 = 12.0;
    pub const LINE: f32 = 20.0;
    pub const REVEAL: f32 = 64.0;
    pub const MARGIN: f32 = 16.0;
    pub const CLOSE: f32 = 22.0;
}

/// How long the dice "roll" before the result shows, seconds.
pub const ROLL_SECONDS: f32 = 1.4;
/// How long the result stays after the roll, seconds.
pub const RESULT_SECONDS: f32 = 6.0;
/// Ticks per second while the dice roll.
const TICKS_PER_SECOND: f32 = 10.0;

/// One modifier on the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifierLine {
    /// Its picture.
    pub icon: UiIcon,
    /// Its name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// The reroll magnitude it has, per cent (positive: stronger than designed).
    pub magnitude_pct: i16,
}

/// Everything the panel shows, built by the application from the realm's state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModifierPanelView {
    /// The key's level.
    pub key_level: u32,
    /// Its modifiers, as they stand now.
    pub lines: Vec<ModifierLine>,
    /// Placed monsters dead.
    pub killed: u32,
    /// Placed monsters.
    pub total: u32,
    /// The kill share a reroll needs, per cent.
    pub unlock_pct: u32,
    /// Rerolls the run still allows.
    pub rerolls_left: u32,
    /// What one reroll costs.
    pub cost: u32,
    /// The currency's display name ("Aether Shard").
    pub resource: String,
    /// How much of it the local player can still spend this run.
    pub available: u64,
    /// The reward bonus a reroll gave the run, per cent; zero before one.
    pub reward_bonus_pct: u32,
}

impl ModifierPanelView {
    /// How much of the dungeon is cleared, `0..=1`.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.killed as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }

    /// Whether enough of the dungeon is cleared for a reroll.
    #[must_use]
    pub fn unlocked(&self) -> bool {
        self.total > 0
            && u64::from(self.killed) * 100 >= u64::from(self.unlock_pct) * u64::from(self.total)
    }

    /// Whether the local player could press Reroll right now.
    #[must_use]
    pub fn can_reroll(&self) -> bool {
        self.unlocked() && self.rerolls_left > 0 && self.available >= u64::from(self.cost)
    }
}

/// What the server rolled, to be shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    /// Who rerolled.
    pub by: String,
    /// Which modifier, by display name.
    pub modifier: String,
    /// The change the dice made, per cent (positive: stronger).
    pub delta_pct: i16,
    /// The chance of a strengthen, per cent, for the rolling line ("60% Strengthen").
    pub strengthen_pct: u32,
}

impl Reveal {
    /// Whether the result strengthened the modifier.
    #[must_use]
    pub fn strengthened(&self) -> bool {
        self.delta_pct > 0
    }
}

/// What the reveal's clock crossed in one frame: for the sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RevealBeats {
    /// Dice ticks to play.
    pub ticks: u32,
    /// The result landed this frame: `Some(true)` for a strengthen, `Some(false)` for a weaken.
    pub result: Option<bool>,
}

/// What the window decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierOutcome {
    /// The pointer is not on the window.
    Idle,
    /// The pointer is on the window: the world should not also see it.
    Consumed,
    /// The close button was pressed.
    Closed,
    /// A Reroll button was pressed, for the modifier at this index.
    Reroll(usize),
}

/// The Modifier window: whether it is open, what it shows and the reveal under way.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifierWindow {
    open: bool,
    scale: UiScale,
    view: ModifierPanelView,
    reveal: Option<(Reveal, f32)>,
}

impl Default for ModifierWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ModifierWindow {
    /// Closed, with nothing to show.
    #[must_use]
    pub fn new() -> Self {
        Self::with_scale(UiScale::default())
    }

    /// Closed, laid out at `scale`.
    #[must_use]
    pub fn with_scale(scale: UiScale) -> Self {
        Self {
            open: false,
            scale,
            view: ModifierPanelView::default(),
            reveal: None,
        }
    }

    /// Re-lays the window out at `scale`.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.scale = scale;
    }

    /// Whether it is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens it.
    pub fn open(&mut self) {
        self.open = true;
    }

    /// Closes it.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Opens or closes it.
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// What it shows.
    #[must_use]
    pub fn view(&self) -> &ModifierPanelView {
        &self.view
    }

    /// Replaces what it shows. Called every frame the run is on.
    pub fn set_view(&mut self, view: ModifierPanelView) {
        self.view = view;
    }

    /// Starts the reveal of a reroll the server made, and opens the window so everyone sees it.
    pub fn start_reveal(&mut self, reveal: Reveal) {
        self.reveal = Some((reveal, 0.0));
        self.open = true;
    }

    /// The reveal under way: what it shows and how many seconds in.
    #[must_use]
    pub fn reveal(&self) -> Option<(&Reveal, f32)> {
        self.reveal
            .as_ref()
            .map(|(reveal, elapsed)| (reveal, *elapsed))
    }

    /// Whether the dice are still rolling (the result is not yet shown).
    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.reveal
            .as_ref()
            .is_some_and(|(_, elapsed)| *elapsed < ROLL_SECONDS)
    }

    /// Advances the reveal by `dt` seconds and reports what it crossed.
    pub fn advance(&mut self, dt: f32) -> RevealBeats {
        let Some((reveal, elapsed)) = self.reveal.as_mut() else {
            return RevealBeats::default();
        };
        let before = *elapsed;
        *elapsed += dt.max(0.0);
        let after = *elapsed;
        let mut beats = RevealBeats::default();
        let ticks_at = |t: f32| (t.min(ROLL_SECONDS) * TICKS_PER_SECOND).floor() as u32;
        beats.ticks = ticks_at(after).saturating_sub(ticks_at(before));
        if before < ROLL_SECONDS && after >= ROLL_SECONDS {
            beats.result = Some(reveal.strengthened());
        }
        if after >= ROLL_SECONDS + RESULT_SECONDS {
            self.reveal = None;
        }
        beats
    }

    fn kit(&self) -> Kit {
        Kit::new(self.scale)
    }

    /// The height of the lines section.
    fn lines_height(&self, kit: &Kit) -> f32 {
        if self.view.lines.is_empty() {
            return kit.px(layout::EMPTY_ROW);
        }
        let count = self.view.lines.len() as f32;
        (kit.px(layout::ROW) + kit.px(layout::ROW_GAP)) * count - kit.px(layout::ROW_GAP)
    }

    /// How many text lines the footer needs under the bar.
    fn footer_lines(&self) -> usize {
        let mut lines = 1; // the unlock / cost line
        if self.view.reward_bonus_pct > 0 {
            lines += 1;
        }
        lines
    }

    /// The window, docked to the left edge below the top of the screen.
    #[must_use]
    pub fn panel_rect(&self, viewport: (u32, u32)) -> UiRect {
        let kit = self.kit();
        let pad = kit.px(layout::PADDING);
        let mut height = kit.px(layout::HEADER)
            + pad
            + self.lines_height(&kit)
            + pad
            + kit.px(layout::BAR)
            + kit.px(8.0)
            + kit.px(layout::LINE) * self.footer_lines() as f32
            + pad;
        if self.reveal.is_some() {
            height += kit.px(layout::REVEAL) + kit.px(8.0);
        }
        UiRect::new(
            kit.px(layout::MARGIN),
            (viewport.1 as f32 * 0.22).max(kit.px(150.0)),
            kit.px(layout::WIDTH),
            height,
        )
    }

    fn close_rect(&self, panel: UiRect) -> UiRect {
        let kit = self.kit();
        let edge = kit.px(layout::CLOSE);
        UiRect::new(
            panel.right() - kit.px(layout::PADDING) - edge,
            panel.y + (kit.px(layout::HEADER) - edge) * 0.5 + kit.px(2.0),
            edge,
            edge,
        )
    }

    fn row_rect(&self, panel: UiRect, index: usize) -> UiRect {
        let kit = self.kit();
        let pad = kit.px(layout::PADDING);
        let pitch = kit.px(layout::ROW) + kit.px(layout::ROW_GAP);
        UiRect::new(
            panel.x + pad,
            panel.y + kit.px(layout::HEADER) + pad + pitch * index as f32,
            panel.width - pad * 2.0,
            kit.px(layout::ROW),
        )
    }

    /// Where line `index`'s Reroll button is.
    #[must_use]
    pub fn reroll_rect(&self, panel: UiRect, index: usize) -> UiRect {
        let kit = self.kit();
        let row = self.row_rect(panel, index);
        UiRect::new(
            row.right() - kit.px(layout::PADDING) - kit.px(layout::BUTTON_WIDTH),
            row.y + kit.px(10.0),
            kit.px(layout::BUTTON_WIDTH),
            kit.px(layout::BUTTON_HEIGHT),
        )
    }

    fn bar_rect(&self, panel: UiRect) -> UiRect {
        let kit = self.kit();
        let pad = kit.px(layout::PADDING);
        UiRect::new(
            panel.x + pad,
            panel.y + kit.px(layout::HEADER) + pad + self.lines_height(&kit) + pad,
            panel.width - pad * 2.0,
            kit.px(layout::BAR),
        )
    }

    fn reveal_rect(&self, panel: UiRect) -> UiRect {
        let kit = self.kit();
        let pad = kit.px(layout::PADDING);
        UiRect::new(
            panel.x + pad,
            panel.bottom() - pad - kit.px(layout::REVEAL),
            panel.width - pad * 2.0,
            kit.px(layout::REVEAL),
        )
    }

    /// Whether `pointer` is over the open window.
    #[must_use]
    pub fn wants_pointer(&self, pointer: (f32, f32), viewport: (u32, u32)) -> bool {
        self.open && self.panel_rect(viewport).contains(pointer.0, pointer.1)
    }

    /// Handles this frame's pointer.
    pub fn update(&mut self, input: &UiInput, viewport: (u32, u32)) -> ModifierOutcome {
        if !self.open {
            return ModifierOutcome::Idle;
        }
        let panel = self.panel_rect(viewport);
        let (x, y) = input.pointer;
        if !panel.contains(x, y) {
            return ModifierOutcome::Idle;
        }
        if !input.pointer_pressed {
            return ModifierOutcome::Consumed;
        }
        if self.close_rect(panel).contains(x, y) {
            self.open = false;
            return ModifierOutcome::Closed;
        }
        if self.view.can_reroll() && !self.is_rolling() {
            for index in 0..self.view.lines.len() {
                if self.reroll_rect(panel, index).contains(x, y) {
                    return ModifierOutcome::Reroll(index);
                }
            }
        }
        ModifierOutcome::Consumed
    }
}

/// The colour a magnitude is shown in: stronger is worse for the party, weaker is better.
fn magnitude_color(pct: i16) -> UiColor {
    match pct.cmp(&0) {
        std::cmp::Ordering::Greater => color::CRIMSON,
        std::cmp::Ordering::Less => color::VERDANT,
        std::cmp::Ordering::Equal => color::TEXT_SECONDARY,
    }
}

/// Draws the window. Takes the pointer to light the button under it.
pub fn push_modifiers(
    list: &mut UiDrawList,
    window: &ModifierWindow,
    pointer: (f32, f32),
    viewport: (u32, u32),
) {
    if !window.is_open() {
        return;
    }
    let kit = window.kit();
    let panel = window.panel_rect(viewport);
    kit::panel(list, &kit, panel, color::BORDER_STRONG);
    let pad = kit.px(layout::PADDING);
    let input = UiInput::new().with_pointer(pointer.0, pointer.1);
    let view = &window.view;

    // Header: title, key level, close.
    let header_centre = panel.y + kit.px(layout::HEADER) * 0.5 + kit.px(2.0);
    kit.text(
        list,
        panel.x + pad,
        header_centre,
        "Key Modifiers",
        type_scale::HEADING,
        color::TEXT_TITLE,
        TextAlign::Left,
    );
    let close = window.close_rect(panel);
    kit.text(
        list,
        close.x - kit.px(8.0),
        header_centre,
        &format!("Key level {}", view.key_level),
        type_scale::META,
        color::TEXT_SECONDARY,
        TextAlign::Right,
    );
    kit::button(
        list,
        &kit,
        close,
        "x",
        None,
        ButtonVariant::Ghost,
        Interaction::at(close, &input),
    );

    // The modifiers.
    if view.lines.is_empty() {
        let row = window.row_rect(panel, 0);
        kit.text(
            list,
            row.x + kit.px(4.0),
            row.y + kit.px(layout::EMPTY_ROW) * 0.5,
            "This key has no modifiers.",
            type_scale::UI,
            color::TEXT_MUTED,
            TextAlign::Left,
        );
    }
    let can_press = view.can_reroll() && !window.is_rolling();
    for (index, line) in view.lines.iter().enumerate() {
        let row = window.row_rect(panel, index);
        let corner = kit.px(radius::CARD);
        list.panel(row, corner, color::SURFACE_2, color::SURFACE_1);
        list.panel_stroke(row, corner, 1.0, color::BORDER);
        let edge = kit.px(layout::ICON);
        let icon_rect = UiRect::new(
            row.x + kit.px(8.0),
            row.y + (row.height - edge) * 0.5,
            edge,
            edge,
        );
        list.icon(icon_rect, line.icon, [1.0, 1.0, 1.0, 1.0]);
        let text_x = icon_rect.right() + kit.px(10.0);
        kit.text(
            list,
            text_x,
            row.y + kit.px(22.0),
            &line.name,
            type_scale::ABILITY,
            color::TEXT_PRIMARY,
            TextAlign::Left,
        );
        let name_end = text_x + kit.width(type_scale::ABILITY, &line.name) + kit.px(8.0);
        kit.text(
            list,
            name_end,
            row.y + kit.px(22.0),
            &engine_realm::modifiers::magnitude_text(line.magnitude_pct),
            type_scale::ABILITY,
            magnitude_color(line.magnitude_pct),
            TextAlign::Left,
        );
        let button = window.reroll_rect(panel, index);
        let wrap_width = (row.right() - kit.px(8.0) - text_x).max(kit.px(40.0));
        for (i, text) in kit
            .style
            .wrap(type_scale::META, &line.description, wrap_width)
            .iter()
            .take(3)
            .enumerate()
        {
            kit.text(
                list,
                text_x,
                row.y + kit.px(44.0) + kit.px(14.0) * i as f32,
                text,
                type_scale::META,
                color::TEXT_SECONDARY,
                TextAlign::Left,
            );
        }
        if view.unlocked() {
            kit::button(
                list,
                &kit,
                button,
                "Reroll",
                None,
                ButtonVariant::Primary,
                Interaction::at(button, &input).disabled(!can_press),
            );
        }
    }

    // How much of the dungeon is cleared, with the unlock marked on the bar.
    let bar = window.bar_rect(panel);
    let corner = kit.px(radius::CHIP);
    list.panel(
        bar,
        corner,
        super::kit::tokens::WELL,
        super::kit::tokens::WELL,
    );
    let fill = (bar.width * view.progress()).max(if view.killed > 0 { 2.0 } else { 0.0 });
    if fill > 0.0 {
        let tint = if view.unlocked() {
            color::VERDANT
        } else {
            color::BRONZE
        };
        list.panel(
            UiRect::new(bar.x, bar.y, fill, bar.height),
            corner,
            color::alpha(tint, 0.75),
            color::alpha(tint, 0.55),
        );
    }
    let mark = bar.x + bar.width * (view.unlock_pct as f32 / 100.0).clamp(0.0, 1.0);
    list.rect(
        UiRect::new(
            mark.round(),
            bar.y - kit.px(2.0),
            2.0,
            bar.height + kit.px(4.0),
        ),
        color::TEXT_PRIMARY,
    );
    let mut y = bar.bottom() + kit.px(8.0) + kit.px(layout::LINE) * 0.5;
    kit.text(
        list,
        bar.x,
        y,
        &if view.unlocked() {
            format!("Dungeon cleared {}%", (view.progress() * 100.0).floor())
        } else {
            format!(
                "Reroll unlocks at {}%  \u{b7}  cleared {}%",
                view.unlock_pct,
                (view.progress() * 100.0).floor()
            )
        },
        type_scale::UI,
        if view.unlocked() {
            color::VERDANT
        } else {
            color::TEXT_SECONDARY
        },
        TextAlign::Left,
    );
    if view.unlocked() {
        let text = if view.rerolls_left == 0 {
            "The key has been rerolled.".to_owned()
        } else {
            format!(
                "Reroll costs {} {} \u{b7} you have {}",
                view.cost, view.resource, view.available
            )
        };
        kit.text(
            list,
            bar.right(),
            y,
            &text,
            type_scale::META,
            if view.rerolls_left == 0 || view.available < u64::from(view.cost) {
                color::AMBER
            } else {
                color::TEXT_SECONDARY
            },
            TextAlign::Right,
        );
    }
    if view.reward_bonus_pct > 0 {
        y += kit.px(layout::LINE);
        kit.text(
            list,
            bar.x,
            y,
            &format!("+{}% loot bonus active", view.reward_bonus_pct),
            type_scale::UI,
            color::VERDANT,
            TextAlign::Left,
        );
    }

    // The reveal: the dice, then the answer the server gave.
    if let Some((reveal, elapsed)) = window.reveal() {
        let rect = window.reveal_rect(panel);
        let tint = if elapsed < ROLL_SECONDS {
            color::AMBER
        } else if reveal.strengthened() {
            color::CRIMSON
        } else {
            color::VERDANT
        };
        list.panel(
            rect,
            kit.px(radius::CARD),
            color::alpha(tint, 0.20),
            color::alpha(tint, 0.08),
        );
        list.panel_stroke(rect, kit.px(radius::CARD), 1.0, color::alpha(tint, 0.6));
        kit.text(
            list,
            rect.x + rect.width * 0.5,
            rect.y + kit.px(16.0),
            &format!("{} rerolled {}", reveal.by, reveal.modifier),
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Center,
        );
        if elapsed < ROLL_SECONDS {
            kit.text(
                list,
                rect.x + rect.width * 0.5,
                rect.y + kit.px(40.0),
                &format!(
                    "ROLLING...  {}% Strengthen / {}% Weaken",
                    reveal.strengthen_pct,
                    100u32.saturating_sub(reveal.strengthen_pct)
                ),
                type_scale::HEADING,
                tint,
                TextAlign::Center,
            );
        } else {
            kit.text(
                list,
                rect.x + rect.width * 0.5,
                rect.y + kit.px(40.0),
                &format!(
                    "Result: {}  {}",
                    engine_realm::modifiers::magnitude_text(reveal.delta_pct),
                    if reveal.strengthened() {
                        "Strengthened"
                    } else {
                        "Weakened"
                    }
                ),
                type_scale::HEADING,
                tint,
                TextAlign::Center,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render_data::ui::UiCommand;

    const VIEWPORT: (u32, u32) = (1280, 720);

    fn lines() -> Vec<ModifierLine> {
        vec![
            ModifierLine {
                icon: UiIcon::ModifierRotatingBeams,
                name: "Rotating Beams".into(),
                description: "Beams sweep out from the centre of every pack.".into(),
                magnitude_pct: 0,
            },
            ModifierLine {
                icon: UiIcon::ModifierPlagueExplosion,
                name: "Plague Explosion".into(),
                description: "Slain monsters fester, then burst.".into(),
                magnitude_pct: 0,
            },
        ]
    }

    fn view(killed: u32, total: u32) -> ModifierPanelView {
        ModifierPanelView {
            key_level: 5,
            lines: lines(),
            killed,
            total,
            unlock_pct: 50,
            rerolls_left: 1,
            cost: 1,
            resource: "Aether Shard".into(),
            available: 3,
            reward_bonus_pct: 0,
        }
    }

    fn window(view: ModifierPanelView) -> ModifierWindow {
        let mut window = ModifierWindow::new();
        window.open();
        window.set_view(view);
        window
    }

    fn click(rect: UiRect) -> UiInput {
        UiInput {
            pointer: (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5),
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::new()
        }
    }

    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } | UiCommand::Label { range, .. } => {
                    Some(list.text_of(*range).to_owned())
                }
                _ => None,
            })
            .collect()
    }

    fn drawn(window: &ModifierWindow) -> (UiDrawList, Vec<String>) {
        let mut list = UiDrawList::new();
        push_modifiers(&mut list, window, (0.0, 0.0), VIEWPORT);
        let words = texts(&list);
        (list, words)
    }

    fn icons(list: &UiDrawList) -> Vec<UiIcon> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Icon { icon, .. } => Some(*icon),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_closed_panel_draws_nothing_and_ignores_the_pointer() {
        let mut window = window(view(10, 10));
        window.close();
        assert!(drawn(&window).0.is_empty());
        let panel = window.panel_rect(VIEWPORT);
        assert_eq!(
            window.update(&click(panel), VIEWPORT),
            ModifierOutcome::Idle
        );
    }

    #[test]
    fn locked_it_lists_the_modifiers_and_says_when_a_reroll_unlocks() {
        let window = window(view(3, 10));
        let (list, words) = drawn(&window);
        assert!(words.contains(&"Key Modifiers".to_owned()), "{words:?}");
        assert!(words.contains(&"Key level 5".to_owned()));
        assert!(words.contains(&"Rotating Beams".to_owned()));
        assert!(words.contains(&"Plague Explosion".to_owned()));
        assert!(words.contains(&"+0%".to_owned()));
        assert!(
            words.iter().any(|t| t.starts_with("Reroll unlocks at 50%")),
            "{words:?}"
        );
        assert!(
            !words.contains(&"Reroll".to_owned()),
            "no buttons while locked"
        );
        assert_eq!(
            icons(&list),
            [
                UiIcon::ModifierRotatingBeams,
                UiIcon::ModifierPlagueExplosion
            ]
        );
        // And a press on where the button would be does nothing.
        let mut window = window;
        let panel = window.panel_rect(VIEWPORT);
        let button = window.reroll_rect(panel, 0);
        assert_eq!(
            window.update(&click(button), VIEWPORT),
            ModifierOutcome::Consumed
        );
    }

    #[test]
    fn unlocked_it_shows_reroll_buttons_with_the_cost_and_the_balance() {
        let mut window = window(view(5, 10));
        let (_, words) = drawn(&window);
        assert_eq!(
            words.iter().filter(|t| *t == "Reroll").count(),
            2,
            "{words:?}"
        );
        assert!(
            words
                .iter()
                .any(|t| t.contains("Reroll costs 1 Aether Shard") && t.contains("you have 3")),
            "{words:?}"
        );
        let panel = window.panel_rect(VIEWPORT);
        assert_eq!(
            window.update(&click(window.reroll_rect(panel, 1)), VIEWPORT),
            ModifierOutcome::Reroll(1)
        );
        assert_eq!(
            window.update(&click(window.reroll_rect(panel, 0)), VIEWPORT),
            ModifierOutcome::Reroll(0)
        );
    }

    #[test]
    fn a_player_who_cannot_pay_or_a_used_reroll_cannot_press() {
        for tweak in [
            |v: &mut ModifierPanelView| v.available = 0,
            |v: &mut ModifierPanelView| v.rerolls_left = 0,
        ] {
            let mut v = view(8, 10);
            tweak(&mut v);
            let mut window = window(v);
            let panel = window.panel_rect(VIEWPORT);
            assert_eq!(
                window.update(&click(window.reroll_rect(panel, 0)), VIEWPORT),
                ModifierOutcome::Consumed
            );
        }
        let mut v = view(8, 10);
        v.rerolls_left = 0;
        let (_, words) = drawn(&window(v));
        assert!(
            words.contains(&"The key has been rerolled.".to_owned()),
            "{words:?}"
        );
    }

    #[test]
    fn the_loot_bonus_shows_once_a_reroll_set_it() {
        let (_, words) = drawn(&window(view(5, 10)));
        assert!(!words.iter().any(|t| t.contains("loot bonus")));
        let mut v = view(5, 10);
        v.reward_bonus_pct = 25;
        let (_, words) = drawn(&window(v));
        assert!(
            words.contains(&"+25% loot bonus active".to_owned()),
            "{words:?}"
        );
    }

    #[test]
    fn a_key_without_modifiers_says_so() {
        let mut v = view(0, 10);
        v.lines.clear();
        let (_, words) = drawn(&window(v));
        assert!(
            words.contains(&"This key has no modifiers.".to_owned()),
            "{words:?}"
        );
    }

    #[test]
    fn the_reveal_rolls_then_shows_the_servers_result() {
        let mut window = window(view(5, 10));
        window.start_reveal(Reveal {
            by: "Ash".into(),
            modifier: "Plague Explosion".into(),
            delta_pct: 17,
            strengthen_pct: 60,
        });
        assert!(window.is_rolling());
        let (_, words) = drawn(&window);
        assert!(
            words.contains(&"ROLLING...  60% Strengthen / 40% Weaken".to_owned()),
            "{words:?}"
        );
        assert!(words.contains(&"Ash rerolled Plague Explosion".to_owned()));
        // While the dice roll, Reroll cannot be pressed again.
        let panel = window.panel_rect(VIEWPORT);
        assert_eq!(
            window.update(&click(window.reroll_rect(panel, 0)), VIEWPORT),
            ModifierOutcome::Consumed
        );

        let beats = window.advance(ROLL_SECONDS + 0.05);
        assert!(beats.ticks >= 10, "{beats:?}");
        assert_eq!(beats.result, Some(true));
        assert!(!window.is_rolling());
        let (_, words) = drawn(&window);
        assert!(
            words.contains(&"Result: +17%  Strengthened".to_owned()),
            "{words:?}"
        );
        // The result is announced once, and the reveal ends by itself.
        assert_eq!(window.advance(0.5).result, None);
        window.advance(RESULT_SECONDS);
        assert!(window.reveal().is_none());
    }

    #[test]
    fn a_weakening_reveal_says_so_and_a_reveal_opens_a_closed_panel() {
        let mut window = window(view(5, 10));
        window.close();
        window.start_reveal(Reveal {
            by: "Bo".into(),
            modifier: "Rotating Beams".into(),
            delta_pct: -12,
            strengthen_pct: 60,
        });
        assert!(window.is_open());
        assert_eq!(window.advance(ROLL_SECONDS + 0.01).result, Some(false));
        let (_, words) = drawn(&window);
        assert!(
            words.contains(&"Result: -12%  Weakened".to_owned()),
            "{words:?}"
        );
    }

    #[test]
    fn ticks_are_counted_once_each_however_the_frames_fall() {
        let mut slow = window(view(5, 10));
        let mut fast = slow.clone();
        let reveal = Reveal {
            by: "A".into(),
            modifier: "M".into(),
            delta_pct: 10,
            strengthen_pct: 60,
        };
        slow.start_reveal(reveal.clone());
        fast.start_reveal(reveal);
        let mut a = 0;
        let mut b = 0;
        for _ in 0..200 {
            a += slow.advance(0.01).ticks;
        }
        for _ in 0..20 {
            b += fast.advance(0.1).ticks;
        }
        assert_eq!(a, 14);
        assert_eq!(a, b);
    }

    #[test]
    fn the_close_button_closes_and_the_panel_owns_the_pointer_over_it() {
        let mut window = window(view(0, 10));
        let panel = window.panel_rect(VIEWPORT);
        assert!(window.wants_pointer((panel.x + 2.0, panel.y + 2.0), VIEWPORT));
        assert!(!window.wants_pointer((900.0, 600.0), VIEWPORT));
        assert_eq!(
            window.update(&click(window.close_rect(panel)), VIEWPORT),
            ModifierOutcome::Closed
        );
        assert!(!window.is_open());
    }

    #[test]
    fn progress_and_unlock_follow_the_kill_share() {
        assert!(!view(4, 10).unlocked());
        assert!(view(5, 10).unlocked());
        assert!(!view(0, 0).unlocked());
        assert!((view(1, 4).progress() - 0.25).abs() < 1e-6);
        assert!(view(5, 10).can_reroll());
        let mut broke = view(5, 10);
        broke.available = 0;
        assert!(!broke.can_reroll());
    }
}
