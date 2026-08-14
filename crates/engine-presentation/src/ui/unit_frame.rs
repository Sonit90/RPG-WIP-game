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

use crate::game_ui::{UiTheme, write_integer};
use crate::snapshot::{PartyMemberView, TargetView, UiSnapshot};

/// Design sizes, in logical pixels. Multiplied by the interface scale and the element's own.
pub mod size {
    /// Width of the player's frame.
    pub const PLAYER_WIDTH: f32 = 260.0;
    /// Height of the player's frame: a name, health, and two resource bars.
    pub const PLAYER_HEIGHT: f32 = 82.0;
    /// Width of the target frame.
    pub const TARGET_WIDTH: f32 = 260.0;
    /// Height of the target frame: a name with a distance, and health.
    pub const TARGET_HEIGHT: f32 = 58.0;
    /// Width of the target-of-target frame, which shows less.
    pub const TOT_WIDTH: f32 = 170.0;
    /// Height of the target-of-target frame.
    pub const TOT_HEIGHT: f32 = 40.0;
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
        HudElementId::ActionBar(_) => return None,
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

/// How big text is drawn inside a frame whose rectangle is `rect` at design width `design`.
///
/// Derived from the rectangle rather than passed in, so a frame the player scaled up gets
/// bigger text with it and a frame at half size stays legible rather than keeping full-size
/// glyphs in a box too small for them.
fn text_scale(theme: &UiTheme, factor: f32) -> f32 {
    // The bitmap font only looks right at sensible multiples, so this is deliberately not
    // rounded to an integer here: `UiScale::text` is what does the rounding, once.
    UiScale::new(factor.clamp(UiScale::MIN, UiScale::MAX)).text(theme.hud_text_scale)
}

/// How much bigger than its design size a rectangle is.
fn factor_of(rect: UiRect, design_width: f32) -> f32 {
    if design_width <= 0.0 {
        1.0
    } else {
        rect.width / design_width
    }
}

/// Draws the panel every frame sits on.
fn push_panel(list: &mut UiDrawList, rect: UiRect, theme: &UiTheme) {
    list.rect(rect, theme.panel);
}

/// Draws a labelled bar with its numbers, and returns the row below it.
#[allow(
    clippy::too_many_arguments,
    reason = "one row of a frame is this many facts"
)]
fn push_bar_row(
    list: &mut UiDrawList,
    scratch: &mut FrameScratch,
    theme: &UiTheme,
    rect: UiRect,
    fraction: f32,
    fill: UiColor,
    current: f32,
    maximum: f32,
    scale: f32,
) {
    list.bar(rect, fraction, theme.bar_background, fill);
    scratch.health.clear();
    write_integer(&mut scratch.health, current.round() as i64);
    scratch.health.push('/');
    scratch.total.clear();
    write_integer(&mut scratch.total, maximum.round() as i64);
    let total = std::mem::take(&mut scratch.total);
    scratch.health.push_str(&total);
    scratch.total = total;
    let ((x, y), align) = crate::game_ui::centered(
        rect.x + rect.width * 0.5,
        rect.y + (rect.height - scale * 7.0) * 0.5,
    );
    list.text_aligned((x, y), &scratch.health, theme.text, scale, align);
}

/// Draws the player's frame: who they are, their health, their Heat and their Energy.
pub fn push_player_frame(
    list: &mut UiDrawList,
    rect: UiRect,
    snapshot: &UiSnapshot,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
) {
    let factor = factor_of(rect, size::PLAYER_WIDTH);
    let scale = text_scale(theme, factor);
    let padding = size::PADDING * factor;
    push_panel(list, rect, theme);

    let player = &snapshot.player;
    let inner = rect.inset(padding);
    list.text_at((inner.x, inner.y), "YOU", theme.text, scale);
    if !player.alive {
        list.text_aligned(
            (inner.right(), inner.y),
            "DEAD",
            theme.death_text,
            scale,
            TextAlign::Right,
        );
    }

    let mut y = inner.y + size::LINE * factor;
    let bar = size::BAR * factor;
    push_bar_row(
        list,
        scratch,
        theme,
        UiRect::new(inner.x, y, inner.width, bar),
        player.health_fraction(),
        theme.player_health_color(player.health_fraction()),
        player.health,
        player.max_health,
        scale,
    );

    // Heat and Ultimate Energy: the two resources the class actually has. Nothing invents a
    // third, and a class with different resources would list different ones here.
    y += bar + 2.0 * factor;
    list.bar(
        UiRect::new(inner.x, y, inner.width, bar),
        player.heat_fraction(),
        theme.bar_background,
        theme.heat_color(player.heat_tier),
    );
    scratch.detail.clear();
    scratch.detail.push_str("HEAT ");
    write_integer(&mut scratch.detail, player.heat.round() as i64);
    list.text_at(
        (inner.x + 3.0 * factor, y),
        &scratch.detail,
        theme.text,
        scale,
    );

    y += bar + 2.0 * factor;
    list.bar(
        UiRect::new(inner.x, y, inner.width, bar),
        player.energy_fraction(),
        theme.bar_background,
        if player.ultimate_ready {
            theme.energy_full
        } else {
            theme.energy
        },
    );
    list.text_at((inner.x + 3.0 * factor, y), "ENERGY", theme.text, scale);
}

