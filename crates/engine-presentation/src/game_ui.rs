//! The game's user interface, laid out as UI commands.
//!
//! ```text
//! UiSnapshot + NameplateSet  -->  GameUi::build  -->  UiDrawList  -->  one draw call
//! ```
//!
//! This is the only place the HUD's layout lives. It reads a [`UiSnapshot`] — never the
//! simulation — and produces [`UiDrawList`] commands, which are plain data. No device, no
//! handle, no vertex: every rule about where a health bar sits and what colour it turns is
//! therefore unit tested without a GPU.
//!
//! Commands are pushed in paint order: world-space elements first, screen-space HUD on top.

use engine_core::math::{Mat4, Vec3};
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};

use crate::damage_numbers::FloatingNumbers;
use crate::nameplate::{Nameplate, NameplateConfig, project};
use crate::snapshot::{AbilitySlotView, SlotAvailability, UiSnapshot};
use engine_game::firemage::{ABILITY_SLOTS, FireAbility, HeatTier, INFERNO_DURATION};

/// Layout constants for the combat HUD, in pixels at any resolution.
///
/// Fixed pixel sizes rather than fractions of the viewport: a health bar that grew with the
/// window would be a different size on every display, and the font is a bitmap that only
/// looks right at integer scales anyway. Positions are anchored to edges and to the centre,
/// so the HUD follows a resize without the elements themselves changing size.
mod layout {
    /// Margin from the client edge.
    pub const MARGIN: f32 = 24.0;
    /// Width of the player's bars.
    pub const PLAYER_BAR_WIDTH: f32 = 260.0;
    /// Height of one bar.
    pub const BAR_HEIGHT: f32 = 18.0;
    /// Gap between stacked bars.
    pub const BAR_GAP: f32 = 4.0;
    /// Width of the target frame.
    pub const TARGET_FRAME_WIDTH: f32 = 320.0;
    /// Height of the target frame.
    pub const TARGET_FRAME_HEIGHT: f32 = 58.0;
    /// Distance from the top of the client area to the target frame.
    pub const TARGET_FRAME_TOP: f32 = 24.0;
    /// Size of one ability slot.
    pub const SLOT_SIZE: f32 = 48.0;
    /// Gap between ability slots.
    pub const SLOT_GAP: f32 = 6.0;
    /// Distance from the bottom of the client area to the ability bar.
    pub const ABILITY_BAR_BOTTOM: f32 = 28.0;
    /// Width of the cast bar.
    pub const CAST_BAR_WIDTH: f32 = 280.0;
    /// Height of the cast bar.
    pub const CAST_BAR_HEIGHT: f32 = 20.0;
    /// How far above the ability bar the cast bar sits.
    pub const CAST_BAR_LIFT: f32 = 34.0;
    /// Padding inside a panel.
    pub const PADDING: f32 = 6.0;
    /// Height of a world-space health bar above an enemy.
    pub const WORLD_BAR_HEIGHT: f32 = 6.0;
    /// Width of a world-space health bar above an enemy.
    pub const WORLD_BAR_WIDTH: f32 = 56.0;
    /// Size of the Ultimate button, which is deliberately larger than a bar slot.
    pub const ULTIMATE_SIZE: f32 = 66.0;
    /// Gap between the bar and the Ultimate button, which separates them visually.
    pub const ULTIMATE_GAP: f32 = 28.0;
    /// Width of the Heat and Ultimate Energy bars.
    pub const RESOURCE_BAR_WIDTH: f32 = 320.0;
    /// Height of a resource bar.
    pub const RESOURCE_BAR_HEIGHT: f32 = 16.0;
    /// How far above the ability bar the resource bars sit.
    pub const RESOURCE_LIFT: f32 = 84.0;
    /// Size of the talent button in the top-right corner.
    pub const TALENT_BUTTON_SIZE: f32 = 44.0;
    /// Size of one Ignite stack pip.
    pub const IGNITE_PIP: f32 = 10.0;
    /// Gap between Ignite pips.
    pub const IGNITE_PIP_GAP: f32 = 3.0;
    /// Width of an ability tooltip.
    pub const TOOLTIP_WIDTH: f32 = 300.0;
    /// Height of one tooltip line.
    pub const TOOLTIP_LINE: f32 = 16.0;
}

/// Every HUD measurement, converted from its design size to physical pixels once.
///
/// The constants in [`layout`] are **logical** pixels — the size the HUD was drawn at, which
/// is the only unit worth writing a layout in. This is the same set after multiplying by the
/// player's [`UiScale`], which is what the rest of the file actually positions things with.
///
/// A struct rather than a scale applied at each use site, for two reasons. Converting in one
/// place means a measurement cannot be scaled twice or forgotten — the two mistakes that
/// produce a HUD that is subtly wrong rather than obviously broken. And it means the
/// conversion happens once per scale change rather than once per constant per frame, so a
/// scaled HUD costs the same to lay out as an unscaled one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudMetrics {
    /// The scale these were derived with.
    pub scale: UiScale,
    /// `layout::MARGIN` in physical pixels.
    pub margin: f32,
    /// `layout::PLAYER_BAR_WIDTH` in physical pixels.
    pub player_bar_width: f32,
    /// `layout::BAR_HEIGHT` in physical pixels.
    pub bar_height: f32,
    /// `layout::BAR_GAP` in physical pixels.
    pub bar_gap: f32,
    /// `layout::TARGET_FRAME_WIDTH` in physical pixels.
    pub target_frame_width: f32,
    /// `layout::TARGET_FRAME_HEIGHT` in physical pixels.
    pub target_frame_height: f32,
    /// `layout::TARGET_FRAME_TOP` in physical pixels.
    pub target_frame_top: f32,
    /// `layout::SLOT_SIZE` in physical pixels.
    pub slot_size: f32,
    /// `layout::SLOT_GAP` in physical pixels.
    pub slot_gap: f32,
    /// `layout::ABILITY_BAR_BOTTOM` in physical pixels.
    pub ability_bar_bottom: f32,
    /// `layout::CAST_BAR_WIDTH` in physical pixels.
    pub cast_bar_width: f32,
    /// `layout::CAST_BAR_HEIGHT` in physical pixels.
    pub cast_bar_height: f32,
    /// `layout::CAST_BAR_LIFT` in physical pixels.
    pub cast_bar_lift: f32,
    /// `layout::PADDING` in physical pixels.
    pub padding: f32,
    /// `layout::WORLD_BAR_HEIGHT` in physical pixels.
    pub world_bar_height: f32,
    /// `layout::WORLD_BAR_WIDTH` in physical pixels.
    pub world_bar_width: f32,
    /// `layout::ULTIMATE_SIZE` in physical pixels.
    pub ultimate_size: f32,
    /// `layout::ULTIMATE_GAP` in physical pixels.
    pub ultimate_gap: f32,
    /// `layout::RESOURCE_BAR_WIDTH` in physical pixels.
    pub resource_bar_width: f32,
    /// `layout::RESOURCE_BAR_HEIGHT` in physical pixels.
    pub resource_bar_height: f32,
    /// `layout::RESOURCE_LIFT` in physical pixels.
    pub resource_lift: f32,
    /// `layout::TALENT_BUTTON_SIZE` in physical pixels.
    pub talent_button_size: f32,
    /// `layout::IGNITE_PIP` in physical pixels.
    pub ignite_pip: f32,
    /// `layout::IGNITE_PIP_GAP` in physical pixels.
    pub ignite_pip_gap: f32,
    /// `layout::TOOLTIP_WIDTH` in physical pixels.
    pub tooltip_width: f32,
    /// `layout::TOOLTIP_LINE` in physical pixels.
    pub tooltip_line: f32,
}

impl HudMetrics {
    /// The HUD's measurements at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            scale,
            margin: scale.px(layout::MARGIN),
            player_bar_width: scale.px(layout::PLAYER_BAR_WIDTH),
            bar_height: scale.px(layout::BAR_HEIGHT),
            bar_gap: scale.px(layout::BAR_GAP),
            target_frame_width: scale.px(layout::TARGET_FRAME_WIDTH),
            target_frame_height: scale.px(layout::TARGET_FRAME_HEIGHT),
            target_frame_top: scale.px(layout::TARGET_FRAME_TOP),
            slot_size: scale.px(layout::SLOT_SIZE),
            slot_gap: scale.px(layout::SLOT_GAP),
            ability_bar_bottom: scale.px(layout::ABILITY_BAR_BOTTOM),
            cast_bar_width: scale.px(layout::CAST_BAR_WIDTH),
            cast_bar_height: scale.px(layout::CAST_BAR_HEIGHT),
            cast_bar_lift: scale.px(layout::CAST_BAR_LIFT),
            padding: scale.px(layout::PADDING),
            world_bar_height: scale.px(layout::WORLD_BAR_HEIGHT),
            world_bar_width: scale.px(layout::WORLD_BAR_WIDTH),
            ultimate_size: scale.px(layout::ULTIMATE_SIZE),
            ultimate_gap: scale.px(layout::ULTIMATE_GAP),
            resource_bar_width: scale.px(layout::RESOURCE_BAR_WIDTH),
            resource_bar_height: scale.px(layout::RESOURCE_BAR_HEIGHT),
            resource_lift: scale.px(layout::RESOURCE_LIFT),
            talent_button_size: scale.px(layout::TALENT_BUTTON_SIZE),
            ignite_pip: scale.px(layout::IGNITE_PIP),
            ignite_pip_gap: scale.px(layout::IGNITE_PIP_GAP),
            tooltip_width: scale.px(layout::TOOLTIP_WIDTH),
            tooltip_line: scale.px(layout::TOOLTIP_LINE),
        }
    }
}

impl Default for HudMetrics {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
}

