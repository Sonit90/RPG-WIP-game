//! A row of buff and debuff icons, and the tooltip for whichever one the pointer is on.
//!
//! ```text
//!  ┌─3┐┌──┐┌──┐
//!  │  ││  ││  │     icon, the stacks in its top corner, the seconds along its bottom,
//!  └12┘└─4┘└──┘     a darkening wipe for the time already gone,
//!   ^^^^            and a blue border when a dispel can take it off
//! ```
//!
//! One widget for every place an effect is shown — under a nameplate, in the player's buff and
//! debuff bars, on a party row and a raid cell — so an effect looks the same wherever it is and
//! a dispellable debuff is blue-bordered everywhere. Every icon drawn is also recorded in
//! [`AuraHits`], which is what lets a tooltip be found for the pointer after the whole
//! interface has been laid out, without a second layout pass.

use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect};

use crate::aura::{Aura, write_remaining};
use crate::game_ui::{cap_centred_top, faded, plate_text_style};
use crate::ui::map_style::color;

/// The border a dispellable debuff wears: the one colour that says "a dispel takes this off".
pub const DISPEL_BORDER: UiColor = [0.28, 0.58, 1.0, 1.0];

/// The hairline every other icon has, so an icon keeps an edge against a bright background.
const PLAIN_BORDER: UiColor = [0.0, 0.0, 0.0, 0.9];

/// How dark the wipe over a spent duration is.
const WIPE: UiColor = [0.0, 0.0, 0.0, 0.5];

/// The seconds printed over an icon.
const TIME_COLOR: UiColor = [1.0, 0.96, 0.84, 1.0];

/// The stack count in an icon's corner.
const STACK_COLOR: UiColor = [1.0, 1.0, 1.0, 1.0];

/// The edge of the stack count's badge: the accent the interface marks things to count with.
const STACK_EDGE: UiColor = [0.867, 0.733, 0.502, 0.85];

/// The dark edge on both.
const TEXT_EDGE: UiColor = [0.0, 0.0, 0.0, 0.95];

/// Which way a row grows from its origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraGrow {
    /// From the left edge, rightwards: a frame's rows.
    Right,
    /// From the right edge, leftwards: a bar anchored to the right of the screen.
    Left,
    /// Centred on the origin: under a nameplate.
    Centered,
}

/// How a row is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuraRowStyle {
    /// An icon's edge, physical pixels.
    pub edge: f32,
    /// Between icons, physical pixels.
    pub gap: f32,
    /// Icons per line before the row wraps.
    pub per_line: usize,
    /// Icons in all, past which the rest are not drawn.
    pub max: usize,
    /// Whether the seconds left are printed over each icon.
    pub show_time: bool,
    /// The whole row's opacity: a dimmed nameplate dims its icons with it.
    pub alpha: f32,
}

impl AuraRowStyle {
    /// A row of `edge`-pixel icons, `per_line` to a line and `max` in all.
    #[must_use]
    pub fn new(edge: f32, per_line: usize, max: usize) -> Self {
        let edge = edge.round().max(8.0);
        Self {
            edge,
            gap: (edge * 0.12).round().max(1.0),
            per_line: per_line.max(1),
            max,
            show_time: true,
            alpha: 1.0,
        }
    }

    /// The same, at `alpha`.
    #[must_use]
    pub fn faded(mut self, alpha: f32) -> Self {
        self.alpha = alpha.clamp(0.0, 1.0);
        self
    }

    /// The same, with or without the seconds printed.
    #[must_use]
    pub fn with_time(mut self, show: bool) -> Self {
        self.show_time = show;
        self
    }

    /// How tall `count` icons are once wrapped, physical pixels. Zero for none.
    #[must_use]
    pub fn height_for(&self, count: usize) -> f32 {
        let count = count.min(self.max);
        if count == 0 {
            return 0.0;
        }
        let lines = count.div_ceil(self.per_line);
        lines as f32 * self.edge + (lines - 1) as f32 * self.gap
    }

    /// How wide one line of `count` icons is, physical pixels.
    #[must_use]
    pub fn width_for(&self, count: usize) -> f32 {
        let count = count.min(self.max).min(self.per_line);
        if count == 0 {
            return 0.0;
        }
        count as f32 * self.edge + (count - 1) as f32 * self.gap
    }
}

/// Every icon drawn this frame, with the effect it shows: what a tooltip is looked up in.
///
/// Reserved once and cleared each frame, so recording costs no allocation. A full list drops
/// the rest — they are still drawn, and simply have no tooltip — rather than growing.
#[derive(Debug, Clone)]
pub struct AuraHits {
    hits: Vec<(UiRect, Aura)>,
}

