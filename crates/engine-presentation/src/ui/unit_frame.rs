//! The player, target, target-of-target and party frames.
//!
//! ```text
//! UiSnapshot  +  HudLayout rectangle  ->  push_*_frame  ->  UiDrawList
//! ```
//!
//! Four frames, one shape. Each takes the rectangle the [`HudLayout`](super::hud_layout)
//! resolved for it and fills it: a name, a health bar, and whatever else that unit has. None
//! of them decides where it is, how big it is or whether it is drawn — that is the layout's
//! job, which is what makes all four movable, scalable and toggleable without four
//! implementations of moving, scaling and toggling.
//!
//! ## What a frame is allowed to print
//!
//! Only what the snapshot has. There are no character names in the world yet, so a frame
//! prints what the entity *is* — `YOU`, `ENEMY`, `DUMMY` — rather than inventing a name the
//! gameplay systems could not supply. When there is nothing to show at all, the frame prints
//! its empty state rather than disappearing: a target frame that vanished between targets
//! would be a hole in the layout the player cannot aim at.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect, UiScale};
use engine_settings::hud::HudPlacement;

use super::frame_style::{frame_bar, frame_panel, frame_text, friendly_fill, hostile_fill, shaded};
use crate::game_ui::{UiTheme, write_integer};
use crate::snapshot::{PartyMemberView, TargetView, UiSnapshot};

/// Design sizes, in logical pixels. Multiplied by the interface scale and the element's own.
pub mod size {
    /// Width of the player's frame.
    pub const PLAYER_WIDTH: f32 = 260.0;
    /// Height of the player's frame: one health bar with the name inside it.
    pub const PLAYER_HEIGHT: f32 = 40.0;
    /// Width of the target frame.
    pub const TARGET_WIDTH: f32 = 260.0;
    /// Height of the target frame: one health bar with the name inside it.
    pub const TARGET_HEIGHT: f32 = 40.0;
    /// Width of the target-of-target frame, which shows less.
    pub const TOT_WIDTH: f32 = 170.0;
    /// Height of the target-of-target frame.
    pub const TOT_HEIGHT: f32 = 30.0;
    /// Width of the resource bar.
    pub const RESOURCE_WIDTH: f32 = 260.0;
    /// Height of the resource bar: room for the Druid's two bars, one bar for the others.
    pub const RESOURCE_HEIGHT: f32 = 30.0;
    /// Width of the Ultimate Energy bar.
    pub const ENERGY_WIDTH: f32 = 260.0;
    /// Height of the Ultimate Energy bar.
    pub const ENERGY_HEIGHT: f32 = 14.0;
    /// Width of the party frame.
    pub const PARTY_WIDTH: f32 = 200.0;
    /// Height of one party member's row.
    pub const PARTY_ROW: f32 = 34.0;
    /// Padding inside any frame.
    pub const PADDING: f32 = 6.0;
    /// Height of a bar inside a frame.
    pub const BAR: f32 = 14.0;
    /// Height of a line of text inside a frame.
    pub const LINE: f32 = 14.0;
    /// Corner radius of a unit bar.
    pub const RADIUS: f32 = 5.0;
}

/// The player frame's design size.
#[must_use]
pub fn player_size() -> (f32, f32) {
    (size::PLAYER_WIDTH, size::PLAYER_HEIGHT)
}

/// The target frame's design size.
#[must_use]
pub fn target_size() -> (f32, f32) {
    (size::TARGET_WIDTH, size::TARGET_HEIGHT)
}

/// The target-of-target frame's design size.
#[must_use]
pub fn target_of_target_size() -> (f32, f32) {
    (size::TOT_WIDTH, size::TOT_HEIGHT)
}

/// The party frame's design size, which always has room for the whole party.
///
/// A fixed height rather than one that shrinks to the number of members: the frame is
/// something the player drags to a place on their screen, and one that changed size whenever
/// somebody joined would move under their hand. Rows with nobody in them are simply not
/// drawn.
#[must_use]
pub fn party_size() -> (f32, f32) {
    (
        size::PARTY_WIDTH,
        size::PADDING * 2.0 + size::PARTY_ROW * engine_game::party::MAX_PARTY as f32,
    )
}

/// The design size of whatever `id` is, for the layout to resolve.
///
/// One table, so the painter, the editor and the hit-test cannot disagree about how big a
/// frame is — which would show up as an outline that does not match what it outlines.
#[must_use]
pub fn frame_size(id: super::hud_layout::HudElementId) -> Option<(f32, f32)> {
    use super::hud_layout::HudElementId;
    Some(match id {
        HudElementId::Player => player_size(),
        HudElementId::Target => target_size(),
        HudElementId::TargetOfTarget => target_of_target_size(),
        HudElementId::Party => party_size(),
        HudElementId::Raid => super::raid_frame::raid_size(),
        HudElementId::Buffs => super::aura_row::buff_bar_size(),
        HudElementId::Resource => (size::RESOURCE_WIDTH, size::RESOURCE_HEIGHT),
        HudElementId::Energy => (size::ENERGY_WIDTH, size::ENERGY_HEIGHT),
        // Its size is the player's to set, so it is the layout's to report.
        HudElementId::ActionBar(_)
        | HudElementId::CastBar
        | HudElementId::Tooltip
        | HudElementId::Debuffs
        | HudElementId::TargetBuffs
        | HudElementId::TargetDebuffs => {
            return None;
        }
    })
}

/// Reused text buffers, so drawing a frame allocates nothing after the first one.
#[derive(Debug, Default)]
pub struct FrameScratch {
    health: String,
    detail: String,
    /// A second number buffer, so "current / maximum" needs no temporary.
    total: String,
}