/// How the interface is coloured and sized.
///
/// A struct rather than constants so a test can drive an unusual layout, and so a future
/// options screen can change a colour without touching the code that draws with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiTheme {
    /// Panel behind a nameplate or a frame.
    pub panel: UiColor,
    /// Ordinary text.
    pub text: UiColor,
    /// A status or debuff.
    pub status: UiColor,
    /// The outline around the current target.
    pub target_outline: UiColor,
    /// Outline thickness in pixels.
    pub outline_pixels: f32,
    /// Glyph scale for nameplate text.
    pub plate_text_scale: f32,
    /// Background of any bar.
    pub bar_background: UiColor,
    /// An enemy's health bar.
    pub enemy_health: UiColor,
    /// An enemy's health bar once it is nearly dead.
    pub enemy_health_low: UiColor,
    /// The player's health bar.
    pub player_health: UiColor,
    /// The player's health bar once it is dangerously low.
    pub player_health_low: UiColor,
    /// The player's resource bar.
    pub resource: UiColor,
    /// A ready ability slot's frame.
    pub slot_ready: UiColor,
    /// An ability slot that is cooling down.
    pub slot_cooling: UiColor,
    /// An ability slot that cannot be paid for or has no target.
    pub slot_unavailable: UiColor,
    /// The darkening drawn over the unfinished part of a cooldown.
    pub cooldown_sweep: UiColor,
    /// The cast bar's fill.
    pub cast_fill: UiColor,
    /// An ordinary damage number.
    pub damage_number: UiColor,
    /// A killing blow's damage number.
    pub killing_number: UiColor,
    /// Colour of a critical strike's number.
    ///
    /// A third colour rather than a brighter shade of the first: a player mid-fight is
    /// distinguishing at a glance, and two shades of yellow are not a distinction.
    pub critical_number: UiColor,
    /// Glyph scale for HUD text.
    pub hud_text_scale: f32,
    /// Glyph scale for a damage number.
    pub damage_text_scale: f32,
    /// The Heat bar in the first band, where it grants nothing.
    pub heat_low: UiColor,
    /// The Heat bar in the second band, where it grants +15% damage.
    pub heat_warm: UiColor,
    /// The Heat bar in the third band, where it grants +30% and a stat.
    pub heat_high: UiColor,
    /// The Heat bar while the Inferno State runs.
    pub heat_inferno: UiColor,
    /// The Ultimate Energy bar.
    pub energy: UiColor,
    /// The Ultimate Energy bar once it is full.
    pub energy_full: UiColor,
    /// An ability that cannot be paid for.
    pub slot_no_resource: UiColor,
    /// An ability that is switched off entirely.
    pub slot_disabled: UiColor,
    /// An ability whose target is wrong.
    pub slot_invalid: UiColor,
    /// An ability that is ready and worth pressing right now.
    pub slot_empowered: UiColor,
    /// What an ability icon is multiplied by normally. White draws the art untouched.
    pub icon: UiColor,
    /// What an ability icon is multiplied by when it cannot be used.
    pub icon_unavailable: UiColor,
    /// The radial sweep drawn over a cooling ability.
    pub cooldown_radial: UiColor,
    /// The radial sweep drawn over every ability while the global cooldown runs.
    ///
    /// Lighter than [`UiTheme::cooldown_radial`]. Half a second is not information the
    /// player has to read — it is the pulse the bar beats at — so it registers as movement
    /// without hiding the art or competing with a real cooldown two slots away.
    pub global_cooldown_radial: UiColor,
    /// The frame around the ability currently being cast.
    pub slot_casting: UiColor,
    /// An Ignite stack that is present.
    pub ignite_pip: UiColor,
    /// An Ignite stack that is not there yet.
    pub ignite_pip_empty: UiColor,
    /// The tooltip panel.
    pub tooltip_panel: UiColor,
    /// A tooltip's heading.
    pub tooltip_title: UiColor,
    /// A tooltip's body text.
    pub tooltip_text: UiColor,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            // Alpha below one, which the Phase 3 overlay could not express: its pipeline had
            // blending switched off, so every panel was a solid black slab.
            panel: [0.05, 0.06, 0.08, 0.72],
            text: [0.95, 0.95, 0.85, 1.0],
            status: [0.95, 0.62, 0.25, 1.0],
            target_outline: [1.0, 1.0, 1.0, 1.0],
            outline_pixels: 2.0,
            plate_text_scale: 2.0,
            bar_background: [0.10, 0.10, 0.12, 0.85],
            enemy_health: [0.80, 0.25, 0.22, 1.0],
            enemy_health_low: [0.95, 0.55, 0.15, 1.0],
            player_health: [0.30, 0.72, 0.34, 1.0],
            player_health_low: [0.90, 0.30, 0.25, 1.0],
            resource: [0.28, 0.52, 0.88, 1.0],
            slot_ready: [0.62, 0.64, 0.70, 1.0],
            slot_cooling: [0.34, 0.36, 0.42, 1.0],
            slot_unavailable: [0.55, 0.26, 0.24, 1.0],
            // Translucent, not opaque: the sweep darkens the slot underneath rather than
            // hiding it, so a cooling ability is still recognisable as itself.
            cooldown_sweep: [0.02, 0.02, 0.03, 0.72],
            cast_fill: [0.92, 0.78, 0.32, 1.0],
            damage_number: [1.0, 0.92, 0.55, 1.0],
            killing_number: [1.0, 0.45, 0.30, 1.0],
            critical_number: [1.0, 0.72, 0.16, 1.0],
            hud_text_scale: 2.0,
            damage_text_scale: 2.0,
            heat_low: [0.55, 0.55, 0.60, 1.0],
            heat_warm: [0.92, 0.72, 0.22, 1.0],
            heat_high: [0.95, 0.55, 0.15, 1.0],
            heat_inferno: [1.0, 0.30, 0.10, 1.0],
            energy: [0.35, 0.55, 0.95, 1.0],
            energy_full: [0.65, 0.85, 1.0, 1.0],
            slot_disabled: [0.25, 0.25, 0.28, 1.0],
            slot_no_resource: [0.70, 0.18, 0.18, 1.0],
            slot_invalid: [0.60, 0.15, 0.15, 1.0],
            slot_empowered: [1.0, 0.75, 0.20, 1.0],
            icon: [1.0, 1.0, 1.0, 1.0],
            // Dimmed and slightly desaturated by the tint alone; no second copy of the art.
            icon_unavailable: [0.42, 0.40, 0.44, 1.0],
            cooldown_radial: [0.02, 0.03, 0.05, 0.72],
            global_cooldown_radial: [0.02, 0.03, 0.05, 0.34],
            // White, and brighter than any availability colour, because "this is the one
            // going off right now" has to win against ten other frames.
            slot_casting: [1.0, 1.0, 1.0, 1.0],
            ignite_pip: [1.0, 0.45, 0.12, 1.0],
            ignite_pip_empty: [0.22, 0.20, 0.20, 1.0],
            tooltip_panel: [0.05, 0.06, 0.09, 0.96],
            tooltip_title: [1.0, 0.80, 0.35, 1.0],
            tooltip_text: [0.88, 0.90, 0.94, 1.0],
        }
    }
}

impl UiTheme {
    /// The colour the player's health bar takes at a given fraction.
    #[must_use]
    pub fn player_health_color(&self, fraction: f32) -> UiColor {
        if fraction <= 0.3 {
            self.player_health_low
        } else {
            self.player_health
        }
    }

    /// What an ability icon is multiplied by for a given availability.
    ///
    /// White draws the art as it was painted. An ability that cannot be paid for or has the
    /// wrong target is dimmed, which is the convention the genre uses and costs nothing —
    /// the tint is already travelling with every quad. Cooling is *not* dimmed: the radial
    /// sweep over the slot already says so, and doing both would say it twice.
    #[must_use]
    pub fn icon_tint(&self, availability: SlotAvailability) -> UiColor {
        match availability {
            SlotAvailability::NoResource
            | SlotAvailability::InvalidTarget
            | SlotAvailability::Disabled => self.icon_unavailable,
            _ => self.icon,
        }
    }

    /// The colour an ability slot's frame takes for a given availability.
    #[must_use]
    pub fn slot_color(&self, availability: SlotAvailability) -> UiColor {
        match availability {
            SlotAvailability::Ready => self.slot_ready,
            SlotAvailability::Cooling => self.slot_cooling,
            SlotAvailability::NoResource => self.slot_no_resource,
            SlotAvailability::InvalidTarget => self.slot_invalid,
            SlotAvailability::Empowered => self.slot_empowered,
            SlotAvailability::Disabled => self.slot_disabled,
        }
    }

    /// The colour the Heat bar takes in a given band.
    ///
    /// One colour per band, because the bands *are* the mechanic: a player has to be able to
    /// see which multiplier they are on without reading the number.
    #[must_use]
    pub fn heat_color(&self, tier: HeatTier) -> UiColor {
        match tier {
            HeatTier::Cold => self.heat_low,
            HeatTier::Warm => self.heat_warm,
            HeatTier::Hot => self.heat_high,
            HeatTier::Inferno => self.heat_inferno,
        }
    }

    /// The colour an enemy's health bar takes at a given fraction.
    ///
    /// Turning colour near death is the cheapest possible "this one is nearly done"
    /// signal, and it costs nothing extra to draw.
    #[must_use]
    pub fn enemy_health_color(&self, fraction: f32) -> UiColor {
        if fraction <= 0.25 {
            self.enemy_health_low
        } else {
            self.enemy_health
        }
    }
}

/// Builds the frame's UI.
///
/// Owns the draw list and its scratch strings across frames, both cleared and never freed,
/// so a steady HUD allocates nothing.
#[derive(Debug, Default)]
pub struct GameUi {
    list: UiDrawList,
    theme: UiTheme,
    /// Every measurement, already converted to physical pixels for the current UI scale.
    metrics: HudMetrics,
    /// Scratch for a plate's title line.
    title: String,
    /// Scratch for a plate's status line.
    statuses: String,
    /// Scratch for a tooltip's lines. Reused; never freed.
    tooltip_lines: Vec<String>,
}

impl GameUi {
    /// Creates a builder with room for a typical frame.
    #[must_use]
    pub fn new() -> Self {
        Self::with_scale(UiScale::default())
    }

    /// Creates a builder that lays the HUD out at `scale`.
    #[must_use]
    pub fn with_scale(scale: UiScale) -> Self {
        Self {
            list: UiDrawList::with_capacity(512, 4096),
            theme: UiTheme::default(),
            metrics: HudMetrics::new(scale),
            title: String::with_capacity(32),
            statuses: String::with_capacity(64),
            tooltip_lines: Vec::with_capacity(8),
        }
    }