impl Default for AuraHits {
    fn default() -> Self {
        Self::new()
    }
}

impl AuraHits {
    /// How many icons one frame records.
    pub const CAPACITY: usize = 512;

    /// An empty record with its storage reserved.
    #[must_use]
    pub fn new() -> Self {
        Self {
            hits: Vec::with_capacity(Self::CAPACITY),
        }
    }

    /// Forgets the last frame's icons.
    pub fn clear(&mut self) {
        self.hits.clear();
    }

    /// Records one icon.
    pub fn push(&mut self, rect: UiRect, aura: Aura) {
        if self.hits.len() < Self::CAPACITY {
            self.hits.push((rect, aura));
        }
    }

    /// How many were recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hits.len()
    }

    /// Whether none were.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hits.is_empty()
    }

    /// The icon under `pointer`, if there is one. The last one recorded wins, which is the
    /// one painted on top.
    #[must_use]
    pub fn at(&self, pointer: (f32, f32)) -> Option<(UiRect, Aura)> {
        self.hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(pointer.0, pointer.1))
            .copied()
    }
}

/// Draws `auras` as a row starting at `(x, top)`, and records each icon in `hits`.
///
/// Returns the height the row took, zero when there was nothing to draw — so a caller can
/// stack the next thing under it without leaving a gap for an empty row.
pub fn push_aura_row<'a>(
    list: &mut UiDrawList,
    hits: &mut AuraHits,
    auras: impl Iterator<Item = &'a Aura> + Clone,
    (x, top): (f32, f32),
    grow: AuraGrow,
    style: AuraRowStyle,
    scratch: &mut String,
) -> f32 {
    let count = auras.clone().take(style.max).count();
    if count == 0 || style.alpha <= 0.01 {
        return 0.0;
    }
    let step = style.edge + style.gap;
    for (index, aura) in auras.take(style.max).enumerate() {
        let line = index / style.per_line;
        let column = index % style.per_line;
        // A centred row centres each line on its own, so a short last line sits in the middle.
        let in_line = (count - line * style.per_line).min(style.per_line);
        let left = match grow {
            AuraGrow::Right => x + column as f32 * step,
            AuraGrow::Left => x - style.edge - column as f32 * step,
            AuraGrow::Centered => {
                (x - style.width_for(in_line) * 0.5).round() + column as f32 * step
            }
        };
        let rect = UiRect::new(left, top + line as f32 * step, style.edge, style.edge);
        push_aura_icon(list, rect, aura, style, scratch);
        hits.push(rect, *aura);
    }
    style.height_for(count)
}

/// One icon: backing, art, wipe, border, seconds and stacks.
pub fn push_aura_icon(
    list: &mut UiDrawList,
    rect: UiRect,
    aura: &Aura,
    style: AuraRowStyle,
    scratch: &mut String,
) {
    let alpha = style.alpha;
    let dispellable = aura.is_dispellable();
    // A dispellable debuff's border is thick enough to read at a nameplate's size; anything
    // else keeps a hairline so the art has an edge against a bright spell.
    let border = if dispellable {
        (rect.width * 0.1).round().max(2.0)
    } else {
        (rect.width * 0.04).round().max(1.0)
    };
    list.rect(rect, faded([0.02, 0.02, 0.03, 0.9], alpha));
    let art = rect.inset(border);
    list.icon(art, aura.kind.icon(), faded([1.0; 4], alpha));
    if let Some(fraction) = aura.fraction() {
        let spent = (art.height * (1.0 - fraction)).round();
        if spent > 0.0 {
            list.rect(
                UiRect::new(art.x, art.y, art.width, spent),
                faded(WIPE, alpha),
            );
        }
    }
    let edge = if dispellable {
        DISPEL_BORDER
    } else {
        PLAIN_BORDER
    };
    list.outline(rect, border, faded(edge, alpha));

    let outline = faded(TEXT_EDGE, alpha);
    if style.show_time {
        write_remaining(scratch, aura);
        if !scratch.is_empty() {
            // Low on the icon, so the top of the art — what the effect *is* — stays visible.
            let size = (rect.width * 0.42).max(8.0);
            list.styled_label(
                (
                    rect.x + rect.width * 0.5,
                    cap_centred_top(UiFont::SemiBold, size, rect.bottom() - size * 0.55),
                ),
                scratch,
                faded(TIME_COLOR, alpha),
                UiFont::SemiBold,
                size,
                TextAlign::Center,
                plate_text_style(outline, size, 0.02),
            );
        }
    }
    if aura.stacks > 1 {
        use std::fmt::Write as _;
        scratch.clear();
        let _ = write!(scratch, "{}", aura.stacks);
        // A badge in the top-right corner, hanging just over the edge: a count, read at a
        // glance and never mistaken for the seconds along the bottom. Drawn from the aura's
        // own stack count — the one the simulation keeps — for every stacking effect.
        let size = (rect.width * 0.42).max(9.0);
        let width = UiFont::SemiBold.width(scratch, size);
        let pad = (size * 0.32).round().max(2.0);
        let badge = UiRect::new(
            (rect.right() - width - pad * 2.0 + rect.width * 0.12).round(),
            (rect.y - rect.width * 0.12).round(),
            (width + pad * 2.0).round(),
            (size * 1.05).round(),
        );
        let corner = badge.height * 0.5;
        list.panel(
            badge,
            corner,
            faded([0.10, 0.10, 0.12, 0.95], alpha),
            faded([0.02, 0.02, 0.03, 0.95], alpha),
        );
        list.panel_stroke(badge, corner, 1.0, faded(STACK_EDGE, alpha));
        list.styled_label(
            (
                badge.x + badge.width * 0.5,
                cap_centred_top(UiFont::SemiBold, size, badge.y + badge.height * 0.5),
            ),
            scratch,
            faded(STACK_COLOR, alpha),
            UiFont::SemiBold,
            size,
            TextAlign::Center,
            plate_text_style(outline, size, 0.03),
        );
    }
}