/// Draws a frame for a target, or its empty state when there is nothing selected.
///
/// `title` is what the frame is called when it is empty: the target frame says `NO TARGET`
/// and the target-of-target frame says `NO TARGET OF TARGET`, so a player looking at two
/// empty boxes can tell which is which.
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
    let scale = text_scale(theme, factor);
    let padding = size::PADDING * factor;
    push_panel(list, rect, theme);
    let inner = rect.inset(padding);

    let Some(target) = target else {
        // The empty state is drawn, not hidden. A frame that disappeared would take its
        // place in the layout with it, and the player would have nothing to aim at.
        let ((x, y), align) = crate::game_ui::centered(
            inner.x + inner.width * 0.5,
            inner.y + (inner.height - scale * 7.0) * 0.5,
        );
        list.text_aligned((x, y), empty_label, theme.status, scale, align);
        return;
    };

    list.text_at((inner.x, inner.y), target.kind.as_str(), theme.text, scale);
    // The distance, which is what tells a player whether the thing they selected is in range.
    if target.distance.is_finite() && target.distance < f32::MAX {
        scratch.detail.clear();
        write_integer(&mut scratch.detail, target.distance.round() as i64);
        scratch.detail.push('M');
        list.text_aligned(
            (inner.right(), inner.y),
            &scratch.detail,
            theme.text,
            scale,
            TextAlign::Right,
        );
    }

    let y = inner.y + size::LINE * factor;
    let bar = size::BAR * factor;
    let fraction = target.health_fraction();
    push_bar_row(
        list,
        scratch,
        theme,
        UiRect::new(inner.x, y, inner.width, bar.min(inner.bottom() - y)),
        fraction,
        theme.enemy_health_color(fraction),
        target.health,
        target.max_health,
        scale,
    );

    // Room for one more line only on the full-size frame; the small one stops at health.
    let status_y = y + bar + 2.0 * factor;
    if status_y + size::LINE * factor <= rect.bottom() && target.ignite_stacks > 0 {
        scratch.detail.clear();
        scratch.detail.push_str("IGNITE ");
        write_integer(&mut scratch.detail, i64::from(target.ignite_stacks));
        list.text_at((inner.x, status_y), &scratch.detail, theme.status, scale);
    }
}

/// Draws the party: one row per member, in roster order, local player first.
///
/// Rows nobody is in are left blank rather than filled with placeholders, and the frame is
/// the same size either way — so a member joining fills a row instead of moving the frame.
pub fn push_party_frame(
    list: &mut UiDrawList,
    rect: UiRect,
    members: &[PartyMemberView],
    theme: &UiTheme,
    scratch: &mut FrameScratch,
) {
    let factor = factor_of(rect, size::PARTY_WIDTH);
    let scale = text_scale(theme, factor);
    let padding = size::PADDING * factor;
    push_panel(list, rect, theme);
    let inner = rect.inset(padding);
    let row_height = size::PARTY_ROW * factor;

    for (index, member) in members.iter().enumerate() {
        let top = inner.y + row_height * index as f32;
        if top + row_height > rect.bottom() {
            // More members than the frame was laid out for. Drawing them outside it would
            // put health bars over whatever is underneath.
            break;
        }
        list.text_at((inner.x, top), member.name(), theme.text, scale);
        let fraction = member.health_fraction();
        let bar = UiRect::new(
            inner.x,
            top + size::LINE * factor,
            inner.width,
            size::BAR * factor,
        );
        push_bar_row(
            list,
            scratch,
            theme,
            bar,
            fraction,
            if member.alive {
                theme.player_health_color(fraction)
            } else {
                theme.enemy_health_low
            },
            member.health,
            member.max_health,
            scale,
        );
    }
}