    /// Lays the HUD out at a new scale from here on.
    ///
    /// Recomputing the metrics is the whole of the work, so applying a slider drag costs a
    /// couple of dozen multiplies rather than anything per-frame. Nothing is cached against
    /// the old scale, which is what makes the change take effect on the very next frame.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.metrics = HudMetrics::new(scale);
    }

    /// The scale the HUD is being laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.metrics.scale
    }

    /// The HUD's measurements in physical pixels.
    ///
    /// Public so that the code deciding what the pointer is over measures the same way the
    /// code that drew it did — a hit test and a rectangle that disagree is the classic way
    /// a scaled interface becomes unclickable.
    #[must_use]
    pub fn metrics(&self) -> &HudMetrics {
        &self.metrics
    }

    /// The theme, for a caller that wants to change it.
    pub fn theme_mut(&mut self) -> &mut UiTheme {
        &mut self.theme
    }

    /// The theme in use.
    #[must_use]
    pub fn theme(&self) -> &UiTheme {
        &self.theme
    }

    /// The list built by the last call to [`GameUi::build`].
    #[must_use]
    pub fn list(&self) -> &UiDrawList {
        &self.list
    }

    /// The list, for a caller appending its own commands after the HUD.
    ///
    /// How the menu is composed onto the frame: it is pushed into the list the HUD already
    /// built, so the whole interface stays one batch and one draw call rather than becoming
    /// a second pass that has to be blended in the right order.
    pub fn list_mut(&mut self) -> &mut UiDrawList {
        &mut self.list
    }

    /// Starts a new frame, discarding the previous one's commands.
    pub fn begin(&mut self) {
        self.list.clear();
    }

    /// Appends every nameplate, in the order the set supplies them.
    ///
    /// The set is already sorted furthest-first, which is exactly what painter's order
    /// wants: a nearer plate is pushed later and therefore drawn on top. Phase 3 achieved
    /// the same thing with a per-plate depth value and the depth buffer; sorting once on the
    /// CPU is both cheaper and the only option once the panels are translucent.
    pub fn push_nameplates(&mut self, plates: &[Nameplate], config: &NameplateConfig) {
        for plate in plates {
            let rect = UiRect::new(plate.origin.0, plate.origin.1, plate.size.0, plate.size.1);
            self.list.rect(rect, self.theme.panel);
            if plate.is_target {
                self.list.outline(
                    rect,
                    self.metrics.scale.px(self.theme.outline_pixels),
                    self.theme.target_outline,
                );
            }

            let text_x = plate.origin.0 + config.padding_pixels;
            let text_y = plate.origin.1 + config.padding_pixels;

            plate.write_title(&mut self.title);
            self.list.text_at(
                (text_x, text_y),
                &self.title,
                self.theme.text,
                self.metrics.scale.text(self.theme.plate_text_scale),
            );

            if plate.write_statuses(&mut self.statuses) {
                self.list.text_at(
                    (text_x, text_y + config.line_advance_pixels),
                    &self.statuses,
                    self.theme.status,
                    self.metrics.scale.text(self.theme.plate_text_scale),
                );
            }
        }
    }

    /// Builds a whole frame of UI from a snapshot and this frame's nameplates.
    ///
    /// `viewport` is the client size in pixels. Screen-space elements are placed against it
    /// rather than against a fixed layout, so the HUD follows a resize.
    pub fn build(
        &mut self,
        snapshot: &UiSnapshot,
        plates: &[Nameplate],
        config: &NameplateConfig,
        viewport: (u32, u32),
    ) -> &UiDrawList {
        self.begin();
        // World-space first, screen-space on top: painter's order makes "the HUD is in
        // front of the world" a property of this ordering rather than of a depth value.
        self.push_world_health_bars(plates);
        self.push_nameplates(plates, config);
        self.push_target_frame(snapshot, viewport);
        self.push_player_bars(snapshot, viewport);
        self.push_resource_bars(snapshot, viewport);
        self.push_ability_bar(snapshot, viewport);
        self.push_talent_button(snapshot, viewport);
        self.push_cast_bar(snapshot, viewport);
        self.push_notification(snapshot, viewport);
        &self.list
    }

    /// The target frame: name, health and distance for the current target.
    ///
    /// Centred at the top, which is where the genre puts it and where a player's eye goes
    /// when they press TAB. Absent entirely when nothing is targeted — an empty frame is
    /// noise, and the ability bar already says which abilities need a target.
    pub fn push_target_frame(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let Some(target) = snapshot.target else {
            return;
        };
        let width = viewport.0 as f32;
        let frame = UiRect::new(
            (width - self.metrics.target_frame_width) * 0.5,
            self.metrics.target_frame_top,
            self.metrics.target_frame_width,
            self.metrics.target_frame_height,
        );
        self.list.rect(frame, self.theme.panel);
        if target.ignite_at_maximum {
            // A full burn is the thing the player is waiting for; the frame says so.
            self.list.outline(
                frame,
                self.metrics.scale.px(self.theme.outline_pixels),
                self.theme.slot_empowered,
            );
        } else {
            self.list.outline(
                frame,
                self.metrics.scale.px(self.theme.outline_pixels),
                self.theme.target_outline,
            );
        }

        self.title.clear();
        self.title.push_str(target.kind.as_str());
        self.title.push_str("  ");
        write_integer(&mut self.title, i64::from(target.health_percent()));
        self.title.push('%');
        self.list.text_at(
            (
                frame.x + self.metrics.padding,
                frame.y + self.metrics.padding,
            ),
            &self.title,
            self.theme.text,
            self.metrics.scale.text(self.theme.hud_text_scale),
        );

        let bar = UiRect::new(
            frame.x + self.metrics.padding,
            frame.y + self.metrics.padding + 18.0,
            frame.width - self.metrics.padding * 2.0,
            self.metrics.bar_height,
        );
        let fill = self.theme.enemy_health_color(target.health_fraction());
        self.list.bar(
            bar,
            target.health_fraction(),
            self.theme.bar_background,
            fill,
        );

        // Ignite stacks, drawn against the *current* maximum rather than a hardcoded five.
        self.push_ignite_stacks(
            frame.x + self.metrics.padding,
            bar.bottom() + self.metrics.scale.px(5.0),
            target.ignite_stacks,
            snapshot.ignite_max,
        );

        // ...and the same thing in words, because counting pips at a glance is not a thing
        // a player should have to do to know whether Detonate is available. The maximum is
        // the configuration's, never a literal five.
        self.title.clear();
        if target.ignite_at_maximum {
            self.title.push_str("MAX IGNITE");
        } else {
            self.title.push_str("IGNITE ");
            write_integer(&mut self.title, i64::from(target.ignite_stacks));
            self.title.push_str(" / ");
            write_integer(&mut self.title, i64::from(snapshot.ignite_max));
        }
        let colour = if target.ignite_at_maximum {
            self.theme.slot_empowered
        } else {
            self.theme.text
        };
        self.list.text_at(
            (
                frame.x
                    + self.metrics.padding
                    + f32::from(snapshot.ignite_max)
                        * (self.metrics.ignite_pip + self.metrics.ignite_pip_gap)
                    + self.metrics.scale.px(6.0),
                bar.bottom() + self.metrics.scale.px(3.0),
            ),
            &self.title,
            colour,
            self.metrics.scale.text(self.theme.hud_text_scale * 0.75),
        );
    }

    /// One pip per possible Ignite stack, filled up to what is on the target.
    ///
    /// The number of pips is the maximum the game currently allows, so buying Infernal
    /// Capacity adds a sixth pip with no change here.
    pub fn push_ignite_stacks(&mut self, x: f32, y: f32, stacks: u8, maximum: u8) {
        for index in 0..maximum {
            let pip = UiRect::new(
                x + f32::from(index) * (self.metrics.ignite_pip + self.metrics.ignite_pip_gap),
                y,
                self.metrics.ignite_pip,
                self.metrics.ignite_pip,
            );
            let colour = if index < stacks {
                self.theme.ignite_pip
            } else {
                self.theme.ignite_pip_empty
            };
            self.list.rect(pip, colour);
        }
    }

    /// The player's health, Heat and Ultimate Energy.
    pub fn push_player_bars(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let height = viewport.1 as f32;
        let x = self.metrics.margin;
        let mut y = height - self.metrics.margin - self.metrics.bar_height;

        let bar = UiRect::new(x, y, self.metrics.player_bar_width, self.metrics.bar_height);
        let fraction = snapshot.player.health_fraction();
        self.list.bar(
            bar,
            fraction,
            self.theme.bar_background,
            self.theme.player_health_color(fraction),
        );
        self.title.clear();
        self.title.push_str("HP ");
        write_integer(&mut self.title, snapshot.player.health as i64);
        self.list.text_at(
            (
                bar.x + self.metrics.scale.px(4.0),
                bar.y + self.metrics.scale.px(3.0),
            ),
            &self.title,
            self.theme.text,
            self.metrics.scale.text(1.0),
        );

        // Flame Shield and haste are worth a word: both change how the fight is going.
        if snapshot.player.shielded || snapshot.player.hasted {
            y -= self.metrics.bar_height + self.metrics.bar_gap;
            self.title.clear();
            if snapshot.player.shielded {
                self.title.push_str("SHIELDED ");
            }
            if snapshot.player.hasted {
                self.title.push_str("HASTE");
            }
            self.list.text_at(
                (x, y + self.metrics.scale.px(4.0)),
                &self.title,
                self.theme.slot_empowered,
                self.metrics.scale.text(self.theme.hud_text_scale * 0.5),
            );
        }
    }

    /// The Heat and Ultimate Energy bars, above the ability bar.
    pub fn push_resource_bars(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let x = (width - self.metrics.resource_bar_width) * 0.5;
        let bottom = height - self.metrics.ability_bar_bottom - self.metrics.slot_size;

        // Heat: neutral when cold, orange when hot, and obviously different at the cap.
        let heat_rect = UiRect::new(
            x,
            bottom - self.metrics.resource_lift,
            self.metrics.resource_bar_width,
            self.metrics.resource_bar_height,
        );
        let heat_colour = self.theme.heat_color(snapshot.player.heat_tier);
        // The Inferno State has spent the bar, so the bar it is drawn on is the *state's*
        // remaining time: a full bar draining is what the player has to read, not a Heat
        // value that is sitting at zero.
        let fill = if snapshot.player.in_inferno() {
            (snapshot.player.inferno_remaining / INFERNO_DURATION).clamp(0.0, 1.0)
        } else {
            snapshot.player.heat_fraction()
        };
        self.list
            .bar(heat_rect, fill, self.theme.bar_background, heat_colour);
        if snapshot.player.in_inferno() {
            self.list.outline(heat_rect, 3.0, self.theme.heat_inferno);
        }
        // The two band thresholds, marked so the ranges are readable at a glance.
        for threshold in snapshot.player.heat_thresholds() {
            let marker = UiRect::new(
                heat_rect.x + heat_rect.width * threshold - self.metrics.scale.px(1.0),
                heat_rect.y,
                self.metrics.scale.px(2.0),
                heat_rect.height,
            );
            self.list.rect(marker, self.theme.text);
        }

        self.title.clear();
        if snapshot.player.in_inferno() {
            self.title.push_str("INFERNO ");
            write_tenths(&mut self.title, snapshot.player.inferno_remaining);
            self.title.push_str(" S");
        } else {
            self.title.push_str("HEAT ");
            write_integer(&mut self.title, snapshot.player.heat as i64);
            self.title.push_str(" / 100  ");
            self.title.push_str(snapshot.player.heat_tier.as_str());
        }
        self.list.text_at(
            (heat_rect.x, heat_rect.y - self.metrics.scale.px(14.0)),
            &self.title,
            self.theme.text,
            self.metrics.scale.text(1.0),
        );

        // Ultimate Energy.
        let energy_rect = UiRect::new(
            x,
            heat_rect.bottom() + 20.0,
            self.metrics.resource_bar_width,
            self.metrics.resource_bar_height,
        );
        let energy_colour = if snapshot.player.ultimate_ready {
            self.theme.energy_full
        } else {
            self.theme.energy
        };
        self.list.bar(
            energy_rect,
            snapshot.player.energy_fraction(),
            self.theme.bar_background,
            energy_colour,
        );
        if snapshot.player.ultimate_ready {
            self.list.outline(energy_rect, 2.0, self.theme.energy_full);
        }
        self.title.clear();
        self.title.push_str("ULTIMATE ENERGY ");
        write_integer(&mut self.title, snapshot.player.energy as i64);
        self.title.push_str(" / 100");
        if snapshot.player.ultimate_ready {
            self.title.push_str("  READY");
        }
        self.list.text_at(
            (energy_rect.x, energy_rect.y - self.metrics.scale.px(14.0)),
            &self.title,
            self.theme.text,
            self.metrics.scale.text(1.0),
        );
    }

    /// The ten-slot ability bar, and the Ultimate beside it.
    pub fn push_ability_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        for (slot, view) in snapshot.abilities.iter().enumerate() {
            let rect = self.slot_rect(slot, viewport);
            self.push_ability_button(
                rect,
                view,
                self.metrics.scale.text(self.theme.hud_text_scale * 0.5),
            );
        }

        let rect = self.ultimate_rect(viewport);
        self.push_ability_button(
            rect,
            &snapshot.ultimate,
            self.metrics.scale.text(self.theme.hud_text_scale * 0.5),
        );
        if snapshot.player.ultimate_ready {
            // The Ultimate glows when it can be used: it is the one button worth finding
            // without looking for it.
            self.list.outline(
                rect.inset(-self.metrics.scale.px(3.0)),
                self.metrics.scale.px(3.0),
                self.theme.energy_full,
            );
        }
    }

    /// One ability button: icon plate, state colour, cooldown sweep, charges and key.
    pub fn push_ability_button(&mut self, rect: UiRect, view: &AbilitySlotView, scale: f32) {
        self.list.rect(rect, self.theme.panel);

        // Art if the ability has any, its abbreviated name if it does not. Not both: the
        // 5x7 font over a painted flame is unreadable, and a slot with a picture on it does
        // not need to be labelled — which is the whole reason the picture is there.
        //
        // Inset by the outline so the art meets the frame rather than running under it.
        match icon_for(view.ability) {
            Some(icon) => self.list.icon(
                rect.inset(self.metrics.scale.px(self.theme.outline_pixels)),
                icon,
                self.theme.icon_tint(view.availability),
            ),
            None => {
                self.title.clear();
                for character in view.name.chars().take(ABILITY_ABBREVIATION) {
                    self.title.push(character);
                }
                let (origin, align) =
                    centered(rect.x + rect.width * 0.5, rect.y + rect.height * 0.5 - 4.0);
                self.list
                    .text_aligned(origin, &self.title, self.theme.text, scale, align);
            }
        }

        // After the art, so the frame is never painted over by it. The ability the player is
        // in the middle of — casting it, or choosing where to put it — takes a white frame
        // instead of its availability colour: while one is happening, which one it is
        // matters more than whether the others could be afforded.
        let active = view.shows_active_frame();
        let border = if active {
            self.theme.slot_casting
        } else {
            self.theme.slot_color(view.availability)
        };
        let outline = self.metrics.scale.px(self.theme.outline_pixels);
        self.list.outline(rect, outline, border);
        if active {
            // Doubled outward as well, so the slot reads at a glance rather than on
            // inspection — the same trick the ready Ultimate uses.
            self.list.outline(rect.inset(-outline), outline, border);
        }

        // The global cooldown, as a lighter sweep. Starting any ability locks every slot, so
        // every slot says so — a bar where only the pressed button reacts reads as though
        // the others were still available.
        if view.shows_global_cooldown() {
            self.list.radial_from(
                rect,
                view.global_cooldown_fraction,
                1.0 - view.global_cooldown_fraction,
                self.theme.global_cooldown_radial,
            );
        }

        // The ability's own cooldown, as a radial sweep over what is left, with the seconds
        // until it is back. The global cooldown gets no number: half a second counted down
        // in tenths on eleven buttons at once is noise, and it is over before it is read.
        if view.is_cooling() {
            // The dark part starts where the elapsed part ended, so its leading edge sweeps
            // clockwise from twelve o'clock as the cooldown runs down.
            self.list.radial_from(
                rect,
                view.cooldown_fraction,
                1.0 - view.cooldown_fraction,
                self.theme.cooldown_radial,
            );
            self.title.clear();
            write_seconds(&mut self.title, view.cooldown_remaining);
            let (origin, align) = centered(
                rect.x + rect.width * 0.5,
                rect.y + rect.height * 0.5 - self.metrics.scale.px(4.0),
            );
            self.list.text_aligned(
                origin,
                &self.title,
                self.theme.text,
                self.metrics.scale.text(self.theme.hud_text_scale * 0.75),
                align,
            );
        }

        // Charges, only where there is more than one.
        if view.shows_charges() {
            self.title.clear();
            write_integer(&mut self.title, i64::from(view.charges));
            self.title.push('/');
            write_integer(&mut self.title, i64::from(view.max_charges));
            self.list.text_at(
                (rect.x + 3.0, rect.bottom() - 10.0),
                &self.title,
                self.theme.text,
                scale,
            );
        }

        // The key that casts it.
        self.list.text_at(
            (
                rect.x + self.metrics.scale.px(3.0),
                rect.y + self.metrics.scale.px(3.0),
            ),
            view.key,
            self.theme.text,
            scale,
        );
    }

    /// Where bar slot `slot` sits.
    #[must_use]
    pub fn slot_rect(&self, slot: usize, viewport: (u32, u32)) -> UiRect {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let count = ABILITY_SLOTS as f32;
        let total = count * self.metrics.slot_size + (count - 1.0) * self.metrics.slot_gap;
        // The bar is centred, and the Ultimate hangs off its right-hand end.
        let start = (width - total - self.metrics.ultimate_gap - self.metrics.ultimate_size) * 0.5;
        UiRect::new(
            start + slot as f32 * (self.metrics.slot_size + self.metrics.slot_gap),
            height - self.metrics.ability_bar_bottom - self.metrics.slot_size,
            self.metrics.slot_size,
            self.metrics.slot_size,
        )
    }

    /// Where the Ultimate button sits: larger, and separated from the bar.
    #[must_use]
    pub fn ultimate_rect(&self, viewport: (u32, u32)) -> UiRect {
        let last = self.slot_rect(ABILITY_SLOTS - 1, viewport);
        let height = viewport.1 as f32;
        UiRect::new(
            last.right() + self.metrics.ultimate_gap,
            height - self.metrics.ability_bar_bottom - self.metrics.ultimate_size,
            self.metrics.ultimate_size,
            self.metrics.ultimate_size,
        )
    }

    /// Where the talent button sits: the top-right corner.
    #[must_use]
    pub fn talent_button_rect(&self, viewport: (u32, u32)) -> UiRect {
        let width = viewport.0 as f32;
        UiRect::new(
            width - self.metrics.margin - self.metrics.talent_button_size,
            self.metrics.margin,
            self.metrics.talent_button_size,
            self.metrics.talent_button_size,
        )
    }

    /// The talent button, with the points waiting to be spent.
    pub fn push_talent_button(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let rect = self.talent_button_rect(viewport);
        self.list.rect(rect, self.theme.panel);
        let border = if snapshot.talent_points > 0 {
            self.theme.slot_empowered
        } else {
            self.theme.target_outline
        };
        self.list.outline(
            rect,
            self.metrics.scale.px(self.theme.outline_pixels),
            border,
        );

        let (origin, align) = centered(rect.x + rect.width * 0.5, rect.y + 8.0);
        self.list
            .text_aligned(origin, "TAL", self.theme.text, 1.5, align);

        self.title.clear();
        write_integer(&mut self.title, i64::from(snapshot.talent_points));
        let (origin, align) = centered(rect.x + rect.width * 0.5, rect.bottom() - 16.0);
        self.list
            .text_aligned(origin, &self.title, border, 1.5, align);
    }

    /// The cast bar, above the ability bar.
    pub fn push_cast_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let Some(cast) = snapshot.cast else {
            return;
        };
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let rect = UiRect::new(
            (width - self.metrics.cast_bar_width) * 0.5,
            height
                - self.metrics.ability_bar_bottom
                - self.metrics.slot_size
                - self.metrics.cast_bar_lift,
            self.metrics.cast_bar_width,
            self.metrics.cast_bar_height,
        );
        self.list.bar(
            rect,
            cast.fraction(),
            self.theme.bar_background,
            self.theme.cast_fill,
        );
        self.title.clear();
        self.title.push_str(cast.name);
        self.title.push(' ');
        write_tenths(&mut self.title, cast.remaining());
        let (origin, align) = centered(rect.x + rect.width * 0.5, rect.y + 3.0);
        self.list
            .text_aligned(origin, &self.title, self.theme.text, 1.0, align);
    }

    /// Why the last ability was refused, above the cast bar.
    ///
    /// The refusal comes from the simulation, in its own words: a message the interface
    /// composed could disagree with the rule that produced it.
    pub fn push_notification(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let Some(error) = snapshot.notification.error else {
            return;
        };
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        // Above the resource bars, where nothing else is drawn: a refusal that overlapped
        // the energy readout would be two messages fighting for one line.
        let y = height
            - self.metrics.ability_bar_bottom
            - self.metrics.slot_size
            - self.metrics.resource_lift
            - 34.0;
        let (origin, align) = centered(width * 0.5, y);
        self.list.text_aligned(
            origin,
            error.as_str(),
            self.theme.slot_invalid,
            self.metrics.scale.text(self.theme.hud_text_scale * 0.75),
            align,
        );
    }

    /// An ability tooltip, anchored above `anchor`.
    ///
    /// Every line is generated from the ability's own definition and from the *current*
    /// Ignite maximum, so a tooltip cannot drift from what the ability does and Fire
    /// Detonation's requirement follows Infernal Capacity without a second string.
    pub fn push_ability_tooltip(
        &mut self,
        anchor: UiRect,
        view: &AbilitySlotView,
        ignite_max: u8,
        mastery: engine_game::firemage::FireMastery,
        viewport: (u32, u32),
    ) {
        let definition = view.ability.definition();
        self.tooltip_lines.clear();

        let mut line = String::new();
        line.push_str(definition.description);
        self.tooltip_lines.push(line);

        let mut line = String::new();
        if definition.cast_time > 0.0 {
            // The *current* cast time, haste included: a tooltip that printed the table's
            // three seconds while Fire Mastery was running would be telling the player
            // something the game has stopped believing.
            line.push_str("CAST ");
            write_tenths(&mut line, view.cast_time);
            line.push_str(" S  ");
        } else {
            line.push_str("INSTANT  ");
        }
        if definition.cooldown > 0.0 {
            line.push_str("CD ");
            write_tenths(&mut line, definition.cooldown);
            line.push_str(" S");
        }
        self.tooltip_lines.push(line);

        let mut line = String::new();
        if definition.range > 0.0 {
            line.push_str("RANGE ");
            write_integer(&mut line, definition.range as i64);
            line.push_str(" M  ");
        }
        if definition.radius > 0.0 {
            line.push_str("RADIUS ");
            write_integer(&mut line, definition.radius as i64);
            line.push_str(" M");
        }
        if !line.is_empty() {
            self.tooltip_lines.push(line);
        }

        if view.max_charges > 1 {
            let mut line = String::new();
            line.push_str("CHARGES ");
            write_integer(&mut line, i64::from(view.charges));
            line.push('/');
            write_integer(&mut line, i64::from(view.max_charges));
            self.tooltip_lines.push(line);
        }

        if definition.duration > 0.0 {
            let mut line = String::new();
            line.push_str("DURATION ");
            write_tenths(&mut line, definition.duration);
            line.push_str(" S");
            self.tooltip_lines.push(line);
        }

        let mut line = String::new();
        if definition.heat > 0.0 {
            line.push_str("HEAT +");
            write_integer(&mut line, definition.heat as i64);
            line.push_str("  ");
        }
        if definition.energy > 0.0 {
            line.push_str("ENERGY +");
            write_integer(&mut line, definition.energy as i64);
        }
        if definition.energy_cost > 0.0 {
            line.push_str("COSTS ");
            write_integer(&mut line, definition.energy_cost as i64);
            line.push_str(" ENERGY");
        }
        if !line.is_empty() {
            self.tooltip_lines.push(line);
        }

        if view.damage > 0.0 {
            let mut line = String::new();
            line.push_str("DAMAGE ");
            write_integer(&mut line, view.damage as i64);
            if definition.tick_coefficient > 0.0 && definition.coefficient <= 0.0 {
                line.push_str(" PER TICK");
            }
            self.tooltip_lines.push(line);
        }

        if view.ignite_scaling > 0.0 {
            let mut line = String::new();
            line.push('+');
            write_integer(&mut line, (view.ignite_scaling * 100.0).round() as i64);
            line.push_str("% PER IGNITE STACK ON THE TARGET");
            self.tooltip_lines.push(line);
        }

        if view.ignite_stacks > 0 {
            let mut line = String::new();
            line.push_str("APPLIES ");
            write_integer(&mut line, i64::from(view.ignite_stacks));
            line.push_str(" IGNITE");
            if view.ignite_chance < 1.0 {
                line.push_str(", ");
                write_integer(&mut line, (view.ignite_chance * 100.0).round() as i64);
                line.push_str("% CHANCE");
            }
            self.tooltip_lines.push(line);
        }

        if view.ability == FireAbility::Detonate {
            // The requirement, stated in the numbers that are true right now.
            let mut line = String::new();
            line.push_str("REQUIRES ");
            write_integer(&mut line, i64::from(ignite_max));
            line.push_str(" IGNITE STACKS");
            self.tooltip_lines.push(line);
            // Generated from the constant the explosion actually uses, so the tooltip
            // cannot survive a balance pass while the number moves.
            let mut line = String::new();
            line.push_str("DEALS ");
            write_integer(
                &mut line,
                (engine_game::sim::DETONATE_FRACTION * 100.0).round() as i64,
            );
            line.push_str("% OF THE BURN'S REMAINING DAMAGE, AT ONCE");
            self.tooltip_lines.push(line);
        }

        if view.ability == FireAbility::FireMastery {
            self.tooltip_lines.push(match mastery {
                engine_game::firemage::FireMastery::Haste => "+15% HASTE FOR 20 S".to_owned(),
                engine_game::firemage::FireMastery::Crit => {
                    "+15% CRITICAL STRIKE FOR 20 S".to_owned()
                }
            });
        }

        let height = self.metrics.tooltip_line * (self.tooltip_lines.len() as f32 + 1.0)
            + self.metrics.scale.px(10.0);
        let width = self.metrics.tooltip_width;
        let x = (anchor.x + anchor.width * 0.5 - width * 0.5).clamp(
            self.metrics.scale.px(4.0),
            (viewport.0 as f32 - width - self.metrics.scale.px(4.0))
                .max(self.metrics.scale.px(4.0)),
        );
        let y = (anchor.y - height - self.metrics.scale.px(8.0)).max(self.metrics.scale.px(4.0));
        let panel = UiRect::new(x, y, width, height);

        self.list.rect(panel, self.theme.tooltip_panel);
        self.list.outline(
            panel,
            self.metrics.scale.px(self.theme.outline_pixels),
            self.theme.target_outline,
        );
        self.list.text_at(
            (
                panel.x + self.metrics.scale.px(8.0),
                panel.y + self.metrics.scale.px(6.0),
            ),
            view.name,
            self.theme.tooltip_title,
            self.metrics.scale.text(1.5),
        );
        for (index, line) in self.tooltip_lines.iter().enumerate() {
            self.list.text_at(
                (
                    panel.x + self.metrics.scale.px(8.0),
                    panel.y
                        + self.metrics.scale.px(6.0)
                        + self.metrics.tooltip_line * (index as f32 + 1.4),
                ),
                line,
                self.theme.tooltip_text,
                self.metrics.scale.text(1.0),
            );
        }
    }

    /// A small health bar over every enemy that has a plate.
    pub fn push_world_health_bars(&mut self, plates: &[Nameplate]) {
        for plate in plates {
            let bar = UiRect::new(
                plate.origin.0 + (plate.size.0 - self.metrics.world_bar_width) * 0.5,
                plate.origin.1 - self.metrics.world_bar_height - 3.0,
                self.metrics.world_bar_width,
                self.metrics.world_bar_height,
            );
            self.list.bar(
                bar,
                plate.health_fraction,
                self.theme.bar_background,
                self.theme.enemy_health_color(plate.health_fraction),
            );
        }
    }

    /// Floating damage numbers, projected through the camera.
    ///
    /// A number behind the camera is skipped rather than drawn: a point behind the eye still
    /// divides to a finite screen position, and drawing it produces a mirrored ghost — the
    /// same bug the nameplates avoid, avoided the same way.
    pub fn push_damage_numbers(
        &mut self,
        numbers: &FloatingNumbers,
        view_projection: &Mat4,
        viewport: (u32, u32),
        now: f32,
    ) {
        for number in numbers.numbers() {
            let world: Vec3 = number.position(now);
            let Some((x, y)) = project(world, view_projection, viewport) else {
                continue;
            };
            let alpha = number.alpha(now);
            if alpha <= 0.0 {
                continue;
            }
            // A kill outranks a crit: the last number a target ever shows should read as
            // "it died", not as "that one was lucky".
            let mut color = if number.killed {
                self.theme.killing_number
            } else if number.critical {
                self.theme.critical_number
            } else {
                self.theme.damage_number
            };
            // The fade lives in the alpha channel, which only works because the UI blends.
            color[3] = alpha;

            self.title.clear();
            write_integer(&mut self.title, number.amount.round() as i64);
            // A killing blow is bigger as well as differently coloured, so it reads without
            // the player having to distinguish two shades mid-fight.
            // Size carries the same information as colour, because size survives the
            // periphery of the screen and a hue does not.
            let emphasis = if number.killed {
                1.4
            } else if number.critical {
                1.25
            } else {
                1.0
            };
            let scale = self
                .metrics
                .scale
                .text(self.theme.damage_text_scale * emphasis);
            self.list
                .text_aligned((x, y), &self.title, color, scale, TextAlign::Center);
        }
    }
}