impl FrameScratch {
    /// Empty buffers with room for the numbers a frame prints.
    #[must_use]
    pub fn new() -> Self {
        Self {
            health: String::with_capacity(24),
            detail: String::with_capacity(24),
            total: String::with_capacity(12),
        }
    }
}

/// How much bigger than its design size a rectangle is.
fn factor_of(rect: UiRect, design_width: f32) -> f32 {
    if design_width <= 0.0 {
        1.0
    } else {
        rect.width / design_width
    }
}

/// How big a frame's text is: its design size times how much the frame was scaled.
fn text_size(design: f32, factor: f32) -> f32 {
    design * factor.max(0.25)
}

/// Where a line of text that starts at `top` and is one frame line tall has its centre.
fn line_centre(top: f32, factor: f32) -> f32 {
    top + size::LINE * factor * 0.5
}

/// A bar with its label printed inside it: `label` on the left, `value` on the right.
///
/// The nameplate's bar — dark ground, gradient fill, dark border — with the frame's numbers
/// set in the same edged face as a nameplate's name.
#[allow(
    clippy::too_many_arguments,
    reason = "one row of a frame is this many facts"
)]
fn push_labelled_bar(
    list: &mut UiDrawList,
    theme: &UiTheme,
    rect: UiRect,
    fraction: f32,
    fill: (UiColor, UiColor),
    label: &str,
    value: &str,
    factor: f32,
) {
    let plate = &theme.plate;
    frame_bar(list, rect, fraction, fill, plate);
    let size = text_size(11.5, factor).min(rect.height * 0.9);
    let centre = rect.y + rect.height * 0.5;
    let pad = 4.0 * factor;
    frame_text(
        list,
        rect.x + pad,
        centre,
        label,
        plate.health_text,
        size,
        TextAlign::Left,
        plate,
    );
    frame_text(
        list,
        rect.right() - pad,
        centre,
        value,
        plate.health_text,
        size,
        TextAlign::Right,
        plate,
    );
}

/// A health bar with `current / maximum` centred and the percentage on the right.
#[allow(
    clippy::too_many_arguments,
    reason = "one row of a frame is this many facts"
)]
fn push_health_bar(
    list: &mut UiDrawList,
    scratch: &mut FrameScratch,
    theme: &UiTheme,
    rect: UiRect,
    fraction: f32,
    fill: (UiColor, UiColor),
    current: f32,
    maximum: f32,
    absorb: f32,
    factor: f32,
) {
    let plate = &theme.plate;
    let layers = super::frame_style::HealthLayers {
        absorb: super::frame_style::HealthLayers::of(current, maximum, absorb).absorb,
        ..super::frame_style::HealthLayers::health(fraction)
    };
    super::frame_style::frame_health_bar(list, rect, layers, fill, plate);
    let size = text_size(11.5, factor).min(rect.height * 0.9);
    let centre = rect.y + rect.height * 0.5;
    let pad = 4.0 * factor;

    write_percent(&mut scratch.total, fraction, current);

    // The numbers as they are, then abbreviated (`1.2M / 2.0M`) if they would run into the
    // percentage, then not at all: the percentage is the one number a frame must always show.
    let font = engine_render_data::font::UiFont::SemiBold;
    let room = rect.width - font.width(&scratch.total, size) * 2.0 - pad * 4.0;
    write_health(&mut scratch.health, current, maximum, room, size);
    if font.width(&scratch.health, size) <= room {
        frame_text(
            list,
            rect.x + rect.width * 0.5,
            centre,
            &scratch.health,
            plate.health_text,
            size,
            TextAlign::Center,
            plate,
        );
    }
    frame_text(
        list,
        rect.right() - pad,
        centre,
        &scratch.total,
        plate.health_text,
        size,
        TextAlign::Right,
        plate,
    );
}

/// `82%` into `out`; never `0%` for something still standing.
fn write_percent(out: &mut String, fraction: f32, current: f32) {
    out.clear();
    let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as i64;
    let percent = if percent == 0 && current > 0.0 {
        1
    } else {
        percent
    };
    write_integer(out, percent);
    out.push('%');
}

/// `8200 / 10000` into `out`, abbreviated (`1.2M / 2.0M`) when the full numbers would be
/// wider than `room` at `size`.
fn write_health(out: &mut String, current: f32, maximum: f32, room: f32, size: f32) {
    let font = engine_render_data::font::UiFont::SemiBold;
    out.clear();
    write_integer(out, current.round() as i64);
    out.push_str(" / ");
    write_integer(out, maximum.round() as i64);
    if font.width(out, size) > room {
        out.clear();
        crate::nameplate::write_abbreviated(out, current);
        out.push_str(" / ");
        crate::nameplate::write_abbreviated(out, maximum);
    }
}

/// What a unit bar shows: who, how healthy, and in which colours.
struct UnitBar<'a> {
    /// The name, upper case as the tables spell it; printed in title case.
    name: &'a str,
    /// What the name is printed in.
    name_color: UiColor,
    /// Current health.
    health: f32,
    /// Maximum health.
    max_health: f32,
    /// Every shield on the unit.
    absorb: f32,
    /// Whether it is alive: a dead unit's bar reads "Dead" and is empty.
    alive: bool,
    /// The fill, top and bottom.
    fill: (UiColor, UiColor),
    /// A phase, printed after the name, for something that has more than one.
    phase: Option<usize>,
}