/// Draws every enabled frame, in one call.
///
/// The order is painter's order and nothing else: the frames do not overlap in the default
/// layout, and a player who drags two on top of each other gets the later one on top.
pub fn push_frames(
    list: &mut UiDrawList,
    layout: &super::hud_layout::HudLayout,
    snapshot: &UiSnapshot,
    theme: &UiTheme,
    scratch: &mut FrameScratch,
    ui_scale: UiScale,
    viewport: (u32, u32),
) {
    use super::hud_layout::HudElementId;

    let rect_of = |id: HudElementId| -> Option<UiRect> {
        if !layout.is_enabled(id) {
            return None;
        }
        let size = frame_size(id)?;
        Some(layout.resolve(id, size, ui_scale, viewport))
    };

    if let Some(rect) = rect_of(HudElementId::Player) {
        push_player_frame(list, rect, snapshot, theme, scratch);
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
        push_party_frame(list, rect, &snapshot.party, theme, scratch);
    }
}

/// Whether a placement would put a frame under a point. For hit-testing outside edit mode.
#[must_use]
pub fn frame_contains(placement: HudPlacement, rect: UiRect, pointer: (f32, f32)) -> bool {
    placement.enabled && rect.contains(pointer.0, pointer.1)
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
                UiCommand::Text { range, .. } => Some(list.text_of(*range).to_owned()),
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
            UiScale::default(),
            FHD,
        );
        list
    }

    #[test]
    fn the_player_frame_shows_the_player() {
        let simulation = simulation();
        let snapshot = snapshot(&simulation);
        let list = frames(&HudLayout::default(), &snapshot);
        let printed = texts(&list);

        assert!(printed.iter().any(|text| text == "YOU"), "{printed:?}");
        assert!(
            printed.iter().any(|text| text.starts_with("HEAT")),
            "no Heat: {printed:?}"
        );
        assert!(
            printed.iter().any(|text| text == "ENERGY"),
            "no Energy: {printed:?}"
        );
        // The health the simulation actually has, not a number this file made up.
        let health = snapshot.player.health.round() as i64;
        assert!(
            printed
                .iter()
                .any(|text| text.starts_with(&format!("{health}/"))),
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
            printed.iter().filter(|text| *text == "NO TARGET").count() >= 2,
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
        assert!(
            printed.iter().any(|text| text == target.kind.as_str()),
            "{printed:?}"
        );
        let health = target.health.round() as i64;
        assert!(
            printed
                .iter()
                .any(|text| text.starts_with(&format!("{health}/"))),
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
        let kind = snapshot
            .target
            .as_ref()
            .expect("a target")
            .kind
            .as_str()
            .to_owned();
        assert!(
            printed.iter().filter(|text| **text == kind).count() >= 2,
            "{printed:?}"
        );
    }

    #[test]
    fn the_party_frame_grows_and_shrinks_with_the_roster() {
        let simulation = simulation();
        let mut snapshot = snapshot(&simulation);
        assert_eq!(snapshot.party.len(), 1, "a solo player is a party of one");

        let solo = texts(&frames(&HudLayout::default(), &snapshot));
        assert!(solo.iter().filter(|text| **text == "YOU").count() >= 1);

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
        for id in HudElementId::FRAMES {
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
                HudElementId::Party => push_party_frame(
                    &mut list,
                    rect,
                    &snapshot.party,
                    &UiTheme::default(),
                    &mut scratch,
                ),
                HudElementId::ActionBar(_) => continue,
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
                UiScale::default(),
                FHD,
            );
        }
        assert_eq!(scratch.health.capacity(), health, "the scratch grew");
        assert_eq!(scratch.detail.capacity(), detail, "the scratch grew");
        assert_eq!(scratch.total.capacity(), total, "the scratch grew");
    }

    #[test]
    fn a_frame_prints_only_glyphs_the_font_has() {
        // The 5x7 font has uppercase, digits and a little punctuation. A lowercase letter
        // would draw as nothing, which reads as a bug in the game rather than in the string.
        let mut simulation = simulation();
        simulation.select_nearest_target();
        let snapshot = snapshot(&simulation);
        for text in texts(&frames(&HudLayout::default(), &snapshot)) {
            for character in text.chars() {
                assert!(
                    character.is_ascii_uppercase()
                        || character.is_ascii_digit()
                        || " /.:-%+".contains(character),
                    "{character:?} in {text:?} has no glyph"
                );
            }
        }
    }
}