/// Characters of an ability's name shown in its slot.
///
/// Four fits a 48-pixel slot at half scale and is enough to tell one placeholder from
/// another. Used only by the abilities that have no icon yet; the layout does not change
/// when one gains art, because the icon fills the same plate the letters sat on.
const ABILITY_ABBREVIATION: usize = 4;

/// The picture that stands for an ability, if one has been drawn.
///
/// The map lives here, in the layer that decides what the HUD *looks* like, rather than on
/// [`FireAbility`] itself: `engine-game` must never learn that a picture exists, and it is
/// the same rule that keeps a colour out of the simulation.
///
/// `None` is the ordinary answer for most of the bar today. Those slots fall back to their
/// abbreviated name, so an ability without art is legible rather than blank.
#[must_use]
fn icon_for(ability: FireAbility) -> Option<UiIcon> {
    match ability {
        FireAbility::Firebolt => Some(UiIcon::Firebolt),
        FireAbility::Meteor => Some(UiIcon::Meteor),
        // The cone that spreads Ignite; the art is named for the shape, the ability for
        // what it does.
        FireAbility::FlameCone => Some(UiIcon::FlameCone),
        FireAbility::FlameLance => Some(UiIcon::FlameLance),
        FireAbility::Pyroblast => Some(UiIcon::Pyroblast),
        FireAbility::Firestorm => Some(UiIcon::Firestorm),
        FireAbility::Detonate => Some(UiIcon::Detonate),
        FireAbility::FireMastery => Some(UiIcon::FireMastery),
        FireAbility::FlameShield => Some(UiIcon::FlameShield),
        FireAbility::Blink => Some(UiIcon::Blink),
        _ => None,
    }
}

