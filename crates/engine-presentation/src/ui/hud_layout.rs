//! Where every movable interface element sits, at any resolution.
//!
//! ```text
//! HudConfig (anchor + normalized offset + scale)
//!      |
//!      v
//! HudLayout::resolve(id, size, viewport)  ->  UiRect in physical pixels
//! ```
//!
//! One abstraction for every element the player can arrange: eight action bars and four unit
//! frames. They differ in what they draw and in nothing else — dragging, scaling, toggling,
//! clamping and persisting are written once here rather than once per element, which is what
//! stops the target frame and the party frame from disagreeing about what "moved" means.
//!
//! ## Nothing is stored in pixels
//!
//! A position is an [`HudAnchor`] plus an offset in fractions of the viewport. Resolving it
//! is arithmetic against the current viewport, so the same configuration puts an element in
//! the same relative place at 1280x720 and at 3440x1440. A pixel pair could not: it would
//! mean "just above the bottom edge" on one display and "halfway up the screen" on another.
//!
//! The anchor is what makes an aspect-ratio change behave. A bar centred at the bottom is
//! anchored `Bottom` with a zero horizontal offset, so it is centred at the bottom of an
//! ultrawide too. Stored purely as a fraction it would sit at 0.5 of a much wider screen —
//! numerically the same, visually adrift.
//!
//! ## Size is not position
//!
//! Positions scale with the screen; sizes do not. An element's size is its design size in
//! logical pixels, multiplied by the player's global [`UiScale`] and then by that element's
//! own scale. This is the rule the HUD already follows, and it is why a health bar is the
//! same physical size on a 27-inch 1440p display as on a 24-inch 1080p one.
//!
//! ## Nothing can be lost off the edge
//!
//! [`HudLayout::resolve`] clamps, and it clamps on *resolve* rather than only on drag. A
//! layout arranged at 3440x1440 and opened at 1280x720 therefore still produces reachable
//! rectangles, so a resolution change can never strand an element where the player cannot
//! click it back.
//!
//! ## Room a tool panel has taken
//!
//! A development screen docked to an edge — the monster playground's two panels — reserves a
//! strip of the screen with [`HudLayout::set_reserved`]. The HUD is then laid out in what is
//! left, exactly as if the window were that much narrower: a left-anchored frame moves in
//! beside the panel, a centred bar stays centred in the space the player can see. Nothing is
//! written to the configuration, so the arrangement on disk never learns a panel existed.

use engine_render_data::ui::{UiRect, UiScale};
use engine_settings::hud::{
    ACTION_BARS, HUD_SCALE_MAX, HUD_SCALE_MIN, HUD_SCALE_STEP, HudAnchor, HudConfig,
    HudOrientation, HudPlacement,
};

/// How much of an element must stay inside the viewport, per axis.
///
/// Half. Enough that a mis-drag against an edge leaves an unmistakable target to drag back,
/// and loose enough that a wide bar can still be pushed most of the way into a corner.
pub const MIN_VISIBLE_FRACTION: f32 = 0.5;

/// Which arrangeable element.
///
/// One identifier for the whole HUD, so the editor, the configuration and the painter all
/// name the same things. Action bars carry their index rather than having eight variants:
/// everything that touches a bar does so in a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HudElementId {
    /// Action bar `0..ACTION_BARS`, numbered from zero and displayed from one.
    ActionBar(usize),
    /// The player's own frame.
    Player,
    /// The selected target.
    Target,
    /// The target's target.
    TargetOfTarget,
    /// The party.
    Party,
    /// The raid frames: a grid of filled cells, one per member.
    Raid,
    /// The player's own cast bar. The one element whose width and height are set separately.
    CastBar,
    /// The player's buffs.
    Buffs,
}

impl HudElementId {
    /// The four unit frames, in the order the editor lists them.
    pub const FRAMES: [Self; 4] = [
        Self::Player,
        Self::Target,
        Self::TargetOfTarget,
        Self::Party,
    ];

    /// The elements that are neither a bar nor one of the four unit frames.
    pub const EXTRAS: [Self; 3] = [Self::Raid, Self::CastBar, Self::Buffs];