/// The buff and debuff blocks' measurements, logical pixels.
pub mod size {
    /// An icon's edge in any of the four blocks.
    pub const BUFF_ICON: f32 = 30.0;
    /// Buff icons on one line.
    pub const BUFFS_PER_LINE: usize = 10;
    /// Lines of buffs.
    pub const BUFF_LINES: usize = 2;
    /// Between icons.
    pub const GAP: f32 = 4.0;
    /// Debuffs on one line, the player's and the target's: about a frame's width.
    pub const DEBUFFS_PER_LINE: usize = 8;
    /// Lines of debuffs.
    pub const DEBUFF_LINES: usize = 1;
    /// A debuff icon beside a party row.
    pub const PARTY_ICON: f32 = 24.0;
    /// Debuffs beside one party row.
    pub const PARTY_DEBUFFS: usize = 4;
    /// A debuff icon inside a raid cell.
    pub const RAID_ICON: f32 = 16.0;
    /// Debuffs inside one raid cell.
    pub const RAID_DEBUFFS: usize = 3;
}

/// The buff bar's design size: two lines of eight.
#[must_use]
pub fn buff_bar_size() -> (f32, f32) {
    let per_line = size::BUFFS_PER_LINE as f32;
    let lines = size::BUFF_LINES as f32;
    (
        per_line * size::BUFF_ICON + (per_line - 1.0) * size::GAP,
        lines * size::BUFF_ICON + (lines - 1.0) * size::GAP,
    )
}

/// The player's buffs, one per cell of `flow`: the bar the player arranged.
///
/// The first buff is in the corner the flow starts from — top right by default, because the
/// bar's home is the top-right of the screen and a buff gained is then added at the end
/// nearest the middle. Placed there rather than reversed after layout, so three buffs in a
/// right-to-left bar sit against its right edge whatever the bar's capacity.
pub fn push_buff_bar<'a>(
    list: &mut UiDrawList,
    hits: &mut AuraHits,
    flow: &super::flow::Flow,
    buffs: impl Iterator<Item = &'a Aura>,
    scratch: &mut String,
) {
    push_aura_block(list, hits, flow, buffs, scratch);
}

/// Any block of aura icons, one per cell of `flow`: the player's buffs or debuffs, or the
/// target's. Stacks in the corner and seconds along the bottom, exactly as a nameplate
/// draws them, so an effect reads the same on the target frame as over the target's head.
pub fn push_aura_block<'a>(
    list: &mut UiDrawList,
    hits: &mut AuraHits,
    flow: &super::flow::Flow,
    auras: impl Iterator<Item = &'a Aura>,
    scratch: &mut String,
) {
    let Some(first) = flow.cell(0) else {
        return;
    };
    let style = AuraRowStyle::new(first.width, flow.columns, flow.capacity());
    for (index, aura) in auras.enumerate() {
        let Some(cell) = flow.cell(index) else {
            break;
        };
        push_aura_icon(list, cell, aura, style, scratch);
        hits.push(cell, *aura);
    }
}