/// **A unit frame that is nothing but its health bar**: the whole rectangle fills with
/// health, the name sits inside it on the left and the numbers on the right — the raid
/// cell's shape, stretched into a frame.
///
/// ```text
/// ┌──────────────────────────────────────────┐
/// │ Name ███████████████░░shield░░   82%     │
/// └──────────────────────────────────────────┘
/// ```
fn push_unit_bar(
    list: &mut UiDrawList,
    scratch: &mut FrameScratch,
    theme: &UiTheme,
    rect: UiRect,
    factor: f32,
    unit: &UnitBar<'_>,
) {
    use engine_render_data::font::UiFont;
    use engine_render_data::ui::UiClip;
    let plate = &theme.plate;
    let radius = (size::RADIUS * factor).round().max(2.0);
    let fraction = if unit.alive && unit.max_health > 0.0 {
        (unit.health / unit.max_health).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // The ground, then the fill: the same rounded shape cut on the right at the health, so
    // it keeps the frame's rounded left corners and ends in a straight edge.
    list.panel(rect, radius, plate.frame_panel, plate.bar_background);
    if fraction > 0.0 {
        list.clip(UiClip::rect(UiRect::new(
            rect.x,
            rect.y,
            (rect.width * fraction).round().max(1.0),
            rect.height,
        )));
        list.panel(rect, radius, unit.fill.0, unit.fill.1);
        list.clip(UiClip::NONE);
    }
    if unit.alive {
        let layers = super::frame_style::HealthLayers {
            absorb: super::frame_style::HealthLayers::of(unit.health, unit.max_health, unit.absorb)
                .absorb,
            ..super::frame_style::HealthLayers::health(fraction)
        };
        let inset = (radius * 0.5).round();
        super::frame_style::push_absorb_layer(
            list,
            UiRect::new(
                rect.x + inset,
                rect.y + inset,
                rect.width - inset * 2.0,
                rect.height - inset * 2.0,
            ),
            layers,
            plate,
            1.0,
        );
    }
    list.panel_stroke(rect, radius, factor.max(1.0).round(), plate.frame_border);

    // The numbers on the right first, so the name knows how much room it has.
    let pad = (8.0 * factor).round();
    let centre = rect.y + rect.height * 0.5;
    let number_size = text_size(12.5, factor).min(rect.height * 0.6);
    let font = UiFont::SemiBold;
    let right = if unit.alive {
        write_percent(&mut scratch.total, fraction, unit.health);
        let room = rect.width * 0.65;
        write_health(
            &mut scratch.health,
            unit.health,
            unit.max_health,
            room - font.width(&scratch.total, number_size) - pad,
            number_size,
        );
        // On a narrow frame the percentage alone: it is the number a frame must show.
        if font.width(&scratch.health, number_size) + font.width(&scratch.total, number_size) + pad
            <= room
        {
            scratch.health.push_str("   ");
            scratch.health.push_str(&scratch.total);
        } else {
            scratch.health.clear();
            scratch.health.push_str(&scratch.total);
        }
        plate.health_text
    } else {
        scratch.health.clear();
        scratch.health.push_str("Dead");
        theme.death_text
    };
    frame_text(
        list,
        rect.right() - pad,
        centre,
        &scratch.health,
        right,
        number_size,
        TextAlign::Right,
        plate,
    );

    // The name, cut where the numbers begin rather than printed over them.
    crate::ui::map_style::write_title_case(&mut scratch.detail, unit.name);
    if let Some(phase) = unit.phase {
        scratch.detail.push_str("  ·  Phase ");
        write_integer(&mut scratch.detail, phase as i64);
    }
    let numbers = font.width(&scratch.health, number_size);
    let name_right = (rect.right() - pad * 2.0 - numbers).max(rect.x + pad);
    list.clip(UiClip::rect(UiRect::new(
        rect.x,
        rect.y,
        name_right - rect.x,
        rect.height,
    )));
    frame_text(
        list,
        rect.x + pad,
        centre,
        &scratch.detail,
        unit.name_color,
        text_size(13.5, factor).min(rect.height * 0.62),
        TextAlign::Left,
        plate,
    );
    list.clip(UiClip::NONE);
}

/// Draws the player's frame: their health, with their name inside it.
///
/// Nothing else. The class's resource and the Ultimate's energy are bars of their own, and
/// every effect on the player — Deflection's bonus, a shield, respawn protection — is an
/// icon in the buff and debuff blocks, where it has a tooltip.
pub fn push_player_frame(
    list: &mut UiDrawList,
    rect: UiRect,
    snapshot: &UiSnapshot,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
) {
    let factor = factor_of(rect, size::PLAYER_WIDTH);
    let player = &snapshot.player;
    push_unit_bar(
        list,
        scratch,
        theme,
        rect,
        factor,
        &UnitBar {
            name: "YOU",
            name_color: theme.plate.name_target,
            health: player.health,
            max_health: player.max_health,
            absorb: player.absorb,
            alive: player.alive,
            fill: friendly_fill(&theme.plate, player.health_fraction()),
            phase: None,
        },
    );
}

/// **The hero's own resource, as a bar of its own.**
///
/// The Warrior has Deflection, the FireMage Heat, the Druid Mana over Astral Power — one
/// element, one branch, and no hero is shown a bar that can never move. The Druid's two bars
/// split the element's height; everyone else's one bar fills it.
pub fn push_resource_bar(
    list: &mut UiDrawList,
    rect: UiRect,
    snapshot: &UiSnapshot,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
) {
    let factor = factor_of(rect, size::RESOURCE_WIDTH);
    let player = &snapshot.player;
    if player.druid.active {
        let gap = (2.0 * factor).round();
        let half = ((rect.height - gap) * 0.5).floor();
        let druid = &player.druid;
        scratch.detail.clear();
        write_integer(&mut scratch.detail, druid.mana.round() as i64);
        push_labelled_bar(
            list,
            theme,
            UiRect::new(rect.x, rect.y, rect.width, half),
            druid.mana_fraction(),
            shaded(theme.mana),
            "Mana",
            &scratch.detail,
            factor,
        );
        // The armed state is a word on the bar rather than a colour alone, so it survives
        // being read at a glance and by somebody who cannot tell the two purples apart.
        scratch.detail.clear();
        write_integer(&mut scratch.detail, druid.astral.round() as i64);
        if druid.armed {
            scratch.detail.push_str("  Armed");
        } else if druid.can_arm {
            scratch.detail.push_str("  *");
        }
        push_labelled_bar(
            list,
            theme,
            UiRect::new(rect.x, rect.bottom() - half, rect.width, half),
            druid.astral_fraction(),
            shaded(theme.astral_color(druid.armed)),
            "Astral",
            &scratch.detail,
            factor,
        );
        return;
    }
    scratch.detail.clear();
    if player.warrior.active {
        let warrior = &player.warrior;
        write_integer(&mut scratch.detail, warrior.deflection.round() as i64);
        // The empowered marker, on the resource itself: twenty banked is an ability's worth.
        if warrior.can_empower() {
            scratch.detail.push_str("  *");
        }
        push_labelled_bar(
            list,
            theme,
            rect,
            warrior.deflection_fraction(),
            shaded(theme.deflection_color(warrior.is_capped())),
            "Deflection",
            &scratch.detail,
            factor,
        );
        return;
    }
    write_integer(&mut scratch.detail, player.heat.round() as i64);
    push_labelled_bar(
        list,
        theme,
        rect,
        player.heat_fraction(),
        shaded(theme.heat_color(player.heat_tier)),
        "Heat",
        &scratch.detail,
        factor,
    );
}

/// The Ultimate's energy, as a bar of its own.
pub fn push_energy_bar(
    list: &mut UiDrawList,
    rect: UiRect,
    snapshot: &UiSnapshot,
    theme: &UiTheme,
) {
    let player = &snapshot.player;
    push_labelled_bar(
        list,
        theme,
        rect,
        player.energy_fraction(),
        shaded(if player.ultimate_ready {
            theme.energy_full
        } else {
            theme.energy
        }),
        "Energy",
        if player.ultimate_ready { "Ready" } else { "" },
        factor_of(rect, size::ENERGY_WIDTH),
    );
}

/// Draws a frame for a target, or its empty state when there is nothing selected.
///
/// The same single bar as the player's frame. `empty_label` is what the frame says when it
/// is empty, so a player looking at two empty boxes can tell which is which.
pub fn push_target_frame(
    list: &mut UiDrawList,
    rect: UiRect,
    target: Option<&TargetView>,
    empty_label: &str,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
    design_width: f32,
) {
    let factor = factor_of(rect, design_width);
    let plate = &theme.plate;

    let Some(target) = target else {
        // The empty state is drawn, not hidden. A frame that disappeared would take its
        // place in the layout with it, and the player would have nothing to aim at.
        frame_panel(list, rect, plate, factor);
        crate::ui::map_style::write_title_case(&mut scratch.detail, empty_label);
        frame_text(
            list,
            rect.x + rect.width * 0.5,
            rect.y + rect.height * 0.5,
            &scratch.detail,
            plate.name_dead,
            text_size(13.0, factor).min(rect.height * 0.6),
            TextAlign::Center,
            plate,
        );
        return;
    };

    let hostile = matches!(
        target.kind,
        engine_game::world::EntityKind::Enemy | engine_game::world::EntityKind::Dummy
    );
    let fraction = target.health_fraction();
    push_unit_bar(
        list,
        scratch,
        theme,
        rect,
        factor,
        &UnitBar {
            name: target.name,
            name_color: if !target.alive {
                plate.name_dead
            } else if target.boss {
                plate.name_boss
            } else {
                plate.name
            },
            health: target.health,
            max_health: target.max_health,
            absorb: target.absorb,
            alive: target.alive,
            fill: if hostile {
                hostile_fill(plate, fraction)
            } else {
                friendly_fill(plate, fraction)
            },
            // The phase, after the name, for something that has more than one: the boss
            // encounter's only piece of dedicated interface.
            phase: target.phase.map(|(phase, _)| phase),
        },
    );
}

/// Draws the party: one row per member, in roster order, local player first.
///
/// Rows nobody is in are left blank rather than filled with placeholders, and the frame is
/// the same size either way — so a member joining fills a row instead of moving the frame.
pub fn push_party_frame(
    list: &mut UiDrawList,
    rect: UiRect,
    members: &[PartyMemberView],
    upwards: bool,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
) {
    let factor = factor_of(rect, size::PARTY_WIDTH);
    let padding = size::PADDING * factor;
    let plate = &theme.plate;
    frame_panel(list, rect, plate, factor);
    let inner = rect.inset(padding);
    let row_height = size::PARTY_ROW * factor;

    for (index, member) in members.iter().enumerate() {
        let top = inner.y + row_height * party_row(index, upwards) as f32;
        if top + row_height > rect.bottom() {
            // More members than the frame was laid out for. Drawing them outside it would
            // put health bars over whatever is underneath.
            break;
        }
        crate::ui::map_style::write_title_case(&mut scratch.detail, member.name());
        frame_text(
            list,
            inner.x,
            line_centre(top, factor),
            &scratch.detail,
            if member.alive {
                plate.name
            } else {
                plate.name_dead
            },
            text_size(12.5, factor),
            TextAlign::Left,
            plate,
        );
        let fraction = member.health_fraction();
        let bar = UiRect::new(
            inner.x,
            top + size::LINE * factor,
            inner.width,
            size::BAR * factor,
        );
        push_health_bar(
            list,
            scratch,
            theme,
            bar,
            if member.alive { fraction } else { 0.0 },
            friendly_fill(plate, fraction),
            member.health,
            member.max_health,
            if member.alive { member.absorb } else { 0.0 },
            factor,
        );
    }
}

/// Draws every enabled frame, in one call: the four unit frames, the raid frames and the
/// buff bar, with the debuffs each frame carries — the player's under their frame, each
/// member's beside their party row and inside their raid cell. Every icon is recorded in
/// `hits` for the tooltip.
///
/// The order is painter's order and nothing else: the frames do not overlap in the default
/// layout, and a player who drags two on top of each other gets the later one on top.
#[allow(
    clippy::too_many_arguments,
    reason = "the frames read from this many independent sources"
)]
pub fn push_frames(
    list: &mut UiDrawList,
    layout: &super::hud_layout::HudLayout,
    snapshot: &UiSnapshot,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
    hits: &mut super::aura_row::AuraHits,
    ui_scale: UiScale,
    viewport: (u32, u32),
) {
    use super::hud_layout::HudElementId;

    let rect_of = |id: HudElementId| -> Option<UiRect> {
        if !layout.is_enabled(id) {
            return None;
        }
        Some(layout.rect_of(id, ui_scale, viewport))
    };

    if let Some(rect) = rect_of(HudElementId::Player) {
        push_player_frame(list, rect, snapshot, theme, scratch);
    }
    if let Some(rect) = rect_of(HudElementId::Resource) {
        push_resource_bar(list, rect, snapshot, theme, scratch);
    }
    if let Some(rect) = rect_of(HudElementId::Energy) {
        push_energy_bar(list, rect, snapshot, theme);
    }
    if let Some(rect) = rect_of(HudElementId::Target) {
        push_target_frame(
            list,
            rect,
            snapshot.target.as_ref(),
            "NO TARGET",
            theme,
            scratch,
            size::TARGET_WIDTH,
        );
    }
    if let Some(rect) = rect_of(HudElementId::TargetOfTarget) {
        push_target_frame(
            list,
            rect,
            snapshot.target_of_target.as_ref(),
            "NO TARGET",
            theme,
            scratch,
            size::TOT_WIDTH,
        );
    }
    if let Some(rect) = rect_of(HudElementId::Party) {
        let upwards = layout.direction(HudElementId::Party).bottom_to_top;
        push_party_frame(list, rect, &snapshot.party, upwards, theme, scratch);
        super::aura_row::push_party_debuffs(
            list,
            hits,
            rect,
            &snapshot.party,
            upwards,
            &mut scratch.detail,
        );
    }
    if let Some(rect) = rect_of(HudElementId::Raid) {
        let flow = layout.raid_flow(rect);
        super::raid_frame::push_raid_frame(
            list,
            &flow,
            &snapshot.party,
            snapshot.target.as_ref().map(|target| target.entity),
            &theme.plate,
            &mut scratch.detail,
        );
        super::aura_row::push_raid_debuffs(list, hits, &flow, &snapshot.party, &mut scratch.detail);
    }
    // The four blocks of effects: the player's buffs and debuffs, and the target's.
    let target_auras = snapshot.target.as_ref().map(|target| &target.auras);
    for (id, auras) in [
        (HudElementId::Buffs, Some(&snapshot.player.auras)),
        (HudElementId::Debuffs, Some(&snapshot.player.auras)),
        (HudElementId::TargetBuffs, target_auras),
        (HudElementId::TargetDebuffs, target_auras),
    ] {
        let (Some(rect), Some(auras)) = (rect_of(id), auras) else {
            continue;
        };
        let flow = layout.aura_flow(id, rect);
        if matches!(id, HudElementId::Buffs | HudElementId::TargetBuffs) {
            super::aura_row::push_aura_block(list, hits, &flow, auras.buffs(), &mut scratch.detail);
        } else {
            super::aura_row::push_aura_block(
                list,
                hits,
                &flow,
                auras.debuffs(),
                &mut scratch.detail,
            );
        }
    }
}

