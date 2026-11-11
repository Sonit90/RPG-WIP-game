//! The raid frames: one filled, rounded cell per member.
//!
//! ```text
//!   ┌───────────────┐
//!   │ You           │   <- name, top-left
//!   │███████████    │   <- the cell itself fills with health, left to right
//!   │          87%  │   <- the percentage, bottom-right
//!   └───────────────┘
//! ```
//!
//! The genre's raid grid: members stacked in columns of five, each a rounded rectangle whose
//! own area *is* the health bar. It is drawn with a party of one too — the player alone is a
//! raid of one — so a healer's layout is the same the first time they log in as it is in a
//! full group, and nothing appears or moves when somebody joins.
//!
//! Like every other element it fills the rectangle the [`HudLayout`](super::hud_layout)
//! resolves for it and decides nothing about where that is; cells for members who are not
//! there are not drawn, and the frame keeps its size so it never moves under the player's
//! hand.

use engine_game::entity::EntityId;
use engine_game::party::MAX_PARTY;
use engine_render_data::ui::{TextAlign, UiClip, UiDrawList, UiRect};

use super::frame_style::{frame_text, friendly_fill};
use crate::game_ui::{PlateTheme, faded};
use crate::snapshot::PartyMemberView;

/// Design sizes, in logical pixels.
pub mod size {
    /// Width of one cell.
    pub const CELL_WIDTH: f32 = 104.0;
    /// Height of one cell.
    pub const CELL_HEIGHT: f32 = 46.0;
    /// Gap between cells.
    pub const GAP: f32 = 4.0;
    /// Members per column: one group, as the genre stacks them.
    pub const ROWS: usize = 5;
    /// Corner radius of a cell.
    pub const RADIUS: f32 = 6.0;
}

/// How many columns the frame has room for: enough for a full roster.
fn columns() -> usize {
    MAX_PARTY.div_ceil(size::ROWS).max(1)
}

/// How many rows the frame has room for.
fn rows() -> usize {
    MAX_PARTY.clamp(1, size::ROWS)
}

/// The raid frame's design size at the default spacing: room for the whole roster.
#[must_use]
pub fn raid_size() -> (f32, f32) {
    raid_size_spaced(1.0)
}

/// The raid frame's design size with the gap between cells at `spacing` times its default.
#[must_use]
pub fn raid_size_spaced(spacing: f32) -> (f32, f32) {
    super::flow::Flow::design_size(
        (size::CELL_WIDTH, size::CELL_HEIGHT),
        size::GAP * spacing.max(0.0),
        columns(),
        rows(),
    )
}

/// The frame's cells inside `rect`: columns of one group, filled downwards first, in
/// `direction`, `spacing` times the design gap apart.
#[must_use]
pub fn flow(
    rect: UiRect,
    direction: super::flow::FlowDirection,
    spacing: f32,
) -> super::flow::Flow {
    super::flow::Flow {
        rect,
        columns: columns(),
        rows: rows(),
        axis: super::flow::FlowAxis::Columns,
        direction,
        // Against the cell's height, the axis groups stack along.
        gap_ratio: size::GAP / size::CELL_HEIGHT * spacing.max(0.0),
    }
}

/// Where member `index`'s cell is inside a frame drawn in `rect`, in reading order at the
/// default spacing. The arranged HUD asks [`HudLayout::raid_flow`](super::hud_layout::HudLayout::raid_flow).
#[must_use]
pub fn cell_rect(rect: UiRect, index: usize) -> Option<UiRect> {
    flow(rect, super::flow::FlowDirection::READING, 1.0).cell(index)
}

/// Which member's cell `pointer` is on, if it is on one.
#[must_use]
pub fn raid_cell_at(rect: UiRect, members: usize, pointer: (f32, f32)) -> Option<usize> {
    raid_cell_in(
        &flow(rect, super::flow::FlowDirection::READING, 1.0),
        members,
        pointer,
    )
}

/// Which member's cell `pointer` is on, in an arranged frame.
#[must_use]
pub fn raid_cell_in(
    flow: &super::flow::Flow,
    members: usize,
    pointer: (f32, f32),
) -> Option<usize> {
    flow.index_at(members.min(MAX_PARTY), pointer)
}