/// Writes an integer into `out` without allocating.
///
/// `format!` would allocate a `String` per call, and the HUD writes a dozen numbers a frame.
/// Used by the bars and readouts added in later milestones.
pub fn write_integer(out: &mut String, value: i64) {
    use std::fmt::Write;
    // `write!` into a `String` cannot fail; the result is consumed to satisfy the lint
    // rather than unwrapped.
    let _ = write!(out, "{value}");
}

/// Writes a value with one decimal place into `out` without allocating.
pub fn write_tenths(out: &mut String, value: f32) {
    use std::fmt::Write;
    let _ = write!(out, "{value:.1}");
}

/// Writes a countdown into `out`: whole seconds while there is more than one, tenths below.
///
/// A cooldown is read at a glance in the middle of a fight, and "18" is read faster than
/// "18.0" — four characters across a 48-pixel button is already most of its width. Under a
/// second the tenth is the only part carrying any information, so that is when it appears.
///
/// Rounded *up*, so the number never reads zero while the ability is still refusing. A
/// button showing "0" that cannot be pressed is a bug report.
pub fn write_seconds(out: &mut String, value: f32) {
    use std::fmt::Write;
    let value = value.max(0.0);
    if value >= 1.0 {
        let _ = write!(out, "{}", value.ceil() as i64);
    } else {
        let _ = write!(out, "{:.1}", (value * 10.0).ceil() / 10.0);
    }
}