/// Whether a placement would put a frame under a point. For hit-testing outside edit mode.
#[must_use]
pub fn frame_contains(placement: HudPlacement, rect: UiRect, pointer: (f32, f32)) -> bool {
    placement.enabled && rect.contains(pointer.0, pointer.1)
}

#[cfg(test)]
fn push_party_frame_down(
    list: &mut UiDrawList,
    rect: UiRect,
    members: &[PartyMemberView],
    theme: &UiTheme,
    scratch: &mut FrameScratch,
) {
    push_party_frame(list, rect, members, false, theme, scratch);
}

/// Which of the frame's rows member `index` is drawn in: from the top, or — for a frame the
/// player turned upwards — from the bottom, the first member nearest the bottom edge.
#[must_use]
pub fn party_row(index: usize, upwards: bool) -> usize {
    if upwards {
        engine_game::party::MAX_PARTY
            .saturating_sub(1)
            .saturating_sub(index)
    } else {
        index
    }
}

/// Which party row `pointer` is on, if it is on one.
///
/// The counterpart of [`action_bar::slot_at`](super::action_bar::slot_at), and derived from
/// the same numbers [`push_party_frame`] lays the rows out with — so the row a player clicks
/// is the row they were looking at. Rows past the end of the roster are not rows: an empty
/// place in the frame is not a member to select.
#[must_use]
pub fn party_row_at(
    rect: UiRect,
    members: usize,
    upwards: bool,
    pointer: (f32, f32),
) -> Option<usize> {
    if members == 0 || !rect.contains(pointer.0, pointer.1) {
        return None;
    }
    let factor = factor_of(rect, size::PARTY_WIDTH);
    let padding = size::PADDING * factor;
    let inner = rect.inset(padding);
    let row_height = size::PARTY_ROW * factor;
    if row_height <= 0.0 || pointer.1 < inner.y {
        return None;
    }
    let row = ((pointer.1 - inner.y) / row_height) as usize;
    // The row back to the member drawn in it: the inverse of `party_row`.
    let index = if upwards {
        engine_game::party::MAX_PARTY
            .checked_sub(1)
            .and_then(|last| last.checked_sub(row))?
    } else {
        row
    };
    (index < members).then_some(index)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::ui::hud_layout::{HudElementId, HudLayout};
    use engine_game::arena::{ArenaConfig, training_arena};
    use engine_game::sim::Simulation;
    use engine_render_data::ui::UiCommand;

    const FHD: (u32, u32) = (1920, 1080);

    fn simulation() -> Simulation {
        training_arena(&ArenaConfig::default())
    }

    fn snapshot(simulation: &Simulation) -> UiSnapshot {
        UiSnapshot::build(simulation)
    }

    /// Every string the draw list holds, so a test can assert on what a frame printed.
    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } | UiCommand::StyledLabel { range, .. } => {
                    Some(list.text_of(*range).to_owned())
                }
                _ => None,
            })
            .collect()
    }

    /// Draws every enabled frame at 1080p and returns the list.
    fn frames(layout: &HudLayout, snapshot: &UiSnapshot) -> UiDrawList {
        let mut list = UiDrawList::new();
        let mut scratch = FrameScratch::new();
        push_frames(
            &mut list,
            layout,
            snapshot,
            &UiTheme::default(),
            &mut scratch,
            &mut super::super::aura_row::AuraHits::new(),
            UiScale::default(),
            FHD,
        );
        list
    }

    /// Every icon drawn, with where it was drawn.
    fn drawn_icons(list: &UiDrawList) -> Vec<(engine_render_data::ui::UiIcon, UiRect)> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Icon { icon, rect, .. } => Some((*icon, *rect)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_debuff_on_the_player_hangs_under_their_frame_and_on_their_party_row() {
        use engine_game::status::{Status, StatusKind};
        use engine_render_data::ui::UiIcon;
        let mut simulation = simulation();
        let player = simulation.player().entity;
        simulation
            .world_mut()
            .statuses_mut(player)
            .expect("the player carries a status set")
            .apply(Status::new(
                StatusKind::Poisoned,
                player,
                engine_game::firemage::FireAbility::Firebolt.into(),
                10.0,
                1.0,
            ));
        let snapshot = snapshot(&simulation);
        let mut layout = HudLayout::default();
        // The party list is hidden by default; this is about the row it draws when shown.
        layout.set_enabled(HudElementId::Party, true);
        let mut list = UiDrawList::new();
        let mut scratch = FrameScratch::new();
        let mut hits = super::super::aura_row::AuraHits::new();
        push_frames(
            &mut list,
            &layout,
            &snapshot,
            &UiTheme::default(),
            &mut scratch,
            &mut hits,
            UiScale::default(),
            FHD,
        );
        let frame = layout.rect_of(HudElementId::Player, UiScale::default(), FHD);
        let party = layout.rect_of(HudElementId::Party, UiScale::default(), FHD);
        let poisons: Vec<UiRect> = drawn_icons(&list)
            .into_iter()
            .filter(|(icon, _)| *icon == UiIcon::AuraPoisoned)
            .map(|(_, rect)| rect)
            .collect();
        assert!(
            poisons
                .iter()
                .any(|rect| rect.y >= frame.bottom() && rect.x >= frame.x - 0.5),
            "no poison under the player frame: {poisons:?}"
        );
        assert!(
            poisons.iter().any(|rect| rect.x >= party.right()),
            "no poison beside the party row: {poisons:?}"
        );
        // A poison can be dispelled, so its icon is framed in the dispel blue.
        assert!(list.commands().iter().any(|command| matches!(
            command,
            UiCommand::Outline { color, .. }
                if *color == super::super::aura_row::DISPEL_BORDER
        )));
        // And each one can be hovered for its tooltip.
        let centre = (poisons[0].x + 2.0, poisons[0].y + 2.0);
        assert!(hits.at(centre).is_some());
    }

    #[test]
    fn the_buff_bar_shows_the_player_s_buffs_where_it_was_put() {
        use engine_game::status::{Status, StatusKind};
        use engine_render_data::ui::UiIcon;
        let mut simulation = simulation();
        let player = simulation.player().entity;
        simulation
            .world_mut()
            .statuses_mut(player)
            .expect("the player carries a status set")
            .apply(Status::new(
                StatusKind::WildRush,
                player,
                engine_game::firemage::FireAbility::WildRush.into(),
                6.0,
                0.0,
            ));
        let snapshot = snapshot(&simulation);
        let mut layout = HudLayout::default();
        let list = frames(&layout, &snapshot);
        let bar = layout.rect_of(HudElementId::Buffs, UiScale::default(), FHD);
        let rush = drawn_icons(&list)
            .into_iter()
            .find(|(icon, _)| *icon == UiIcon::WildRush)
            .map(|(_, rect)| rect)
            .expect("Wild Rush is in the buff bar");
        assert!(
            rush.x >= bar.x && rush.right() <= bar.right() + 0.5,
            "{rush:?} {bar:?}"
        );
        assert!(rush.y >= bar.y && rush.bottom() <= bar.bottom() + 0.5);

        // Hidden in Edit HUD, it draws nothing at all.
        layout.set_enabled(HudElementId::Buffs, false);
        let list = frames(&layout, &snapshot);
        assert!(
            !drawn_icons(&list)
                .iter()
                .any(|(icon, _)| *icon == UiIcon::WildRush)
        );
    }

    #[test]
    fn the_player_frame_shows_the_player() {
        let simulation = simulation();
        let snapshot = snapshot(&simulation);
        let list = frames(&HudLayout::default(), &snapshot);
        let printed = texts(&list);

        assert!(printed.iter().any(|text| text == "You"), "{printed:?}");
        assert!(
            printed.iter().any(|text| text == "Heat"),
            "no Heat: {printed:?}"
        );
        assert!(
            printed.iter().any(|text| text == "Energy"),
            "no Energy: {printed:?}"
        );
        // The health the simulation actually has, not a number this file made up.
        let health = snapshot.player.health.round() as i64;
        assert!(
            printed
                .iter()
                .any(|text| text.starts_with(&format!("{health} / "))),
            "health {health} is not printed: {printed:?}"
        );
    }

    #[test]
    fn the_target_frame_says_so_when_nothing_is_selected() {
        let simulation = simulation();
        let mut snapshot = snapshot(&simulation);
        snapshot.target = None;
        snapshot.target_of_target = None;

        let printed = texts(&frames(&HudLayout::default(), &snapshot));
        assert!(
            printed.iter().filter(|text| *text == "No Target").count() >= 2,
            "the empty frames printed nothing: {printed:?}"
        );
    }

    #[test]
    fn the_target_frame_shows_the_selected_target() {
        let mut simulation = simulation();
        simulation.select_nearest_target();
        let snapshot = snapshot(&simulation);
        let target = snapshot.target.as_ref().expect("a target was selected");

        let printed = texts(&frames(&HudLayout::default(), &snapshot));
        let name = crate::ui::map_style::title_case(target.name);
        assert!(printed.contains(&name), "{printed:?}");
        let health = target.health.round() as i64;
        assert!(
            printed
                .iter()
                .any(|text| text.starts_with(&format!("{health} / "))),
            "the target's health is not printed: {printed:?}"
        );
    }

    #[test]
    fn the_target_of_target_frame_follows_the_chain() {
        let mut simulation = simulation();
        simulation.select_nearest_target();
        let mut snapshot = snapshot(&simulation);
        // The dummy in the training arena fights nobody, so the chain ends there. Give the
        // frame something to show the way an enemy attacking the player would.
        snapshot.target_of_target = snapshot.target;

        let list = frames(&HudLayout::default(), &snapshot);
        let printed = texts(&list);
        // Both frames now name the same unit, which is exactly the chain being followed.
        let name =
            crate::ui::map_style::title_case(snapshot.target.as_ref().expect("a target").name);
        assert!(
            printed.iter().filter(|text| **text == name).count() >= 2,
            "{printed:?}"
        );
    }

    #[test]
    fn the_party_frame_grows_and_shrinks_with_the_roster() {
        let simulation = simulation();
        let mut snapshot = snapshot(&simulation);
        // The player and the friendly dummy the main hall stands them next to.
        assert_eq!(snapshot.party.len(), 2);
        snapshot.party.truncate(1);

        let solo = texts(&frames(&HudLayout::default(), &snapshot));
        assert!(solo.iter().filter(|text| **text == "You").count() >= 1);

        // A member joins. Nothing about the frame changes except that a row fills in.
        let mut extra = snapshot.party[0].clone();
        extra.is_player = false;
        extra.kind = engine_game::world::EntityKind::Enemy;
        snapshot.party.push(extra);
        let grown = texts(&frames(&HudLayout::default(), &snapshot));
        assert!(grown.len() > solo.len(), "the second member drew nothing");

        // And leaving puts it back.
        snapshot.party.truncate(1);
        assert_eq!(texts(&frames(&HudLayout::default(), &snapshot)), solo);
    }

    #[test]
    fn the_party_frame_is_the_same_size_whoever_is_in_it() {
        // The frame is something the player drags to a place. One that resized when somebody
        // joined would move under their hand.
        let layout = HudLayout::default();
        let before = layout.resolve(HudElementId::Party, party_size(), UiScale::default(), FHD);
        assert!(before.height >= size::PARTY_ROW * engine_game::party::MAX_PARTY as f32);
        let after = layout.resolve(HudElementId::Party, party_size(), UiScale::default(), FHD);
        assert_eq!(before, after);
    }

    #[test]
    fn a_disabled_frame_draws_nothing() {
        let simulation = simulation();
        let snapshot = snapshot(&simulation);
        let mut layout = HudLayout::default();
        for id in HudElementId::FRAMES.into_iter().chain(HudElementId::EXTRAS) {
            layout.set_enabled(id, false);
        }
        let list = frames(&layout, &snapshot);
        assert!(
            list.is_empty(),
            "a hidden frame drew {} commands",
            list.len()
        );
    }

    #[test]
    fn every_frame_draws_inside_the_rectangle_the_layout_gave_it() {
        let simulation = simulation();
        let mut simulation = simulation;
        simulation.select_nearest_target();
        let snapshot = snapshot(&simulation);
        let layout = HudLayout::default();

        for id in HudElementId::FRAMES {
            let size = frame_size(id).expect("a frame has a size");
            let rect = layout.resolve(id, size, UiScale::default(), FHD);
            let mut list = UiDrawList::new();
            let mut scratch = FrameScratch::new();
            match id {
                HudElementId::Player => {
                    push_player_frame(
                        &mut list,
                        rect,
                        &snapshot,
                        &UiTheme::default(),
                        &mut scratch,
                    );
                }
                HudElementId::Target => push_target_frame(
                    &mut list,
                    rect,
                    snapshot.target.as_ref(),
                    "NO TARGET",
                    &UiTheme::default(),
                    &mut scratch,
                    size::TARGET_WIDTH,
                ),
                HudElementId::TargetOfTarget => push_target_frame(
                    &mut list,
                    rect,
                    None,
                    "NO TARGET",
                    &UiTheme::default(),
                    &mut scratch,
                    size::TOT_WIDTH,
                ),
                HudElementId::Party => push_party_frame_down(
                    &mut list,
                    rect,
                    &snapshot.party,
                    &UiTheme::default(),
                    &mut scratch,
                ),
                HudElementId::ActionBar(_)
                | HudElementId::Tooltip
                | HudElementId::Raid
                | HudElementId::CastBar
                | HudElementId::Buffs
                | HudElementId::Debuffs
                | HudElementId::TargetBuffs
                | HudElementId::TargetDebuffs
                | HudElementId::Resource
                | HudElementId::Energy => {
                    continue;
                }
            }
            for command in list.commands() {
                let drawn = match command {
                    UiCommand::Rect { rect, .. } => *rect,
                    _ => continue,
                };
                assert!(
                    drawn.x >= rect.x - 0.01
                        && drawn.y >= rect.y - 0.01
                        && drawn.right() <= rect.right() + 0.01
                        && drawn.bottom() <= rect.bottom() + 0.01,
                    "{id:?} drew {drawn:?} outside {rect:?}"
                );
            }
        }
    }

    #[test]
    fn a_scaled_frame_draws_bigger_without_moving_off_its_rectangle() {
        let simulation = simulation();
        let snapshot = snapshot(&simulation);
        let mut layout = HudLayout::default();
        layout.set_scale(HudElementId::Player, 2.0);

        let rect = layout.resolve(HudElementId::Player, player_size(), UiScale::default(), FHD);
        assert!((rect.width - size::PLAYER_WIDTH * 2.0).abs() < 0.01);

        let mut list = UiDrawList::new();
        let mut scratch = FrameScratch::new();
        push_player_frame(
            &mut list,
            rect,
            &snapshot,
            &UiTheme::default(),
            &mut scratch,
        );
        for command in list.commands() {
            if let UiCommand::Rect { rect: drawn, .. } = command {
                assert!(drawn.right() <= rect.right() + 0.01, "{drawn:?}");
            }
        }
    }

    #[test]
    fn drawing_the_frames_allocates_nothing_after_the_first_frame() {
        let simulation = simulation();
        let snapshot = snapshot(&simulation);
        let layout = HudLayout::default();
        let mut list = UiDrawList::new();
        let mut scratch = FrameScratch::new();
        let theme = UiTheme::default();

        for _ in 0..4 {
            list.clear();
            push_frames(
                &mut list,
                &layout,
                &snapshot,
                &theme,
                &mut scratch,
                &mut super::super::aura_row::AuraHits::new(),
                UiScale::default(),
                FHD,
            );
        }
        let health = scratch.health.capacity();
        let detail = scratch.detail.capacity();
        let total = scratch.total.capacity();
        for _ in 0..500 {
            list.clear();
            push_frames(
                &mut list,
                &layout,
                &snapshot,
                &theme,
                &mut scratch,
                &mut super::super::aura_row::AuraHits::new(),
                UiScale::default(),
                FHD,
            );
        }
        assert_eq!(scratch.health.capacity(), health, "the scratch grew");
        assert_eq!(scratch.detail.capacity(), detail, "the scratch grew");
        assert_eq!(scratch.total.capacity(), total, "the scratch grew");
    }

    #[test]
    fn a_party_row_is_found_where_it_was_drawn() {
        // The row a player clicks has to be the row they were looking at, so the hit test is
        // built from the same numbers the painter lays the rows out with.
        let layout = HudLayout::default();
        let rect = layout.rect_of(HudElementId::Party, UiScale::default(), FHD);
        let factor = factor_of(rect, size::PARTY_WIDTH);
        let inner = rect.inset(size::PADDING * factor);
        let row_height = size::PARTY_ROW * factor;

        // A party of three: the first three rows answer, the fourth does not.
        for index in 0..3 {
            let middle = (
                inner.x + inner.width * 0.5,
                inner.y + row_height * (index as f32 + 0.5),
            );
            assert_eq!(party_row_at(rect, 3, false, middle), Some(index));
        }
        let empty = (inner.x + inner.width * 0.5, inner.y + row_height * 3.5);
        assert_eq!(
            party_row_at(rect, 3, false, empty),
            None,
            "an empty place in the frame is not a member to select"
        );

        // Outside the frame, and a party of nobody.
        assert_eq!(
            party_row_at(rect, 3, false, (rect.x - 5.0, rect.y - 5.0)),
            None
        );
        assert_eq!(party_row_at(rect, 0, false, (inner.x, inner.y)), None);

        // Turned upwards, the first member is in the bottom row and the hit test agrees.
        let rows = engine_game::party::MAX_PARTY;
        for index in 0..3 {
            let row = party_row(index, true);
            assert_eq!(row, rows - 1 - index);
            let middle = (
                inner.x + inner.width * 0.5,
                inner.y + row_height * (row as f32 + 0.5),
            );
            assert_eq!(party_row_at(rect, 3, true, middle), Some(index));
        }
    }

    #[test]
    fn a_frame_prints_only_glyphs_the_font_has() {
        // The interface font covers its charset; anything else would draw as a question mark.
        let known = |character: char| {
            character == '?'
                || engine_render_data::font::glyph_index(character)
                    != engine_render_data::font::glyph_index('?')
        };
        let mut simulation = simulation();
        simulation.select_nearest_target();
        let snapshot = snapshot(&simulation);
        for text in texts(&frames(&HudLayout::default(), &snapshot)) {
            assert!(
                text.chars().all(known),
                "{text:?} has a character with no glyph"
            );
        }
    }

    #[test]
    fn the_frames_are_drawn_like_the_nameplates() {
        // Edged interface type and bordered gradient bars — no bitmap text at all.
        let mut simulation = simulation();
        simulation.select_nearest_target();
        let snapshot = snapshot(&simulation);
        let list = frames(&HudLayout::default(), &snapshot);
        assert!(
            !list
                .commands()
                .iter()
                .any(|command| matches!(command, UiCommand::Text { .. })),
            "a frame still draws bitmap text"
        );
        for command in list.commands() {
            if let UiCommand::StyledLabel { style, .. } = command {
                assert!(style.has_outline());
            }
        }
        assert!(
            list.commands()
                .iter()
                .any(|command| matches!(command, UiCommand::Panel { radius, .. } if *radius > 0.0)),
            "the panels are rounded"
        );
    }
}