    /// Every element, bars first.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..ACTION_BARS)
            .map(Self::ActionBar)
            .chain(Self::FRAMES)
            .chain(Self::EXTRAS)
    }

    /// Whether this element's width and height can be set on their own.
    #[must_use]
    pub fn is_resizable(self) -> bool {
        self == Self::CastBar
    }

    /// Whether this element is an action bar, and which one.
    #[must_use]
    pub fn bar_index(self) -> Option<usize> {
        match self {
            Self::ActionBar(index) => Some(index),
            _ => None,
        }
    }

    /// The name the editor prints over this element.
    ///
    /// Returned as an owned string only for the bars, which need their number; the callers
    /// that draw a name write into a reused scratch buffer rather than allocating per frame.
    pub fn write_label(self, out: &mut String) {
        use std::fmt::Write as _;
        out.clear();
        match self {
            Self::ActionBar(index) => {
                let _ = write!(out, "ACTION BAR {}", index + 1);
            }
            Self::Player => out.push_str("PLAYER"),
            Self::Target => out.push_str("TARGET"),
            Self::TargetOfTarget => out.push_str("TARGET OF TARGET"),
            Self::Party => out.push_str("PARTY"),
            Self::Raid => out.push_str("RAID"),
            Self::CastBar => out.push_str("CAST BAR"),
            Self::Buffs => out.push_str("BUFFS"),
        }
    }
}

/// The arrangement of every element, and the arithmetic that turns it into rectangles.
///
/// Owns a [`HudConfig`] rather than borrowing one: the editor mutates the layout every frame
/// of a drag, and the file is written from it on release. The manager that persists it holds
/// its own copy, and the two meet at [`HudLayout::config`].
#[derive(Debug, Clone, PartialEq)]
pub struct HudLayout {
    config: HudConfig,
    /// Physical pixels a docked panel has taken from the left and the right edge.
    reserved: (f32, f32),
}

impl Default for HudLayout {
    fn default() -> Self {
        Self::new(HudConfig::default())
    }
}

impl HudLayout {
    /// A layout over `config`, validated on the way in.
    ///
    /// Validating here as well as on load means a layout built in a test or handed over by a
    /// future network sync cannot contain a nonsense offset either.
    #[must_use]
    pub fn new(config: HudConfig) -> Self {
        Self {
            config: config.validated(),
            reserved: (0.0, 0.0),
        }
    }

    /// Takes `left` and `right` physical pixels off the screen's sides for everything the
    /// layout places, until it is called again. Zero gives them back.
    ///
    /// Not part of the configuration: see the module notes.
    pub fn set_reserved(&mut self, left: f32, right: f32) {
        let clean = |value: f32| {
            if value.is_finite() {
                value.max(0.0)
            } else {
                0.0
            }
        };
        self.reserved = (clean(left), clean(right));
    }

    /// What [`HudLayout::set_reserved`] took, left and right.
    #[must_use]
    pub fn reserved(&self) -> (f32, f32) {
        self.reserved
    }

    /// The part of `viewport` the HUD is laid out in: left edge, width and height.
    fn region(&self, viewport: (u32, u32)) -> Region {
        let (width, height) = viewport_size(viewport);
        // The reservation never takes more than three quarters of the screen: a HUD squeezed
        // into nothing would put every element on top of every other.
        let available = width * 0.25;
        let taken = self.reserved.0 + self.reserved.1;
        let fit = if taken > width - available && taken > 0.0 {
            (width - available) / taken
        } else {
            1.0
        };
        let left = self.reserved.0 * fit;
        Region {
            x: left,
            width: (width - taken * fit).max(1.0),
            height,
        }
    }

    /// The whole arrangement, for writing to disk.
    #[must_use]
    pub fn config(&self) -> &HudConfig {
        &self.config
    }

    /// Replaces the whole arrangement. What loading a file does.
    pub fn set_config(&mut self, config: HudConfig) {
        self.config = config.validated();
    }

    /// Where `id` is placed, or its default if the element does not exist.
    ///
    /// A bar index past the end cannot happen after validation — there are always
    /// [`ACTION_BARS`] bars — so the fallback is unreachable rather than meaningful. It is
    /// there so that a caller iterating an index range never has to unwrap.
    #[must_use]
    pub fn placement(&self, id: HudElementId) -> HudPlacement {
        match id {
            HudElementId::ActionBar(index) => self
                .config
                .bar(index)
                .map_or_else(HudPlacement::default, |bar| bar.placement),
            HudElementId::Player => self.config.frames.player,
            HudElementId::Target => self.config.frames.target,
            HudElementId::TargetOfTarget => self.config.frames.target_of_target,
            HudElementId::Party => self.config.frames.party,
            HudElementId::Raid => self.config.frames.raid,
            HudElementId::CastBar => self.config.frames.cast_bar.placement,
            HudElementId::Buffs => self.config.frames.buffs,
        }
    }

    /// Replaces where `id` is placed.
    pub fn set_placement(&mut self, id: HudElementId, placement: HudPlacement) {
        let slot = match id {
            HudElementId::ActionBar(index) => match self.config.bars.get_mut(index) {
                Some(bar) => &mut bar.placement,
                None => return,
            },
            HudElementId::Player => &mut self.config.frames.player,
            HudElementId::Target => &mut self.config.frames.target,
            HudElementId::TargetOfTarget => &mut self.config.frames.target_of_target,
            HudElementId::Party => &mut self.config.frames.party,
            HudElementId::Raid => &mut self.config.frames.raid,
            HudElementId::Buffs => &mut self.config.frames.buffs,
            HudElementId::CastBar => &mut self.config.frames.cast_bar.placement,
        };
        *slot = placement;
    }