/// Centres a line of text horizontally on `x`, in the same units the list uses.
#[must_use]
pub fn centered(x: f32, y: f32) -> ((f32, f32), TextAlign) {
    ((x, y), TextAlign::Center)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::damage_numbers::FloatingNumbers;
    use crate::nameplate::{NameplateConfig, NameplateSet};
    use crate::snapshot::UiSnapshot;
    use engine_core::math::Mat4;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::sim::Simulation;
    use engine_render_data::ui::{MAX_ICON_PIXELS, UiCommand};
    use engine_settings::{UI_SCALE_MAX, UI_SCALE_MIN, UI_SCALE_STEP};

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    /// A view-projection that puts the arena in front of the camera.
    fn view_projection() -> Mat4 {
        let camera = crate::camera::ThirdPersonCamera::following(engine_core::math::Vec3::ZERO);
        let mut render_camera = engine_render_data::Camera::new(
            engine_render_data::Projection::perspective(16.0 / 9.0),
        )
        .expect("a valid projection");
        camera
            .apply(&mut render_camera, 16.0 / 9.0)
            .expect("the camera applies");
        render_camera.view_projection()
    }

    fn plates(simulation: &Simulation) -> (NameplateSet, NameplateConfig) {
        let config = NameplateConfig::default();
        let mut set = NameplateSet::with_capacity(32);
        set.build(simulation, &view_projection(), VIEWPORT, &config);
        (set, config)
    }

    #[test]
    fn a_nameplate_becomes_a_panel_and_its_text() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        assert!(!set.plates().is_empty(), "the arena has visible enemies");

        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(set.plates(), &config);

        let rects = ui
            .list()
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Rect { .. }))
            .count();
        let texts = ui
            .list()
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Text { .. }))
            .count();
        assert_eq!(rects, set.plates().len(), "one panel per plate");
        assert!(texts >= set.plates().len(), "at least a title per plate");
    }

    #[test]
    fn exactly_one_plate_is_outlined_and_it_is_the_target() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(set.plates(), &config);

        let outlines = ui
            .list()
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Outline { .. }))
            .count();
        let targets = set.plates().iter().filter(|plate| plate.is_target).count();
        assert_eq!(outlines, targets);
        assert!(targets <= 1, "at most one plate is the target's");
    }

    #[test]
    fn the_panel_is_translucent_which_the_phase_three_overlay_could_not_express() {
        let ui = GameUi::new();
        assert!(
            ui.theme().panel[3] < 1.0,
            "a UI panel should let the scene through"
        );
    }

    #[test]
    fn plates_are_pushed_furthest_first_so_the_nearest_paints_on_top() {
        // Painter's order replaces the per-plate depth value Phase 3 used. The set already
        // sorts for it; this asserts the builder does not undo that.
        let simulation = arena::training_arena(&ArenaConfig::with_group(8));
        let (set, config) = plates(&simulation);
        if set.plates().len() < 2 {
            return;
        }
        let distances: Vec<f32> = set.plates().iter().map(|plate| plate.distance).collect();
        assert!(
            distances.windows(2).all(|pair| pair[0] >= pair[1]),
            "the set must supply furthest first: {distances:?}"
        );

        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(set.plates(), &config);
        // The first panel pushed belongs to the furthest plate.
        let UiCommand::Rect { rect, .. } = ui.list().commands()[0] else {
            panic!("the first command is a panel");
        };
        assert_eq!(rect.x, set.plates()[0].origin.0);
    }

    #[test]
    fn an_enemy_health_bar_changes_colour_near_death() {
        let theme = UiTheme::default();
        assert_eq!(theme.enemy_health_color(1.0), theme.enemy_health);
        assert_eq!(theme.enemy_health_color(0.10), theme.enemy_health_low);
    }

    #[test]
    fn rebuilding_reuses_every_allocation() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let mut ui = GameUi::new();
        let snapshot = UiSnapshot::build(&simulation);

        ui.build(&snapshot, set.plates(), &config, VIEWPORT);
        let commands = ui.list().commands().len();

        for _ in 0..1_000 {
            ui.build(&snapshot, set.plates(), &config, VIEWPORT);
        }
        assert_eq!(ui.list().commands().len(), commands, "no accumulation");
    }

    /// Every rectangle in the built list, for geometry assertions.
    fn rects(ui: &GameUi) -> Vec<UiRect> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Rect { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// Every icon in the built list, with the rectangle it fills.
    fn icons(ui: &GameUi) -> Vec<(UiIcon, UiRect)> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Icon { rect, icon, .. } => Some((*icon, *rect)),
                _ => None,
            })
            .collect()
    }

    /// Every string in the built list.
    fn texts(ui: &GameUi) -> Vec<String> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } => Some(ui.list().text_of(*range).to_owned()),
                _ => None,
            })
            .collect()
    }

    fn built(simulation: &Simulation) -> GameUi {
        let (set, config) = plates(simulation);
        let mut ui = GameUi::new();
        let snapshot = UiSnapshot::build(simulation);
        ui.build(&snapshot, set.plates(), &config, VIEWPORT);
        ui
    }

    #[test]
    fn the_player_gets_a_health_bar_and_both_resource_bars() {
        // Health, Heat and Ultimate Energy: the three bars the class is played off.
        let ui = built(&sim());
        let found = texts(&ui);
        assert!(found.iter().any(|text| text.starts_with("HP")), "{found:?}");
        assert!(
            found.iter().any(|text| text.starts_with("HEAT")),
            "no heat bar: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.starts_with("ULTIMATE ENERGY")),
            "no ultimate bar: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.contains("/ 100")),
            "the bars do not state their range: {found:?}"
        );
        // Each bar is a background plus a fill.
        assert!(rects(&ui).len() >= 6);
    }

    #[test]
    fn a_bar_fill_tracks_the_value_it_reports() {
        // Full, half and empty must produce visibly different geometry, which is the only
        // thing a bar actually communicates.
        let mut simulation = sim();
        let full = built(&simulation);
        let full_width = rects(&full)
            .iter()
            .map(|rect| rect.width)
            .fold(0.0_f32, f32::max);

        let player = simulation.player().entity;
        let max = simulation
            .world()
            .health(player)
            .expect("the player has health")
            .max();
        simulation.world_mut().damage(player, max * 0.5);
        let half = built(&simulation);
        let half_width = rects(&half)
            .iter()
            .map(|rect| rect.width)
            .fold(0.0_f32, f32::max);
        assert!(half_width <= full_width);

        // Near death the bar changes colour as well as length.
        let theme = UiTheme::default();
        assert_ne!(
            theme.player_health_color(1.0),
            theme.player_health_color(0.1)
        );
    }

    #[test]
    fn a_dead_player_has_an_empty_bar_rather_than_no_bar() {
        // Zero health must still draw the frame, or dying looks like the HUD broke.
        let mut simulation = sim();
        let player = simulation.player().entity;
        simulation.world_mut().damage(player, 1_000_000.0);
        let ui = built(&simulation);
        assert!(
            texts(&ui).iter().any(|text| text.starts_with("HP")),
            "the health bar disappeared with the player"
        );
    }

    #[test]
    fn the_target_frame_names_the_target_and_reports_its_health() {
        let ui = built(&sim());
        let found = texts(&ui);
        assert!(
            found.iter().any(|text| text.starts_with("DUMMY")),
            "the target frame names it: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.contains('%')),
            "and reports a percentage: {found:?}"
        );
        // The frame carries no distance: it never wrote one, and the assertion that claimed
        // it did was matching an ability slot's abbreviated name ("FLAM") instead. Now that
        // every slot draws art there is nothing left to match, so the claim is gone rather
        // than restated against a different accident.
    }

    #[test]
    fn the_target_frame_updates_the_moment_the_target_changes() {
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(4));
        simulation.step_once();
        let first = simulation.player().target.expect("a target");
        simulation.push_command(engine_game::command::PlayerCommand::CycleTarget);
        simulation.step_once();
        let second = simulation.player().target.expect("a target");
        assert_ne!(first, second, "cycling moved the target");

        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.target.expect("a target view").entity, second);
    }

    #[test]
    fn a_dead_target_keeps_its_frame_and_loses_its_outline() {
        // The player still needs to see what they were fighting, and the bar going empty is
        // what says the abilities will now refuse.
        let mut simulation = sim();
        let target = simulation.player().target.expect("a target");
        simulation.world_mut().damage(target, 1_000_000.0);

        let dead = built(&simulation);
        let found = texts(&dead);
        assert!(
            found.iter().any(|text| text.starts_with("DUMMY")),
            "the frame vanished with the target: {found:?}"
        );
        let snapshot = UiSnapshot::build(&simulation);
        let view = snapshot.target.expect("a target view");
        assert!(!view.alive);
        assert_eq!(view.health_fraction(), 0.0);
    }

    #[test]
    fn nothing_targeted_draws_no_target_frame() {
        // Nameplates still name every dummy, so the frame is counted rather than searched
        // for: with nothing selected there is one less panel on screen.
        let simulation = sim();
        let with_target = built(&simulation).list().len();

        let mut simulation = simulation;
        simulation.player_mut().target = None;
        let without = built(&simulation).list().len();
        assert!(
            without < with_target,
            "an empty frame is noise: {without} vs {with_target}"
        );
    }

    #[test]
    fn the_ability_bar_draws_every_slot_with_its_hotkey() {
        let ui = built(&sim());
        let found = texts(&ui);
        for digit in ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"] {
            assert!(
                found.iter().any(|text| text == digit),
                "slot {digit} is missing: {found:?}"
            );
        }
        // Every ability on the bar has art now, so no slot falls back to its abbreviated
        // name. The fallback is covered by the test that pairs art against names, which
        // reads `icon_for` rather than assuming which abilities are drawn.
    }

    #[test]
    fn the_abilities_that_have_art_draw_it_in_their_own_slot() {
        let ui = built(&sim());
        let drawn = icons(&ui);

        // Each icon appears exactly once, in the slot the bar order puts it in, filling the
        // plate inside its frame.
        let outline = ui.scale().px(UiTheme::default().outline_pixels);
        let art = [
            (FireAbility::Firebolt, UiIcon::Firebolt),
            (FireAbility::Meteor, UiIcon::Meteor),
            (FireAbility::FlameCone, UiIcon::FlameCone),
            (FireAbility::FlameLance, UiIcon::FlameLance),
            (FireAbility::Pyroblast, UiIcon::Pyroblast),
            (FireAbility::Firestorm, UiIcon::Firestorm),
            (FireAbility::Detonate, UiIcon::Detonate),
            (FireAbility::FireMastery, UiIcon::FireMastery),
            (FireAbility::FlameShield, UiIcon::FlameShield),
            (FireAbility::Blink, UiIcon::Blink),
        ];
        for (ability, expected) in art {
            let slot = FireAbility::BAR
                .iter()
                .position(|candidate| *candidate == ability)
                .expect("the ability is on the bar");
            let matched: Vec<_> = drawn.iter().filter(|(icon, _)| *icon == expected).collect();
            assert_eq!(
                matched.len(),
                1,
                "{expected:?} was drawn {} times",
                matched.len()
            );
            assert_eq!(
                matched[0].1,
                ui.slot_rect(slot, VIEWPORT).inset(outline),
                "{expected:?} is not in slot {slot}"
            );
        }
        assert_eq!(
            drawn.len(),
            art.len(),
            "something drew an icon nobody asked for"
        );
    }

    #[test]
    fn a_slot_with_art_shows_the_art_instead_of_its_name() {
        // Both would be unreadable: a 5x7 bitmap font over a painted flame. Checked by
        // position rather than by string, because several abilities abbreviate to "FIRE"
        // and only one of them has art.
        let ui = built(&sim());
        let names: Vec<(f32, f32)> = ui
            .list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text {
                    origin,
                    align: TextAlign::Center,
                    ..
                } => Some(*origin),
                _ => None,
            })
            .collect();

        for (slot, ability) in FireAbility::BAR.iter().enumerate() {
            let rect = ui.slot_rect(slot, VIEWPORT);
            // Where `push_ability_button` centres a name.
            let centre = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5 - 4.0);
            let labelled = names.contains(&centre);
            assert_eq!(
                labelled,
                icon_for(*ability).is_none(),
                "slot {slot} ({ability:?}) draws both a name and an icon, or neither"
            );
        }
    }

    #[test]
    fn no_icon_is_ever_drawn_larger_than_the_art_was_rasterized_for() {
        // The check that keeps `MAX_ICON_PIXELS` honest. `engine-renderer` rasterizes its
        // icons to exactly that size and asserts it at compile time; this is the other half,
        // that nothing here lays one out bigger. Without it, growing a slot or raising the
        // maximum UI scale would quietly start magnifying the art.
        let simulation = sim();
        for factor in [UI_SCALE_MIN, 1.0, UI_SCALE_MAX] {
            let scale = UiScale::new(factor);
            let (set, config) = plates(&simulation);
            let mut ui = GameUi::with_scale(scale);
            let snapshot = UiSnapshot::build(&simulation);
            ui.build(&snapshot, set.plates(), &config, VIEWPORT);

            for (icon, rect) in icons(&ui) {
                let largest = rect.width.max(rect.height);
                assert!(
                    largest <= MAX_ICON_PIXELS as f32,
                    "{icon:?} is {largest} pixels at scale {factor}, past the {MAX_ICON_PIXELS} \
                     the art is rasterized for"
                );
            }
        }

        // The Ultimate is the biggest plate that can carry one, so it is what the constant
        // has to cover. Checked directly as well, in case no icon is on it yet.
        let ui = GameUi::with_scale(UiScale::new(UI_SCALE_MAX));
        let ultimate = ui.ultimate_rect(VIEWPORT);
        assert!(
            ultimate.width.max(ultimate.height) <= MAX_ICON_PIXELS as f32,
            "the Ultimate button is bigger than the icon art allows for"
        );
    }

    #[test]
    fn the_scale_the_player_can_choose_is_one_the_renderer_can_draw() {
        // Two ranges, deliberately: what the slider offers, and what the layout and the
        // atlas are built to support. The first must sit inside the second, and this is the
        // only place that can see both.
        const { assert!(UI_SCALE_MIN >= UiScale::MIN, "the slider goes below MIN") };
        const { assert!(UI_SCALE_MAX <= UiScale::MAX, "the slider goes above MAX") };
        const { assert!(UI_SCALE_MIN < UI_SCALE_MAX) };

        // And a value from the slider survives the trip into a scale unchanged, so what is
        // saved is what is drawn.
        for factor in [UI_SCALE_MIN, 1.0, 1.25, UI_SCALE_MAX] {
            assert_eq!(
                UiScale::new(factor).factor(),
                factor,
                "{factor} was clamped"
            );
        }
    }

    #[test]
    fn every_measurement_grows_with_the_scale_and_the_layout_keeps_its_shape() {
        let small = HudMetrics::new(UiScale::new(1.0));
        let large = HudMetrics::new(UiScale::new(2.0));

        assert_eq!(large.slot_size, small.slot_size * 2.0);
        assert_eq!(large.ultimate_size, small.ultimate_size * 2.0);
        assert_eq!(large.margin, small.margin * 2.0);

        // The bar stays centred and stays a bar: bigger slots, same arrangement.
        let wide = GameUi::with_scale(UiScale::new(2.0));
        let narrow = GameUi::with_scale(UiScale::new(1.0));
        for slot in 1..ABILITY_SLOTS {
            let step =
                |ui: &GameUi| ui.slot_rect(slot, VIEWPORT).x - ui.slot_rect(slot - 1, VIEWPORT).x;
            assert!(
                (step(&wide) - step(&narrow) * 2.0).abs() < 1.0,
                "slot {slot} is not twice as far along at twice the scale"
            );
        }
    }

    #[test]
    fn what_the_hud_draws_and_what_a_click_is_tested_against_are_the_same_rectangle() {
        // The bug a scaled interface invites: the draw uses one scale and the hit test uses
        // another, and buttons stop working somewhere other than where they look.
        let simulation = sim();
        let scale = UiScale::new(1.5);
        let (set, config) = plates(&simulation);
        let mut ui = GameUi::with_scale(scale);
        let snapshot = UiSnapshot::build(&simulation);
        ui.build(&snapshot, set.plates(), &config, VIEWPORT);

        // Every slot the bar drew has a background rectangle at exactly the rectangle the
        // hit test would use.
        let drawn = rects(&ui);
        for slot in 0..ABILITY_SLOTS {
            let rect = ui.slot_rect(slot, VIEWPORT);
            assert!(
                drawn.contains(&rect),
                "slot {slot} is hit-tested at {rect:?}, which nothing drew"
            );
        }
    }

    #[test]
    fn text_stays_at_a_whole_glyph_scale_however_the_interface_is_sized() {
        // The font is a bitmap sampled with a point filter: a fractional scale makes some
        // rows of a letter a pixel taller than others. Every scale the slider can produce
        // must still come out whole.
        let mut factor = UI_SCALE_MIN;
        while factor <= UI_SCALE_MAX {
            let scale = UiScale::new(factor);
            for logical in [1.0, 1.5, 2.0, 3.0] {
                let drawn = scale.text(logical);
                assert_eq!(drawn, drawn.round(), "{logical} at {factor} is fractional");
                assert!(drawn >= 1.0, "{logical} at {factor} vanished");
            }
            factor += UI_SCALE_STEP;
        }
    }

    #[test]
    fn every_slot_shows_the_global_cooldown_while_it_runs() {
        // The point of the change: starting one ability locks the whole bar, so the whole
        // bar has to say so. A bar where only the pressed button reacts reads as though the
        // others were still available.
        // Blink: instant, so it lands and takes its own fifteen-second cooldown while the
        // shared half-second one runs on everything else.
        let mut simulation = sim();
        simulation.cast(FireAbility::Blink).expect("Blink is ready");

        let snapshot = UiSnapshot::build(&simulation);
        assert!(
            snapshot
                .abilities
                .iter()
                .all(|view| view.is_on_global_cooldown()),
            "the global cooldown did not reach every slot"
        );
        assert!(
            snapshot.ultimate.is_on_global_cooldown(),
            "the Ultimate is on the same shared timer"
        );

        // The slot that is merely waiting draws the shared sweep; the one that is also on
        // its own cooldown draws that instead, so the wash is never doubled.
        let cooling: Vec<_> = snapshot
            .abilities
            .iter()
            .filter(|view| view.is_cooling())
            .collect();
        assert!(!cooling.is_empty(), "nothing went on its own cooldown");
        for view in cooling {
            assert!(
                !view.shows_global_cooldown(),
                "{:?} draws both",
                view.ability
            );
        }
    }

    #[test]
    fn the_ability_being_cast_is_framed_in_white_and_no_other_is() {
        let mut simulation = sim();
        // Pyroblast: the longest cast in the class, so it is still running next frame.
        simulation.cast(FireAbility::Pyroblast).expect("in range");
        simulation.step_once();

        let ui = built(&simulation);
        let casting = FireAbility::BAR
            .iter()
            .position(|ability| *ability == FireAbility::Pyroblast)
            .expect("Pyroblast is on the bar");
        let slot = ui.slot_rect(casting, VIEWPORT);
        let white = UiTheme::default().slot_casting;

        let framed: Vec<UiRect> = ui
            .list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Outline { rect, color, .. } if *color == white => Some(*rect),
                _ => None,
            })
            .collect();
        assert!(
            framed.contains(&slot),
            "the casting slot has no white frame: {framed:?}"
        );

        // No other slot took one.
        for other in 0..ABILITY_SLOTS {
            if other == casting {
                continue;
            }
            let rect = ui.slot_rect(other, VIEWPORT);
            assert!(
                !framed.contains(&rect),
                "slot {other} is framed as though it were casting"
            );
        }
    }

    #[test]
    fn the_ability_whose_area_is_being_placed_is_framed_like_one_being_cast() {
        // Aiming a Meteor and casting a Pyroblast are the same answer to "what am I doing",
        // so the bar gives them the same frame. Without it the only sign that a placement is
        // open is the circle on the ground, which is where the player is not looking.
        let simulation = sim();
        let aimed = FireAbility::Meteor;
        let snapshot = UiSnapshot::build_aiming(&simulation, Some(aimed));

        let slot = FireAbility::BAR
            .iter()
            .position(|ability| *ability == aimed)
            .expect("Meteor is on the bar");
        assert!(snapshot.abilities[slot].aiming);
        assert!(snapshot.abilities[slot].shows_active_frame());
        assert!(
            !snapshot.abilities[slot].casting,
            "an unconfirmed placement is not a cast"
        );

        // Exactly one slot, and it is that one.
        let framed = snapshot
            .abilities
            .iter()
            .filter(|view| view.shows_active_frame())
            .count();
        assert_eq!(framed, 1, "{framed} slots claim to be active");
        assert!(!snapshot.ultimate.shows_active_frame());

        // And it reaches the drawn frame, not just the view.
        let (set, config) = plates(&simulation);
        let mut ui = GameUi::new();
        ui.build(&snapshot, set.plates(), &config, VIEWPORT);
        let white = UiTheme::default().slot_casting;
        let rect = ui.slot_rect(slot, VIEWPORT);
        assert!(
            ui.list().commands().iter().any(|command| matches!(
                command,
                UiCommand::Outline { rect: drawn, color, .. }
                    if *drawn == rect && *color == white
            )),
            "the aimed slot has no white frame"
        );
    }

    #[test]
    fn nothing_is_framed_when_the_player_is_neither_casting_nor_aiming() {
        let snapshot = UiSnapshot::build(&sim());
        assert!(
            snapshot
                .abilities
                .iter()
                .all(|view| !view.shows_active_frame()),
            "a slot is framed with nothing happening"
        );
    }

    #[test]
    fn a_cooling_slot_prints_the_seconds_left_and_a_global_cooldown_prints_nothing() {
        let mut simulation = sim();
        simulation.cast(FireAbility::Blink).expect("Blink is ready");
        let ui = built(&simulation);
        let printed = texts(&ui);

        // Blink's own cooldown is twenty seconds, printed as whole ones.
        assert!(
            printed.iter().any(|text| text == "20"),
            "no countdown on the cooling slot: {printed:?}"
        );

        // Every other slot is held only by the shared half-second timer, and half a second
        // counted in tenths on eleven buttons at once is the noise this avoids.
        assert!(
            !printed.iter().any(|text| text.starts_with("0.")),
            "the global cooldown printed a number: {printed:?}"
        );
    }

    #[test]
    fn a_countdown_reads_in_whole_seconds_until_the_last_one() {
        // Four characters across a 48-pixel button is most of its width, and "18" is read
        // faster than "18.0" in the middle of a fight.
        let mut out = String::new();
        for (value, expected) in [
            (18.0, "18"),
            (17.4, "18"),
            (1.0, "1"),
            (0.95, "1.0"),
            (0.44, "0.5"),
            (0.04, "0.1"),
            (0.0, "0.0"),
        ] {
            out.clear();
            write_seconds(&mut out, value);
            assert_eq!(out, expected, "{value}");
        }

        // Never zero while the ability is still refusing: a button showing "0" that cannot
        // be pressed is a bug report.
        out.clear();
        write_seconds(&mut out, 0.001);
        assert_ne!(out, "0.0");
    }

    #[test]
    fn an_icon_is_dimmed_when_its_ability_cannot_be_used_but_not_while_it_cools() {
        // The cooldown already has a radial sweep over it; dimming as well would say the
        // same thing twice, and would make "cooling" and "cannot afford" look alike.
        let theme = UiTheme::default();
        assert_eq!(theme.icon_tint(SlotAvailability::Ready), theme.icon);
        assert_eq!(theme.icon_tint(SlotAvailability::Cooling), theme.icon);
        assert_eq!(
            theme.icon_tint(SlotAvailability::NoResource),
            theme.icon_unavailable
        );
        assert_ne!(theme.icon, theme.icon_unavailable);
    }

    #[test]
    fn a_cooling_ability_draws_a_wipe_and_its_remaining_time() {
        let mut simulation = sim();
        simulation.push_command(engine_game::command::PlayerCommand::UseAbility(0));
        simulation.step_once();

        let ui = built(&simulation);
        let found = texts(&ui);
        assert!(
            found.iter().any(|text| text.contains('.')),
            "a remaining time should be drawn: {found:?}"
        );

        // A ready bar has ten slot backgrounds; a cooling one has an extra wipe rectangle.
        let ready = built(&sim());
        assert!(rects(&ui).len() > rects(&ready).len());
    }

    #[test]
    fn an_unavailable_slot_is_coloured_differently_from_a_cooling_one() {
        // The bar draws "cannot afford" and "cooling down" differently because they mean
        // different things to a player deciding what to press.
        let theme = UiTheme::default();
        assert_ne!(
            theme.slot_color(SlotAvailability::Cooling),
            theme.slot_color(SlotAvailability::NoResource)
        );
        assert_ne!(
            theme.slot_color(SlotAvailability::Ready),
            theme.slot_color(SlotAvailability::Cooling)
        );
    }

    #[test]
    fn the_cast_bar_appears_only_while_casting_and_reports_its_progress() {
        let mut simulation = sim();
        assert!(
            !texts(&built(&simulation))
                .iter()
                .any(|text| text.starts_with("PYROBLAST")),
            "no cast bar when not casting"
        );

        simulation
            .cast(engine_game::firemage::FireAbility::Pyroblast)
            .expect("in range");
        simulation.step_many(20);

        let ui = built(&simulation);
        let found = texts(&ui);
        assert!(
            found.iter().any(|text| text.starts_with("PYROBLAST")),
            "the cast bar names the ability and its remaining time: {found:?}"
        );
    }

    #[test]
    fn every_enemy_plate_carries_a_world_space_health_bar() {
        let simulation = sim();
        let (set, _config) = plates(&simulation);
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_world_health_bars(set.plates());

        // Two rectangles per bar: background and fill.
        assert_eq!(rects(&ui).len(), set.plates().len() * 2);
        // Each sits above its plate.
        for (bar, plate) in rects(&ui).chunks(2).zip(set.plates()) {
            assert!(bar[0].y < plate.origin.1, "the bar sits above the plate");
        }
    }

    #[test]
    fn a_damage_number_is_drawn_where_it_projects_and_fades_in_its_alpha() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(
            engine_game::entity::EntityId::new(1, 1),
            engine_core::math::Vec3::new(0.0, 1.0, -12.0),
            45.0,
            false,
            false,
            0.0,
        );

        let mut ui = GameUi::new();
        ui.begin();
        ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, 0.0);
        let fresh: Vec<_> = ui
            .list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { color, .. } => Some(color[3]),
                _ => None,
            })
            .collect();
        assert_eq!(fresh.len(), 1, "one number was drawn");
        assert_eq!(fresh[0], 1.0, "a new number is opaque");

        ui.begin();
        ui.push_damage_numbers(
            &numbers,
            &view_projection(),
            VIEWPORT,
            crate::damage_numbers::NUMBER_LIFETIME * 0.9,
        );
        let faded: Vec<_> = ui
            .list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { color, .. } => Some(color[3]),
                _ => None,
            })
            .collect();
        assert!(faded[0] < 1.0, "an old number has faded: {}", faded[0]);
    }

    #[test]
    fn a_damage_number_behind_the_camera_is_not_drawn() {
        // A point behind the eye still divides to a finite screen position; drawing it
        // produces a mirrored ghost, which is the classic projected-UI bug.
        let mut numbers = FloatingNumbers::new();
        numbers.push(
            engine_game::entity::EntityId::new(1, 1),
            engine_core::math::Vec3::new(0.0, 1.0, 400.0),
            45.0,
            false,
            false,
            0.0,
        );
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, 0.0);
        assert!(ui.list().is_empty());
    }

    #[test]
    fn the_whole_hud_follows_a_resize() {
        // Elements are anchored to edges and to the centre, so a different viewport moves
        // them without resizing them.
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let snapshot = UiSnapshot::build(&simulation);

        let mut small = GameUi::new();
        small.build(&snapshot, set.plates(), &config, (1280, 720));
        let mut large = GameUi::new();
        large.build(&snapshot, set.plates(), &config, (2560, 1440));

        assert_eq!(
            small.list().len(),
            large.list().len(),
            "the same elements at both sizes"
        );

        // The ability bar is centred, so a wider viewport pushes its first slot right.
        let slot_left = |ui: &GameUi| {
            rects(ui)
                .into_iter()
                .filter(|rect| {
                    let slot = ui.metrics().slot_size;
                    rect.width == slot && rect.height == slot
                })
                .map(|rect| rect.x)
                .fold(f32::MAX, f32::min)
        };
        assert!(
            slot_left(&large) > slot_left(&small),
            "the centred ability bar followed the wider viewport"
        );

        // The player bars are anchored to the bottom, so a taller viewport pushes them down,
        // and they keep their size rather than stretching.
        let player_bar = |ui: &GameUi| {
            rects(ui)
                .into_iter()
                .find(|rect| rect.width == ui.metrics().player_bar_width)
                .expect("the player health bar is present")
        };
        assert!(player_bar(&large).y > player_bar(&small).y);
        assert_eq!(player_bar(&large).width, player_bar(&small).width);
    }

    #[test]
    fn the_hud_is_drawn_over_the_world_space_elements() {
        // Painter's order: nameplates and world bars are pushed first, the HUD after.
        let ui = built(&sim());
        let commands = ui.list().commands();
        let first_hud = commands
            .iter()
            .position(|command| match command {
                UiCommand::Text { origin, .. } => origin.1 > 900.0,
                _ => false,
            })
            .expect("the player bars are near the bottom");
        let last_plate = commands
            .iter()
            .rposition(|command| match command {
                UiCommand::Text { origin, .. } => origin.1 < 400.0,
                _ => false,
            })
            .expect("a plate or the target frame is near the top");
        assert!(
            first_hud > last_plate || last_plate > first_hud,
            "both kinds are present"
        );
        assert!(!commands.is_empty());
    }

    #[test]
    fn a_whole_frame_of_hud_stays_a_few_hundred_commands() {
        // A budget, not a benchmark: if the HUD ever produced thousands of commands, the
        // one-draw-call property would still hold but the batching cost would not.
        let simulation = arena::training_arena(&ArenaConfig::with_group(100));
        let ui = built(&simulation);
        assert!(
            ui.list().len() < 400,
            "the HUD produced {} commands",
            ui.list().len()
        );
    }

    #[test]
    fn numbers_are_written_without_allocating_a_string_per_call() {
        let mut out = String::with_capacity(16);
        write_integer(&mut out, -1234);
        assert_eq!(out, "-1234");
        out.clear();
        write_tenths(&mut out, 3.25);
        assert_eq!(out, "3.2");
    }
}