/// Each party member's debuffs, beside their row of the party frame `frame`.
///
/// Outside the frame on its right, the way the genre has always hung them: the row itself is
/// a name over a health bar and has no room for an icon a healer can read at a glance.
pub fn push_party_debuffs(
    list: &mut UiDrawList,
    hits: &mut AuraHits,
    frame: UiRect,
    members: &[crate::snapshot::PartyMemberView],
    upwards: bool,
    scratch: &mut String,
) {
    use super::unit_frame::size as frame_size;
    let factor = frame.width / frame_size::PARTY_WIDTH;
    let padding = frame_size::PADDING * factor;
    let row_height = frame_size::PARTY_ROW * factor;
    let style = AuraRowStyle::new(
        size::PARTY_ICON * factor,
        size::PARTY_DEBUFFS,
        size::PARTY_DEBUFFS,
    );
    for (index, member) in members.iter().enumerate() {
        let row = super::unit_frame::party_row(index, upwards);
        let top = frame.y + padding + row_height * row as f32;
        if top + row_height > frame.bottom() {
            break;
        }
        let centre = top + row_height * 0.5;
        push_aura_row(
            list,
            hits,
            member.auras.debuffs(),
            (
                frame.right() + (4.0 * factor).round(),
                (centre - style.edge * 0.5).round(),
            ),
            AuraGrow::Right,
            style,
            scratch,
        );
    }
}

/// Each member's debuffs inside their raid cell, along its bottom-left edge.
///
/// Small and without a countdown: a cell is a health bar first, and what a healer needs from
/// it is *that* there is something to dispel — the blue border — and which one, on hover.
pub fn push_raid_debuffs(
    list: &mut UiDrawList,
    hits: &mut AuraHits,
    flow: &super::flow::Flow,
    members: &[crate::snapshot::PartyMemberView],
    scratch: &mut String,
) {
    let factor = flow.cell(0).map_or(1.0, |cell| {
        cell.height / super::raid_frame::size::CELL_HEIGHT
    });
    let style = AuraRowStyle::new(
        size::RAID_ICON * factor,
        size::RAID_DEBUFFS,
        size::RAID_DEBUFFS,
    )
    .with_time(false);
    let inset = (4.0 * factor).round();
    for (index, member) in members.iter().enumerate() {
        let Some(cell) = flow.cell(index) else {
            break;
        };
        push_aura_row(
            list,
            hits,
            member.auras.debuffs(),
            (cell.x + inset, cell.bottom() - inset - style.edge),
            AuraGrow::Right,
            style,
            scratch,
        );
    }
}

/// A dispel school's name, as a tooltip prints it.
#[must_use]
pub fn school_name(school: engine_game::status::DispelSchool) -> &'static str {
    use engine_game::status::DispelSchool;
    match school {
        DispelSchool::Magic => "Magic",
        DispelSchool::Poison => "Poison",
        DispelSchool::Curse => "Curse",
        DispelSchool::Disease => "Disease",
    }
}