    /// The cast bar's width and height, in logical pixels at the design size.
    #[must_use]
    pub fn cast_bar_size(&self) -> (f32, f32) {
        let bar = self.config.frames.cast_bar;
        (bar.width, bar.height)
    }

    /// Sets the cast bar's width and height, clamped and snapped to the allowed steps.
    ///
    /// Keeps the bar's centre where it was: a bar that grew from its top-left corner would
    /// walk away from the spot the player put it in every time they made it wider.
    pub fn resize_cast_bar(
        &mut self,
        width: f32,
        height: f32,
        ui_scale: UiScale,
        viewport: (u32, u32),
    ) {
        let before = self.rect_of(HudElementId::CastBar, ui_scale, viewport);
        let centre = (
            before.x + before.width * 0.5,
            before.y + before.height * 0.5,
        );
        self.config.frames.cast_bar = self.config.frames.cast_bar.resized(width, height);
        let size = self.cast_bar_size();
        let factor = ui_scale.factor() * self.scale_of(HudElementId::CastBar);
        let (w, h) = (size.0 * factor, size.1 * factor);
        self.move_to(
            HudElementId::CastBar,
            (centre.0 - w * 0.5, centre.1 - h * 0.5),
            size,
            ui_scale,
            viewport,
        );
    }

    /// Makes the cast bar wider or narrower, taller or shorter, by whole steps.
    pub fn nudge_cast_bar(
        &mut self,
        width_steps: i32,
        height_steps: i32,
        ui_scale: UiScale,
        viewport: (u32, u32),
    ) {
        use engine_settings::hud::{CAST_BAR_HEIGHT_STEP, CAST_BAR_WIDTH_STEP};
        let (width, height) = self.cast_bar_size();
        self.resize_cast_bar(
            width + CAST_BAR_WIDTH_STEP * width_steps as f32,
            height + CAST_BAR_HEIGHT_STEP * height_steps as f32,
            ui_scale,
            viewport,
        );
    }

    /// Whether `id` is drawn and interactive at all.
    #[must_use]
    pub fn is_enabled(&self, id: HudElementId) -> bool {
        self.placement(id).enabled
    }

    /// Shows or hides `id`.
    pub fn set_enabled(&mut self, id: HudElementId, enabled: bool) {
        let mut placement = self.placement(id);
        placement.enabled = enabled;
        self.set_placement(id, placement);
    }

    /// Shows `id` if it was hidden and hides it if it was shown. Returns the new state.
    pub fn toggle(&mut self, id: HudElementId) -> bool {
        let enabled = !self.is_enabled(id);
        self.set_enabled(id, enabled);
        enabled
    }

    /// How big `id` is drawn, as a multiple of its design size.
    #[must_use]
    pub fn scale_of(&self, id: HudElementId) -> f32 {
        self.placement(id).scale
    }

    /// Sets how big `id` is drawn, clamped to the bounds the configuration allows.
    ///
    /// Clamped rather than refused, unlike a value read from a file: this comes from a
    /// control the player is pressing, and the honest answer to "smaller" at the smallest
    /// size is to stay there rather than to jump back to the default.
    pub fn set_scale(&mut self, id: HudElementId, scale: f32) {
        let mut placement = self.placement(id);
        placement.scale = if scale.is_finite() {
            scale.clamp(HUD_SCALE_MIN, HUD_SCALE_MAX)
        } else {
            placement.scale
        };
        self.set_placement(id, placement);
    }

    /// Grows or shrinks `id` by one step of the size control.
    pub fn nudge_scale(&mut self, id: HudElementId, steps: i32) {
        let scale = self.scale_of(id) + HUD_SCALE_STEP * steps as f32;
        // Rounded to the step, so pressing bigger then smaller returns to exactly the size
        // it started at rather than to a value a hair away from it.
        let rounded = (scale / HUD_SCALE_STEP).round() * HUD_SCALE_STEP;
        self.set_scale(id, rounded);
    }

    /// Which way action bar `index` runs.
    #[must_use]
    pub fn orientation(&self, index: usize) -> HudOrientation {
        self.config
            .bar(index)
            .map_or(HudOrientation::Horizontal, |bar| bar.orientation)
    }

    /// Turns action bar `index` the other way. Returns its new orientation.
    ///
    /// The slots are not touched. Orientation is a layout fact, and a bar turned on its side
    /// is the same bar holding the same spells on the same keys.
    pub fn flip_orientation(&mut self, index: usize) -> HudOrientation {
        let Some(bar) = self.config.bars.get_mut(index) else {
            return HudOrientation::Horizontal;
        };
        bar.orientation = bar.orientation.flipped();
        bar.orientation
    }