/// Draws one cell per member into `rect`.
///
/// `target` is the player's selection, whose cell is outlined in white — the same mark the
/// target's nameplate carries. `scratch` holds the name and the percentage while they are
/// written, so a frame allocates nothing after the first.
pub fn push_raid_frame(
    list: &mut UiDrawList,
    flow: &super::flow::Flow,
    members: &[PartyMemberView],
    target: Option<EntityId>,
    theme: &PlateTheme,
    scratch: &mut String,
) {
    // Text and corners scale with the cells, not with the frame: wider spacing is more room
    // between the same cells.
    let factor = flow
        .cell(0)
        .map_or(1.0, |cell| cell.height / size::CELL_HEIGHT);
    for (index, member) in members.iter().enumerate() {
        let Some(cell) = flow.cell(index) else {
            break;
        };
        push_cell(
            list,
            cell,
            member,
            target == Some(member.entity),
            theme,
            factor,
            scratch,
        );
    }
}

/// One member's cell.
fn push_cell(
    list: &mut UiDrawList,
    cell: UiRect,
    member: &PartyMemberView,
    targeted: bool,
    theme: &PlateTheme,
    factor: f32,
    scratch: &mut String,
) {
    use std::fmt::Write as _;
    let radius = (size::RADIUS * factor).round().max(2.0);
    let fraction = if member.alive {
        member.health_fraction()
    } else {
        0.0
    };

    // The ground, then the fill: the same rounded shape, cut on the right at the health.
    // Cut with a clip rather than drawn narrower, so the fill keeps the cell's rounded left
    // corners and ends in a straight edge instead of turning into a pill.
    list.panel(cell, radius, theme.frame_panel, theme.bar_background);
    if fraction > 0.0 {
        let (top, bottom) = friendly_fill(theme, fraction);
        list.clip(UiClip::rect(UiRect::new(
            cell.x,
            cell.y,
            (cell.width * fraction).round().max(1.0),
            cell.height,
        )));
        list.panel(cell, radius, top, bottom);
        list.clip(UiClip::NONE);
    }
    // The shield, in the same place and colours as on every other health bar.
    if member.alive {
        let layers = super::frame_style::HealthLayers {
            absorb: super::frame_style::HealthLayers::of(
                member.health,
                member.max_health,
                member.absorb,
            )
            .absorb,
            ..super::frame_style::HealthLayers::health(fraction)
        };
        let inset = (radius * 0.5).round();
        super::frame_style::push_absorb_layer(
            list,
            UiRect::new(
                cell.x,
                cell.y + inset,
                cell.width,
                cell.height - inset * 2.0,
            ),
            layers,
            theme,
            1.0,
        );
    }
    let (stroke, color) = if targeted {
        ((2.0 * factor).round().max(2.0), theme.border_target)
    } else {
        (factor.round().max(1.0), theme.frame_border)
    };
    list.panel_stroke(cell, radius, stroke, color);

    let pad = (6.0 * factor).round();
    let name_size = 12.5 * factor;
    let label_color = if member.alive {
        theme.name
    } else {
        theme.name_dead
    };
    crate::ui::map_style::write_title_case(scratch, member.name());
    // The name is cut at the cell's edge rather than printed over its neighbour.
    list.clip(UiClip::rect(cell.inset(pad * 0.5)));
    frame_text(
        list,
        cell.x + pad,
        cell.y + cell.height * 0.3,
        scratch,
        label_color,
        name_size,
        TextAlign::Left,
        theme,
    );
    list.clip(UiClip::NONE);

    scratch.clear();
    if member.alive {
        let percent = (fraction * 100.0).round() as u32;
        let percent = if percent == 0 && fraction > 0.0 {
            1
        } else {
            percent
        };
        let _ = write!(scratch, "{percent}%");
    } else {
        scratch.push_str("Dead");
    }
    frame_text(
        list,
        cell.right() - pad,
        cell.y + cell.height * 0.72,
        scratch,
        faded(theme.health_text, if member.alive { 1.0 } else { 0.75 }),
        14.0 * factor,
        TextAlign::Right,
        theme,
    );
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::snapshot::UiSnapshot;
    use engine_game::arena::{self, ArenaConfig};
    use engine_render_data::ui::UiCommand;

    fn solo() -> UiSnapshot {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        UiSnapshot::build(&simulation)
    }

    fn labels(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::StyledLabel { range, .. } => Some(list.text_of(*range).to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_party_of_one_is_a_raid_of_one() {
        let snapshot = solo();
        assert!(!snapshot.party.is_empty());
        let rect = UiRect::new(0.0, 0.0, raid_size().0, raid_size().1);
        let mut list = UiDrawList::new();
        push_raid_frame(
            &mut list,
            &flow(rect, crate::ui::flow::FlowDirection::READING, 1.0),
            &snapshot.party[..1],
            None,
            &PlateTheme::default(),
            &mut String::new(),
        );
        let printed = labels(&list);
        assert!(printed.contains(&"You".to_owned()), "{printed:?}");
        assert!(printed.contains(&"100%".to_owned()), "{printed:?}");
    }

    #[test]
    fn the_cell_fills_to_the_health_and_keeps_its_rounded_shape() {
        let snapshot = solo();
        let mut member = snapshot.party[0].clone();
        member.health = member.max_health * 0.4;
        let rect = UiRect::new(0.0, 0.0, raid_size().0, raid_size().1);
        let mut list = UiDrawList::new();
        push_raid_frame(
            &mut list,
            &flow(rect, crate::ui::flow::FlowDirection::READING, 1.0),
            &[member],
            None,
            &PlateTheme::default(),
            &mut String::new(),
        );
        let cell = cell_rect(rect, 0).expect("a cell");
        // The fill is the whole rounded cell drawn under a clip cut at forty per cent.
        let clip = list
            .commands()
            .iter()
            .find_map(|command| match command {
                UiCommand::Clip(clip) => clip.rect,
                _ => None,
            })
            .expect("the fill is clipped");
        assert!((clip.width - cell.width * 0.4).abs() <= 1.0, "{clip:?}");
        assert!(labels(&list).contains(&"40%".to_owned()));
        let radii: Vec<f32> = list
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Panel { radius, .. } => Some(*radius),
                _ => None,
            })
            .collect();
        assert!(radii.iter().all(|radius| *radius > 0.0), "{radii:?}");
    }

    #[test]
    fn a_dead_member_reads_dead_and_the_target_is_outlined_in_white() {
        let snapshot = solo();
        let mut member = snapshot.party[0].clone();
        member.alive = false;
        member.health = 0.0;
        let theme = PlateTheme::default();
        let rect = UiRect::new(0.0, 0.0, raid_size().0, raid_size().1);
        let mut list = UiDrawList::new();
        let entity = member.entity;
        push_raid_frame(
            &mut list,
            &flow(rect, crate::ui::flow::FlowDirection::READING, 1.0),
            &[member],
            Some(entity),
            &theme,
            &mut String::new(),
        );
        assert!(labels(&list).contains(&"Dead".to_owned()));
        assert!(list.commands().iter().any(|command| matches!(
            command,
            UiCommand::Panel { top, stroke, .. } if *stroke > 0.0 && *top == theme.border_target
        )));
    }

    #[test]
    fn a_click_finds_the_cell_it_landed_on() {
        let rect = UiRect::new(100.0, 200.0, raid_size().0 * 1.5, raid_size().1 * 1.5);
        for index in 0..MAX_PARTY {
            let cell = cell_rect(rect, index).expect("a cell");
            let centre = (cell.x + cell.width * 0.5, cell.y + cell.height * 0.5);
            assert_eq!(raid_cell_at(rect, MAX_PARTY, centre), Some(index));
            assert!(cell.right() <= rect.right() + 1.0 && cell.bottom() <= rect.bottom() + 1.0);
        }
        assert_eq!(raid_cell_at(rect, 1, (rect.x - 5.0, rect.y)), None);
        // A cell nobody is in is not a cell.
        let second = cell_rect(rect, 1).expect("a cell");
        assert_eq!(
            raid_cell_at(rect, 1, (second.x + 2.0, second.y + 2.0)),
            None
        );
    }
}