/// A dispel school's colour in a tooltip. Magic is the dispel blue itself.
#[must_use]
pub fn school_color(school: engine_game::status::DispelSchool) -> UiColor {
    use engine_game::status::DispelSchool;
    match school {
        DispelSchool::Magic => DISPEL_BORDER,
        DispelSchool::Poison => [0.55, 0.85, 0.30, 1.0],
        DispelSchool::Curse => color::ARCANE,
        DispelSchool::Disease => [0.80, 0.66, 0.36, 1.0],
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::aura::AuraKind;
    use engine_game::status::StatusKind;
    use engine_render_data::ui::{UiCommand, UiIcon};

    fn poison() -> Aura {
        Aura::timed(AuraKind::Status(StatusKind::Poisoned), 6.0, 12.0)
    }

    fn rend() -> Aura {
        Aura {
            stacks: 3,
            ..Aura::timed(AuraKind::Status(StatusKind::Rended), 4.0, 8.0)
        }
    }

    fn outlines(list: &UiDrawList) -> Vec<(UiRect, UiColor)> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Outline { rect, color, .. } => Some((*rect, *color)),
                _ => None,
            })
            .collect()
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
    fn a_dispellable_debuff_is_bordered_in_blue_and_a_bleed_is_not() {
        let mut list = UiDrawList::new();
        let mut hits = AuraHits::new();
        let mut scratch = String::new();
        let auras = [poison(), rend()];
        push_aura_row(
            &mut list,
            &mut hits,
            auras.iter(),
            (100.0, 50.0),
            AuraGrow::Right,
            AuraRowStyle::new(24.0, 8, 8),
            &mut scratch,
        );
        let borders = outlines(&list);
        assert_eq!(borders.len(), 2);
        assert_eq!(borders[0].1, DISPEL_BORDER);
        assert_ne!(borders[1].1, DISPEL_BORDER);
        assert_eq!(
            icons(&list),
            vec![UiIcon::AuraPoisoned, UiIcon::AuraRended],
            "each effect draws its own icon"
        );
    }

    #[test]
    fn every_icon_drawn_can_be_found_under_the_pointer() {
        let mut list = UiDrawList::new();
        let mut hits = AuraHits::new();
        let mut scratch = String::new();
        let auras = [poison(), rend()];
        push_aura_row(
            &mut list,
            &mut hits,
            auras.iter(),
            (100.0, 50.0),
            AuraGrow::Right,
            AuraRowStyle::new(24.0, 8, 8),
            &mut scratch,
        );
        assert_eq!(hits.len(), 2);
        let (_, first) = hits.at((110.0, 60.0)).expect("the first icon");
        assert_eq!(first.kind, poison().kind);
        let (rect, second) = hits.at((140.0, 60.0)).expect("the second icon");
        assert_eq!(second.kind, rend().kind);
        assert!(rect.x > 124.0);
        assert!(hits.at((10.0, 10.0)).is_none());
    }

    #[test]
    fn a_row_wraps_centres_and_reports_its_height() {
        let style = AuraRowStyle::new(20.0, 3, 5);
        let mut list = UiDrawList::new();
        let mut hits = AuraHits::new();
        let mut scratch = String::new();
        let auras = [poison(); 7];
        let height = push_aura_row(
            &mut list,
            &mut hits,
            auras.iter(),
            (300.0, 0.0),
            AuraGrow::Centered,
            style,
            &mut scratch,
        );
        assert_eq!(hits.len(), 5, "the maximum is honoured");
        assert_eq!(height, style.height_for(5));
        assert!(height > style.edge * 2.0 - 0.5);
        // The last line holds two icons, centred on the origin.
        let rects: Vec<UiRect> = (0..5).map(|index| hits.hits[index].0).collect();
        let last = (rects[3].x + rects[4].right()) * 0.5;
        assert!((last - 300.0).abs() <= 1.0, "last line centred at {last}");
    }

    #[test]
    fn an_empty_row_takes_no_room_and_draws_nothing() {
        let mut list = UiDrawList::new();
        let mut hits = AuraHits::new();
        let mut scratch = String::new();
        let none: [Aura; 0] = [];
        let height = push_aura_row(
            &mut list,
            &mut hits,
            none.iter(),
            (0.0, 0.0),
            AuraGrow::Right,
            AuraRowStyle::new(20.0, 4, 4),
            &mut scratch,
        );
        assert_eq!(height, 0.0);
        assert!(list.commands().is_empty());
    }

    #[test]
    fn stacks_and_seconds_are_printed_over_the_icon() {
        let mut list = UiDrawList::new();
        let mut hits = AuraHits::new();
        let mut scratch = String::new();
        let auras = [rend()];
        push_aura_row(
            &mut list,
            &mut hits,
            auras.iter(),
            (0.0, 0.0),
            AuraGrow::Right,
            AuraRowStyle::new(30.0, 4, 4),
            &mut scratch,
        );
        let texts: Vec<String> = list
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::StyledLabel { range, .. } => Some(list.text_of(*range).to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["4".to_owned(), "3".to_owned()]);
    }

    #[test]
    fn a_tooltip_names_the_effect_its_school_and_its_time() {
        // The aura's tooltip is the shared one: same panel, same dock as an ability's.
        let mut list = UiDrawList::new();
        let mut model = crate::ui::tooltip::TooltipModel::new();
        crate::ui::tooltip::fill_aura(&mut model, &poison());
        crate::ui::tooltip::TooltipPanel::new().push(
            &mut list,
            &model,
            UiRect::new(800.0, 600.0, crate::ui::tooltip::DEFAULT_WIDTH, 64.0),
            crate::ui::flow::FlowDirection::READING,
            &crate::ui::kit::Kit::new(engine_render_data::ui::UiScale::new(1.0)),
            (1280, 720),
        );
        let texts: Vec<String> = list
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Label { range, .. } => Some(list.text_of(*range).to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"Poisoned".to_owned()));
        assert!(texts.contains(&"Poison".to_owned()));
        assert!(texts.iter().any(|text| text.contains("sec remaining")));
        assert!(texts.iter().any(|text| text.contains("Nature damage")));
    }
}