    /// How big `id` is at its design size, in logical pixels.
    ///
    /// The one table the painter, the editor and the hit-test all read, so an outline drawn
    /// around an element and the element itself can never be different sizes. A bar's size
    /// depends on which way it runs, which is why this is a method rather than a constant.
    #[must_use]
    pub fn element_size(&self, id: HudElementId) -> (f32, f32) {
        match id {
            HudElementId::ActionBar(index) => super::action_bar::bar_size(self.orientation(index)),
            HudElementId::CastBar => self.cast_bar_size(),
            HudElementId::Raid => super::raid_frame::raid_size(),
            HudElementId::Buffs => super::aura_row::buff_bar_size(),
            frame => super::unit_frame::frame_size(frame).unwrap_or((0.0, 0.0)),
        }
    }

    /// The rectangle `id` occupies right now, at its own size.
    ///
    /// [`HudLayout::resolve`] with the size filled in. What every caller that is not testing
    /// an unusual size actually wants.
    #[must_use]
    pub fn rect_of(&self, id: HudElementId, ui_scale: UiScale, viewport: (u32, u32)) -> UiRect {
        self.resolve(id, self.element_size(id), ui_scale, viewport)
    }

    /// The rectangle `id` occupies, in physical pixels, clamped on screen.
    ///
    /// `size` is the element's own design size in logical pixels — what it would measure at
    /// scale one — and `ui_scale` is the player's global interface size. The element's own
    /// scale is applied on top of both.
    #[must_use]
    pub fn resolve(
        &self,
        id: HudElementId,
        size: (f32, f32),
        ui_scale: UiScale,
        viewport: (u32, u32),
    ) -> UiRect {
        let placement = self.placement(id);
        let factor = ui_scale.factor() * placement.scale;
        let (width, height) = (size.0 * factor, size.1 * factor);
        let region = self.region(viewport);
        clamp_in(unclamped_rect(placement, (width, height), region), region)
    }

    /// Moves `id` so that its top-left corner lands on `(x, y)` in physical pixels.
    ///
    /// The inverse of [`HudLayout::resolve`], and how a drag writes its result: the pointer
    /// produces a pixel position, this turns it back into the anchor-and-fraction form that
    /// survives a resolution change. The position is clamped first, so a drag cannot store
    /// an offset that would put the element out of reach.
    ///
    /// The anchor is re-chosen from where the element's *centre* ended up, so an element
    /// dragged to the bottom of the screen becomes bottom-anchored and stays at the bottom
    /// on a display of a different shape. Without that, everything would keep whichever
    /// anchor it happened to start with and only the fractions would move.
    pub fn move_to(
        &mut self,
        id: HudElementId,
        position: (f32, f32),
        size: (f32, f32),
        ui_scale: UiScale,
        viewport: (u32, u32),
    ) {
        let mut placement = self.placement(id);
        let factor = ui_scale.factor() * placement.scale;
        let (width, height) = (size.0 * factor, size.1 * factor);
        let region = self.region(viewport);
        let rect = clamp_in(UiRect::new(position.0, position.1, width, height), region);

        let centre = (
            (rect.x - region.x + width * 0.5) / region.width,
            (rect.y + height * 0.5) / region.height,
        );
        placement.anchor = HudAnchor::nearest(centre.0, centre.1);

        let (origin_x, origin_y) = placement.anchor.origin();
        let (align_x, align_y) = placement.anchor.alignment();
        placement.offset_x = (rect.x - region.x + width * align_x) / region.width - origin_x;
        placement.offset_y = (rect.y + height * align_y) / region.height - origin_y;
        self.set_placement(id, placement);
    }
}

/// The viewport as a pair of floats, never zero.
///
/// A zero-sized client area happens for real — a minimized window reports one — and dividing
/// by it would fill every offset in the layout with `NaN`, which would then be written to
/// the file and fail validation on the next launch.
fn viewport_size(viewport: (u32, u32)) -> (f32, f32) {
    ((viewport.0 as f32).max(1.0), (viewport.1 as f32).max(1.0))
}

/// The part of the screen the HUD is laid out in, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Region {
    /// Its left edge.
    x: f32,
    /// Its width, never zero.
    width: f32,
    /// Its height, never zero. It always starts at the top.
    height: f32,
}

/// Where a placement puts an element of a given physical size, before clamping.
fn unclamped_rect(placement: HudPlacement, size: (f32, f32), region: Region) -> UiRect {
    let (origin_x, origin_y) = placement.anchor.origin();
    let (align_x, align_y) = placement.anchor.alignment();
    let x = region.x + (origin_x + placement.offset_x) * region.width - size.0 * align_x;
    let y = (origin_y + placement.offset_y) * region.height - size.1 * align_y;
    UiRect::new(x, y, size.0, size.1)
}

/// The same rectangle, moved the least distance that keeps enough of it on screen.
///
/// An element wider than the viewport is centred rather than clamped: there is no position
/// that satisfies both edges, and pinning it to one of them would hide the other end with no
/// way to see it.
#[must_use]
pub fn clamp_on_screen(rect: UiRect, viewport: (u32, u32)) -> UiRect {
    let (width, height) = viewport_size(viewport);
    clamp_in(
        rect,
        Region {
            x: 0.0,
            width,
            height,
        },
    )
}

/// [`clamp_on_screen`] inside `region` rather than the whole screen.
fn clamp_in(rect: UiRect, region: Region) -> UiRect {
    let clamp_axis = |position: f32, size: f32, extent: f32| -> f32 {
        let visible = size * MIN_VISIBLE_FRACTION;
        if visible > extent {
            // More of the element has to stay on screen than the screen can show. No
            // position satisfies that, so it is centred: pinning it to one edge would push
            // the other end off with no way to bring it back.
            return (extent - size) * 0.5;
        }
        position.clamp(visible - size, extent - visible)
    };
    UiRect {
        x: region.x + clamp_axis(rect.x - region.x, rect.width, region.width),
        y: clamp_axis(rect.y, rect.height, region.height),
        ..rect
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_settings::hud::ACTION_BAR_SLOTS;

    const HD: (u32, u32) = (1280, 720);
    const FHD: (u32, u32) = (1920, 1080);
    const QHD: (u32, u32) = (2560, 1440);
    const ULTRAWIDE: (u32, u32) = (3440, 1440);

    /// A middling element size, in logical pixels.
    const SIZE: (f32, f32) = (320.0, 60.0);

    fn layout() -> HudLayout {
        HudLayout::default()
    }

    /// Where a rectangle's centre sits, as a fraction of the viewport.
    fn centre_fraction(rect: UiRect, viewport: (u32, u32)) -> (f32, f32) {
        (
            (rect.x + rect.width * 0.5) / viewport.0 as f32,
            (rect.y + rect.height * 0.5) / viewport.1 as f32,
        )
    }

    #[test]
    fn every_element_has_a_name_and_the_bars_are_numbered_from_one() {
        let mut label = String::new();
        HudElementId::ActionBar(0).write_label(&mut label);
        assert_eq!(label, "ACTION BAR 1");
        HudElementId::ActionBar(7).write_label(&mut label);
        assert_eq!(label, "ACTION BAR 8");

        for id in HudElementId::all() {
            id.write_label(&mut label);
            assert!(!label.is_empty(), "{id:?} has no name");
        }
        assert_eq!(HudElementId::all().count(), ACTION_BARS + 7);
    }

    #[test]
    fn the_slot_count_agrees_with_the_gameplay_bar() {
        // The configuration crate cannot name this constant; this is the assertion that
        // stops the two definitions from drifting apart.
        assert_eq!(ACTION_BAR_SLOTS, engine_game::firemage::ABILITY_SLOTS);
    }

    #[test]
    fn an_anchored_element_lands_where_the_anchor_says() {
        let mut layout = layout();
        for (anchor, expected) in [
            (HudAnchor::TopLeft, (0.0, 0.0)),
            (HudAnchor::Top, (0.5, 0.0)),
            (HudAnchor::TopRight, (1.0, 0.0)),
            (HudAnchor::Left, (0.0, 0.5)),
            (HudAnchor::Center, (0.5, 0.5)),
            (HudAnchor::Right, (1.0, 0.5)),
            (HudAnchor::BottomLeft, (0.0, 1.0)),
            (HudAnchor::Bottom, (0.5, 1.0)),
            (HudAnchor::BottomRight, (1.0, 1.0)),
        ] {
            layout.set_placement(HudElementId::Player, HudPlacement::at(anchor, 0.0, 0.0));
            let rect = layout.resolve(HudElementId::Player, SIZE, UiScale::default(), FHD);
            // The element's matching corner is at the anchor, so an element in the top-left
            // starts at the top-left and one in the bottom-right ends at the bottom-right.
            let corner = (
                rect.x + rect.width * anchor.alignment().0,
                rect.y + rect.height * anchor.alignment().1,
            );
            let want = (expected.0 * FHD.0 as f32, expected.1 * FHD.1 as f32);
            assert!(
                (corner.0 - want.0).abs() < 0.01 && (corner.1 - want.1).abs() < 0.01,
                "{anchor:?} put its corner at {corner:?}, not {want:?}"
            );
        }
    }

    #[test]
    fn a_normalized_position_means_the_same_place_at_every_resolution() {
        let mut layout = layout();
        layout.set_placement(
            HudElementId::Target,
            HudPlacement::at(HudAnchor::Center, 0.10, -0.20),
        );

        let reference = centre_fraction(
            layout.resolve(HudElementId::Target, SIZE, UiScale::default(), FHD),
            FHD,
        );
        for viewport in [HD, QHD, ULTRAWIDE, (1024, 768)] {
            let rect = layout.resolve(HudElementId::Target, SIZE, UiScale::default(), viewport);
            let centre = centre_fraction(rect, viewport);
            // Not exactly equal: the element keeps its pixel size, so its centre shifts by
            // half the difference in how much of the screen it covers. A hundredth of the
            // screen is well inside "the same relative place".
            assert!(
                (centre.0 - reference.0).abs() < 0.01 && (centre.1 - reference.1).abs() < 0.01,
                "{viewport:?} put it at {centre:?}, not {reference:?}"
            );
        }
    }

    #[test]
    fn a_bar_centred_at_the_bottom_stays_centred_at_the_bottom_on_an_ultrawide() {
        // The reason the anchor exists. Stored as a bare fraction this would drift; anchored
        // `Bottom`, it does not.
        let mut layout = layout();
        layout.set_placement(
            HudElementId::ActionBar(0),
            HudPlacement::at(HudAnchor::Bottom, 0.0, -0.03),
        );
        for viewport in [HD, FHD, QHD, ULTRAWIDE, (1280, 1024)] {
            let rect = layout.resolve(
                HudElementId::ActionBar(0),
                SIZE,
                UiScale::default(),
                viewport,
            );
            let centre_x = rect.x + rect.width * 0.5;
            assert!(
                (centre_x - viewport.0 as f32 * 0.5).abs() < 0.01,
                "{viewport:?} is not centred: {centre_x}"
            );
            assert!(rect.bottom() < viewport.1 as f32, "it left the screen");
            assert!(
                rect.bottom() > viewport.1 as f32 * 0.9,
                "it is not near the bottom"
            );
        }
    }

    #[test]
    fn an_element_keeps_its_size_when_the_screen_changes_and_grows_with_the_scales() {
        let layout = layout();
        let at = |ui_scale: f32, viewport| {
            layout.resolve(HudElementId::Player, SIZE, UiScale::new(ui_scale), viewport)
        };
        // Position follows the screen; size does not.
        assert_eq!(at(1.0, HD).width, at(1.0, QHD).width);
        assert_eq!(at(1.0, HD).height, at(1.0, QHD).height);
        // Both scales multiply.
        assert!((at(2.0, FHD).width - SIZE.0 * 2.0).abs() < 0.01);

        let mut scaled = layout.clone();
        scaled.set_scale(HudElementId::Player, 1.5);
        let rect = scaled.resolve(HudElementId::Player, SIZE, UiScale::new(2.0), FHD);
        assert!((rect.width - SIZE.0 * 3.0).abs() < 0.01, "{rect:?}");
    }

    #[test]
    fn every_element_can_be_moved_and_the_move_survives_a_resolution_change() {
        let mut layout = layout();
        for id in HudElementId::all() {
            let before = layout.resolve(id, SIZE, UiScale::default(), FHD);
            layout.move_to(id, (700.0, 400.0), SIZE, UiScale::default(), FHD);
            let after = layout.resolve(id, SIZE, UiScale::default(), FHD);

            assert_ne!(before, after, "{id:?} did not move");
            assert!(
                (after.x - 700.0).abs() < 0.01 && (after.y - 400.0).abs() < 0.01,
                "{id:?} landed at {after:?}"
            );

            // The same relative place at a different resolution.
            let moved = centre_fraction(after, FHD);
            let elsewhere = centre_fraction(layout.resolve(id, SIZE, UiScale::default(), QHD), QHD);
            assert!(
                (moved.0 - elsewhere.0).abs() < 0.01 && (moved.1 - elsewhere.1).abs() < 0.01,
                "{id:?} drifted to {elsewhere:?} from {moved:?}"
            );
        }
    }

    #[test]
    fn a_drag_re_anchors_to_the_part_of_the_screen_it_finished_in() {
        let mut layout = layout();
        layout.move_to(
            HudElementId::Party,
            (20.0, 20.0),
            SIZE,
            UiScale::default(),
            FHD,
        );
        assert_eq!(
            layout.placement(HudElementId::Party).anchor,
            HudAnchor::TopLeft
        );

        layout.move_to(
            HudElementId::Party,
            (1500.0, 1000.0),
            SIZE,
            UiScale::default(),
            FHD,
        );
        assert_eq!(
            layout.placement(HudElementId::Party).anchor,
            HudAnchor::BottomRight
        );
    }

    #[test]
    fn nothing_can_be_dragged_off_the_screen() {
        let mut layout = layout();
        for target in [
            (-5000.0, -5000.0),
            (5000.0, 5000.0),
            (-5000.0, 5000.0),
            (5000.0, -5000.0),
        ] {
            layout.move_to(HudElementId::Target, target, SIZE, UiScale::default(), FHD);
            let rect = layout.resolve(HudElementId::Target, SIZE, UiScale::default(), FHD);
            assert_visible(rect, FHD);
        }
    }

    #[test]
    fn a_layout_arranged_on_a_large_screen_is_still_reachable_on_a_small_one() {
        // The clamp runs on resolve, not only on drag, which is what makes this true.
        let mut layout = layout();
        layout.move_to(
            HudElementId::Player,
            (3300.0, 1400.0),
            SIZE,
            UiScale::default(),
            ULTRAWIDE,
        );
        for viewport in [HD, FHD, (800, 600), (640, 480)] {
            let rect = layout.resolve(HudElementId::Player, SIZE, UiScale::default(), viewport);
            assert_visible(rect, viewport);
        }
    }

    #[test]
    fn an_element_too_large_for_the_screen_is_centred_rather_than_pinned() {
        let layout = layout();
        let huge = (4000.0, 2000.0);
        let rect = layout.resolve(HudElementId::Player, huge, UiScale::default(), HD);
        assert!((rect.x + rect.width * 0.5 - 640.0).abs() < 0.01, "{rect:?}");
        assert!(
            (rect.y + rect.height * 0.5 - 360.0).abs() < 0.01,
            "{rect:?}"
        );
    }

    #[test]
    fn a_minimized_window_produces_numbers_rather_than_nan() {
        let mut layout = layout();
        let rect = layout.resolve(HudElementId::Player, SIZE, UiScale::default(), (0, 0));
        assert!(rect.x.is_finite() && rect.y.is_finite(), "{rect:?}");

        layout.move_to(
            HudElementId::Player,
            (10.0, 10.0),
            SIZE,
            UiScale::default(),
            (0, 0),
        );
        let placement = layout.placement(HudElementId::Player);
        assert!(
            placement.offset_x.is_finite() && placement.offset_y.is_finite(),
            "{placement:?}"
        );
    }

    #[test]
    fn an_element_can_be_toggled_and_the_state_is_remembered() {
        let mut layout = layout();
        for id in HudElementId::all() {
            let before = layout.is_enabled(id);
            assert_eq!(layout.toggle(id), !before);
            assert_eq!(layout.is_enabled(id), !before);
            layout.set_enabled(id, before);
            assert_eq!(layout.is_enabled(id), before);
        }
        // A fresh layout has bar one on and the other seven off.
        assert!(layout.is_enabled(HudElementId::ActionBar(0)));
        for index in 1..ACTION_BARS {
            assert!(
                !layout.is_enabled(HudElementId::ActionBar(index)),
                "bar {index}"
            );
        }
    }

    #[test]
    fn scaling_steps_up_and_down_and_stops_at_the_bounds() {
        let mut layout = layout();
        let id = HudElementId::Target;
        let start = layout.scale_of(id);

        layout.nudge_scale(id, 1);
        assert!((layout.scale_of(id) - (start + HUD_SCALE_STEP)).abs() < 1e-5);
        layout.nudge_scale(id, -1);
        assert!(
            (layout.scale_of(id) - start).abs() < 1e-5,
            "up then down did not return to {start}"
        );

        for _ in 0..100 {
            layout.nudge_scale(id, -1);
        }
        assert!((layout.scale_of(id) - HUD_SCALE_MIN).abs() < 1e-5);
        for _ in 0..100 {
            layout.nudge_scale(id, 1);
        }
        assert!((layout.scale_of(id) - HUD_SCALE_MAX).abs() < 1e-5);

        // A nonsense scale is refused rather than stored.
        layout.set_scale(id, f32::NAN);
        assert!(layout.scale_of(id).is_finite());
    }

    #[test]
    fn a_bar_turned_on_its_side_keeps_everything_but_its_orientation() {
        let mut layout = layout();
        let before = layout.config().clone();
        assert_eq!(layout.orientation(2), HudOrientation::Horizontal);
        assert_eq!(layout.flip_orientation(2), HudOrientation::Vertical);
        assert_eq!(layout.orientation(2), HudOrientation::Vertical);

        let after = layout.config();
        assert_eq!(after.bars[2].slots, before.bars[2].slots, "slots changed");
        assert_eq!(
            after.bars[2].placement, before.bars[2].placement,
            "placement changed"
        );
        assert_eq!(layout.flip_orientation(2), HudOrientation::Horizontal);
        assert_eq!(layout.config(), &before, "flipping twice changed something");
    }

    #[test]
    fn a_layout_is_validated_on_the_way_in() {
        let mut config = HudConfig::default();
        config.frames.player.scale = 99.0;
        let layout = HudLayout::new(config);
        assert!(layout.config().is_valid());
        assert!((HUD_SCALE_MIN..=HUD_SCALE_MAX).contains(&layout.scale_of(HudElementId::Player)));
    }

    /// Asserts enough of `rect` is inside `viewport` to be clicked.
    fn assert_visible(rect: UiRect, viewport: (u32, u32)) {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let visible_x = (rect.right().min(width) - rect.x.max(0.0)).max(0.0);
        let visible_y = (rect.bottom().min(height) - rect.y.max(0.0)).max(0.0);
        assert!(
            visible_x >= rect.width * MIN_VISIBLE_FRACTION - 0.01,
            "only {visible_x} of {} is on a {viewport:?} screen: {rect:?}",
            rect.width
        );
        assert!(
            visible_y >= rect.height * MIN_VISIBLE_FRACTION - 0.01,
            "only {visible_y} of {} is on a {viewport:?} screen: {rect:?}",
            rect.height
        );
    }

    #[test]
    fn the_cast_bar_resizes_about_its_centre_within_its_bounds() {
        use engine_settings::hud::{CAST_BAR_HEIGHT_MAX, CAST_BAR_WIDTH_MIN};
        let mut layout = layout();
        let scale = UiScale::default();
        let before = layout.rect_of(HudElementId::CastBar, scale, FHD);
        layout.nudge_cast_bar(3, 2, scale, FHD);
        let after = layout.rect_of(HudElementId::CastBar, scale, FHD);
        assert!(after.width > before.width && after.height > before.height);
        let centre = |rect: UiRect| (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
        let (a, b) = (centre(before), centre(after));
        assert!(
            (a.0 - b.0).abs() <= 1.0 && (a.1 - b.1).abs() <= 1.0,
            "{a:?} -> {b:?}"
        );

        layout.resize_cast_bar(1.0, 1_000.0, scale, FHD);
        assert_eq!(
            layout.cast_bar_size(),
            (CAST_BAR_WIDTH_MIN, CAST_BAR_HEIGHT_MAX)
        );
        assert_eq!(
            layout.element_size(HudElementId::CastBar),
            layout.cast_bar_size()
        );
    }

    #[test]
    fn a_reserved_edge_moves_the_hud_into_what_is_left_and_gives_it_back() {
        let mut layout = layout();
        let scale = UiScale::default();
        let player = layout.rect_of(HudElementId::Player, scale, FHD);
        let bar = layout.rect_of(HudElementId::ActionBar(0), scale, FHD);
        let buffs = layout.rect_of(HudElementId::Buffs, scale, FHD);

        layout.set_reserved(330.0, 330.0);
        let moved = layout.rect_of(HudElementId::Player, scale, FHD);
        assert!(
            moved.x >= 330.0,
            "the player frame is still under the panel"
        );
        let centred = layout.rect_of(HudElementId::ActionBar(0), scale, FHD);
        assert!(
            (centred.x + centred.width * 0.5 - (bar.x + bar.width * 0.5)).abs() < 0.5,
            "equal panels keep a centred bar where it was"
        );
        let right = layout.rect_of(HudElementId::Buffs, scale, FHD);
        assert!(right.right() <= FHD.0 as f32 - 330.0 + 0.5);
        assert!(right.x < buffs.x);

        // A drag under a reservation stores a position that comes back to the same place.
        layout.move_to(HudElementId::Party, (500.0, 300.0), SIZE, scale, FHD);
        let dropped = layout.resolve(HudElementId::Party, SIZE, scale, FHD);
        assert!((dropped.x - 500.0).abs() < 0.5 && (dropped.y - 300.0).abs() < 0.5);

        layout.set_reserved(0.0, 0.0);
        assert_eq!(layout.rect_of(HudElementId::Player, scale, FHD), player);
        // An absurd reservation still leaves a quarter of the screen.
        layout.set_reserved(5_000.0, f32::NAN);
        let squeezed = layout.rect_of(HudElementId::Player, scale, HD);
        assert!(squeezed.x.is_finite() && squeezed.x < HD.0 as f32);
    }

    #[test]
    fn the_raid_frames_and_the_cast_bar_are_elements_like_any_other() {
        let mut label = String::new();
        HudElementId::Raid.write_label(&mut label);
        assert_eq!(label, "RAID");
        HudElementId::CastBar.write_label(&mut label);
        assert_eq!(label, "CAST BAR");
        let ids: Vec<_> = HudElementId::all().collect();
        assert!(ids.contains(&HudElementId::Raid) && ids.contains(&HudElementId::CastBar));
        assert!(HudElementId::CastBar.is_resizable());
        assert!(!HudElementId::Raid.is_resizable());
        let mut layout = layout();
        assert!(layout.is_enabled(HudElementId::Raid) && layout.is_enabled(HudElementId::CastBar));
        layout.toggle(HudElementId::CastBar);
        assert!(!layout.is_enabled(HudElementId::CastBar));
    }
}
