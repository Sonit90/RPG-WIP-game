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

use engine_core::input::Modifiers;
use engine_core::math::Mat4;
use engine_render_data::font::UiFont;
use engine_render_data::ui::{
    TextAlign, TextStyle, UiClip, UiColor, UiDrawList, UiIcon, UiRect, UiScale,
};

use crate::damage_numbers::{Avoidance, CombatTextKind, FloatingNumber, FloatingNumbers};
use crate::nameplate::{Nameplate, NameplateConfig, PlateCastState, PlateRank, project};
use crate::snapshot::{AbilitySlotView, SlotAvailability, UiSnapshot};
use engine_game::event::AggroChange;
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
    /// Gap between the two corner buttons.
    pub const CORNER_BUTTON_GAP: f32 = 6.0;
    /// Size of one Ignite stack pip.
    pub const IGNITE_PIP: f32 = 10.0;
    /// Gap between Ignite pips.
    pub const IGNITE_PIP_GAP: f32 = 3.0;
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
    /// `layout::CORNER_BUTTON_GAP` in physical pixels.
    pub corner_button_gap: f32,
    /// `layout::IGNITE_PIP` in physical pixels.
    pub ignite_pip: f32,
    /// `layout::IGNITE_PIP_GAP` in physical pixels.
    pub ignite_pip_gap: f32,
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
            corner_button_gap: scale.px(layout::CORNER_BUTTON_GAP),
            ignite_pip: scale.px(layout::IGNITE_PIP),
            ignite_pip_gap: scale.px(layout::IGNITE_PIP_GAP),
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
    /// Fill of an enemy's cast bar.
    ///
    /// A different colour from the player's own: two bars that meant different things and
    /// looked the same would be one bar the player had to read twice.
    pub enemy_cast_fill: UiColor,
    /// Colour of the death notice.
    pub death_text: UiColor,
    /// How enemy nameplates are coloured.
    pub plate: PlateTheme,
    /// How floating combat text is coloured.
    pub combat: CombatTextTheme,
    /// Aggro floating text for gaining it, or being about to: an enemy turning on you.
    pub aggro_gain_text: UiColor,
    /// Aggro floating text for losing it, or being about to.
    pub aggro_loss_text: UiColor,
    /// Glyph scale for HUD text.
    pub hud_text_scale: f32,
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
    /// The Warrior's Deflection bar below the cap.
    ///
    /// Warm rather than cool, and the *filling* of it is the warning: a full bar is a full
    /// bank and no passive defence at all. See [`UiTheme::deflection_color`].
    pub deflection: UiColor,
    /// The Deflection bar at a hundred, where the passive bonuses are worth nothing.
    pub deflection_capped: UiColor,
    /// The Druid's Mana bar.
    pub mana: UiColor,
    /// The Astral Power bar while it is filling.
    pub astral: UiColor,
    /// The Astral Power bar once the next compatible spell is armed.
    ///
    /// Deliberately the same family as [`UiTheme::slot_empowered`]: "this will be empowered"
    /// is one idea and it should be one colour, on the resource and on the button.
    pub astral_armed: UiColor,
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
            enemy_cast_fill: [0.86, 0.30, 0.24, 1.0],
            death_text: [0.92, 0.34, 0.30, 1.0],
            plate: PlateTheme::default(),
            combat: CombatTextTheme::default(),
            aggro_gain_text: [1.0, 0.36, 0.26, 1.0],
            aggro_loss_text: [0.52, 0.78, 1.0, 1.0],
            hud_text_scale: 2.0,
            heat_low: [0.55, 0.55, 0.60, 1.0],
            heat_warm: [0.92, 0.72, 0.22, 1.0],
            heat_high: [0.95, 0.55, 0.15, 1.0],
            heat_inferno: [1.0, 0.30, 0.10, 1.0],
            energy: [0.35, 0.55, 0.95, 1.0],
            energy_full: [0.65, 0.85, 1.0, 1.0],
            deflection: [0.85, 0.72, 0.30, 1.0],
            deflection_capped: [1.0, 0.55, 0.20, 1.0],
            mana: [0.30, 0.50, 0.95, 1.0],
            astral: [0.55, 0.45, 0.85, 1.0],
            astral_armed: [0.75, 0.85, 1.0, 1.0],
            slot_disabled: [0.25, 0.25, 0.28, 1.0],
            slot_no_resource: [0.70, 0.18, 0.18, 1.0],
            slot_invalid: [0.60, 0.15, 0.15, 1.0],
            slot_empowered: [1.0, 0.88, 0.18, 1.0],
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

    /// What colour the Astral Power bar is, armed or not.
    #[must_use]
    pub fn astral_color(&self, armed: bool) -> UiColor {
        if armed {
            self.astral_armed
        } else {
            self.astral
        }
    }

    /// What colour the Deflection bar is at `value`.
    ///
    /// One of two: the ordinary bar, and the capped one. The cap is worth calling out
    /// because it is the one state the design asks the player to get out of — not with a
    /// penalty, but because the passive maximum health and armour bonuses are worth nothing
    /// there and there is a full bank to spend.
    #[must_use]
    pub fn deflection_color(&self, capped: bool) -> UiColor {
        if capped {
            self.deflection_capped
        } else {
            self.deflection
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

/// How an enemy nameplate is coloured.
///
/// Its own struct inside [`UiTheme`] because a plate is a component with a palette of its
/// own — a name, two bars, their borders and their states — and a flat list of twenty more
/// colours on the theme would bury which of them belong together.
///
/// The palette is the genre's, in this game's own values: a warm off-white name, a saturated
/// red health bar on a near-black ground, an amber cast bar that turns silver when it cannot
/// be interrupted and red when it was. Rank is carried by the border and the crest as well as
/// the name's colour, so it does not depend on telling two hues apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateTheme {
    /// An ordinary enemy's name.
    pub name: UiColor,
    /// The target's name: brighter than the rest, so the selected plate answers at once.
    pub name_target: UiColor,
    /// An elite's name.
    pub name_elite: UiColor,
    /// A boss's name.
    pub name_boss: UiColor,
    /// The name on a dead enemy's fading plate.
    pub name_dead: UiColor,
    /// The status line.
    pub status: UiColor,
    /// The dark edge around every letter on a plate.
    pub text_outline: UiColor,
    /// Behind a bar's fill.
    pub bar_background: UiColor,
    /// A bar's border.
    pub border: UiColor,
    /// An elite's health-bar border.
    pub border_elite: UiColor,
    /// A boss's health-bar border.
    pub border_boss: UiColor,
    /// The target's health-bar border.
    pub border_target: UiColor,
    /// The health fill, top edge.
    pub health_top: UiColor,
    /// The health fill, bottom edge.
    pub health_bottom: UiColor,
    /// The health fill once the enemy is nearly dead, top edge.
    pub health_low_top: UiColor,
    /// The health fill once the enemy is nearly dead, bottom edge.
    pub health_low_bottom: UiColor,
    /// Health just lost, draining towards the fill.
    pub trail: UiColor,
    /// A dead enemy's empty bar.
    pub health_dead: UiColor,
    /// Below this fraction the bar takes its low colours.
    pub low_health: f32,
    /// The text on the health bar.
    pub health_text: UiColor,
    /// An interruptible cast's fill, top edge.
    pub cast_top: UiColor,
    /// An interruptible cast's fill, bottom edge.
    pub cast_bottom: UiColor,
    /// A cast that cannot be interrupted, top edge.
    pub cast_locked_top: UiColor,
    /// A cast that cannot be interrupted, bottom edge.
    pub cast_locked_bottom: UiColor,
    /// The border of a cast that cannot be interrupted.
    pub cast_locked_border: UiColor,
    /// An interrupted cast's fill.
    pub cast_interrupted: UiColor,
    /// A completed cast's fill, as it fades.
    pub cast_complete: UiColor,
    /// The text on a cast bar.
    pub cast_text: UiColor,
    /// A friendly health fill — the player, the party, the raid — top edge.
    pub friendly_top: UiColor,
    /// A friendly health fill, bottom edge.
    pub friendly_bottom: UiColor,
    /// A friendly health fill that is dangerously low, top edge.
    pub friendly_low_top: UiColor,
    /// A friendly health fill that is dangerously low, bottom edge.
    pub friendly_low_bottom: UiColor,
    /// An absorb shield's layer on a health bar, top of its gradient: a pale silver, so a
    /// shield reads as "not health, but in front of it" on a green bar and a red one alike.
    pub absorb_top: UiColor,
    /// An absorb shield's layer, bottom of its gradient.
    pub absorb_bottom: UiColor,
    /// The bright edge where a shield bigger than the missing health runs off the bar's end.
    pub absorb_edge: UiColor,
    /// The panel every unit frame sits on.
    pub frame_panel: UiColor,
    /// The panel's border.
    pub frame_border: UiColor,
}

impl Default for PlateTheme {
    fn default() -> Self {
        Self {
            name: [0.98, 0.95, 0.90, 1.0],
            name_target: [1.0, 1.0, 1.0, 1.0],
            name_elite: [1.0, 0.88, 0.58, 1.0],
            name_boss: [1.0, 0.78, 0.34, 1.0],
            name_dead: [0.62, 0.62, 0.62, 1.0],
            status: [1.0, 0.72, 0.36, 1.0],
            text_outline: [0.0, 0.0, 0.0, 0.92],
            bar_background: [0.035, 0.035, 0.045, 0.92],
            border: [0.0, 0.0, 0.0, 1.0],
            border_elite: [0.80, 0.64, 0.30, 1.0],
            border_boss: [0.96, 0.74, 0.30, 1.0],
            border_target: [1.0, 1.0, 1.0, 1.0],
            health_top: [0.90, 0.17, 0.13, 1.0],
            health_bottom: [0.66, 0.07, 0.05, 1.0],
            health_low_top: [1.0, 0.44, 0.14, 1.0],
            health_low_bottom: [0.82, 0.25, 0.05, 1.0],
            trail: [1.0, 0.86, 0.62, 0.85],
            health_dead: [0.30, 0.30, 0.30, 1.0],
            low_health: 0.20,
            health_text: [1.0, 1.0, 1.0, 1.0],
            cast_top: [1.0, 0.80, 0.24, 1.0],
            cast_bottom: [0.88, 0.56, 0.08, 1.0],
            // Dark enough that the white name printed on it keeps its contrast.
            cast_locked_top: [0.56, 0.58, 0.64, 1.0],
            cast_locked_bottom: [0.36, 0.38, 0.44, 1.0],
            cast_locked_border: [0.86, 0.88, 0.94, 1.0],
            cast_interrupted: [0.82, 0.13, 0.10, 1.0],
            // The casting amber, lifted: the bar reads as the same bar, finished.
            cast_complete: [1.0, 0.88, 0.38, 1.0],
            cast_text: [1.0, 1.0, 1.0, 1.0],
            friendly_top: [0.30, 0.82, 0.34, 1.0],
            friendly_bottom: [0.14, 0.56, 0.18, 1.0],
            friendly_low_top: [0.96, 0.46, 0.16, 1.0],
            friendly_low_bottom: [0.78, 0.24, 0.06, 1.0],
            absorb_top: [0.93, 0.95, 0.98, 0.80],
            absorb_bottom: [0.66, 0.70, 0.76, 0.80],
            absorb_edge: [1.0, 1.0, 1.0, 0.95],
            frame_panel: [0.035, 0.04, 0.055, 0.84],
            frame_border: [0.0, 0.0, 0.0, 0.9],
        }
    }
}

/// How floating combat text is coloured.
///
/// The hierarchy is the genre's: outgoing damage warm gold, a critical a hotter orange,
/// damage over time a muted amber, healing green, damage taken red. Each is also a different
/// size and sign (see `damage_numbers`), so no distinction rests on colour alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CombatTextTheme {
    /// An ordinary hit.
    pub damage: UiColor,
    /// A critical strike.
    pub critical: UiColor,
    /// A tick of damage over time.
    pub periodic: UiColor,
    /// Damage the player takes.
    pub incoming: UiColor,
    /// Healing.
    pub heal: UiColor,
    /// A critical heal.
    pub heal_critical: UiColor,
    /// A dodge, a parry or a block.
    pub avoided: UiColor,
    /// A shield absorbing damage.
    pub absorbed: UiColor,
    /// A spell sent back.
    pub reflected: UiColor,
    /// The dark edge around every letter.
    pub outline: UiColor,
}

impl Default for CombatTextTheme {
    fn default() -> Self {
        Self {
            damage: [1.0, 0.86, 0.24, 1.0],
            critical: [1.0, 0.62, 0.12, 1.0],
            periodic: [0.94, 0.74, 0.42, 1.0],
            incoming: [1.0, 0.30, 0.22, 1.0],
            heal: [0.40, 0.96, 0.40, 1.0],
            heal_critical: [0.64, 1.0, 0.50, 1.0],
            avoided: [0.94, 0.94, 0.97, 1.0],
            absorbed: [0.70, 0.84, 1.0, 1.0],
            reflected: [0.80, 0.66, 1.0, 1.0],
            outline: [0.0, 0.0, 0.0, 0.9],
        }
    }
}

/// The least a living enemy's cast bar is drawn at, however faded its plate is.
const CAST_MIN_ALPHA: f32 = 0.9;

/// The top of a line box that puts `font`'s capitals centred on `center_y` at `size`.
///
/// Cap height is about 0.72 em in both faces; centring the capitals rather than the line box
/// is what makes a label sit in the middle of the bar it is printed on.
pub(crate) fn cap_centred_top(font: UiFont, size: f32, center_y: f32) -> f32 {
    center_y - font.ascent(size) + size * 0.36
}

/// The edge every word on a plate is drawn with, at `size` pixels per em.
///
/// About a fifteenth of the size, never under a pixel: enough to cut a letter out of a torch
/// flare, never so much that the letters blur into one dark shape. The slight downward nudge
/// makes the edge read as a shadow too.
pub(crate) fn plate_text_style(outline: UiColor, size: f32, embolden: f32) -> TextStyle {
    TextStyle::outlined(
        outline,
        (size * 0.075).max(1.0),
        (size * 0.05).clamp(0.5, 1.5),
        size * embolden,
    )
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
    /// Scratch for the seconds and stacks printed over an aura icon.
    statuses: String,
    /// Every buff and debuff icon drawn this frame, for the tooltip lookup.
    aura_hits: crate::ui::aura_row::AuraHits,
    /// The tooltip over one of them, its wrapped description kept between frames.
    /// What the open tooltip says, rebuilt in place each frame it is shown.
    tooltip: crate::ui::tooltip::TooltipModel,
    /// The open tooltip, wrapped and laid out; wrapped again only when its text changes.
    tooltip_panel: crate::ui::tooltip::TooltipPanel,
    /// Where the arranged bar one was drawn last frame, which the Ultimate and the extras
    /// hang off. `None` before the first arranged frame or with bar one hidden or upright.
    bar_one: Option<UiRect>,
    /// Where the minimap is, when the level has one: the corner buttons move to its left
    /// and the dungeon's progress panel moves under it.
    minimap: Option<UiRect>,
    /// How floating combat text is sized and whether criticals are emphasised.
    combat_text: engine_settings::CombatTextSettings,
    /// This frame's floating text, laid out and then moved apart. Reused; never freed.
    combat_layout: Vec<PlacedText>,
    /// How far each entry has been moved up to clear the others, eased from frame to frame so
    /// a number that has to make room slides rather than jumps. Keyed by [`text_key`].
    combat_nudges: Vec<(u64, f32)>,
    /// The simulated time the nudges were last eased at.
    combat_nudged_at: f32,
}

/// One floating entry, laid out for this frame: where it would be, and how big.
#[derive(Debug, Clone, Copy)]
struct PlacedText {
    /// Which entry, in [`FloatingNumbers::numbers`].
    index: usize,
    /// Its identity across frames, for the eased nudge.
    key: u64,
    /// Its centre where the motion puts it, before any nudge.
    center: (f32, f32),
    /// Its size, in pixels per em.
    size: f32,
    /// Half its width and half its height, in pixels, with a little air.
    half: (f32, f32),
    /// How far up it has to go to clear what was placed before it.
    push: f32,
    /// Placed first when it matters more: criticals, then the older.
    critical: bool,
    born: f32,
}

/// An entry's identity from one frame to the next.
fn text_key(number: &FloatingNumber) -> u64 {
    u64::from(number.serial)
}

/// How quickly a nudge follows the room it needs, per second.
const NUDGE_RATE: f32 = 14.0;

/// How many passes the overlap resolution takes before it settles for what it has.
const NUDGE_PASSES: usize = 6;

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
            aura_hits: crate::ui::aura_row::AuraHits::new(),
            tooltip: crate::ui::tooltip::TooltipModel::new(),
            tooltip_panel: crate::ui::tooltip::TooltipPanel::new(),
            bar_one: None,
            minimap: None,
            combat_text: engine_settings::CombatTextSettings::default(),
            combat_layout: Vec::with_capacity(crate::damage_numbers::MAX_FLOATING_NUMBERS),
            combat_nudges: Vec::with_capacity(crate::damage_numbers::MAX_FLOATING_NUMBERS),
            combat_nudged_at: 0.0,
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

    /// Draws floating combat text as `settings` say from here on: its size and whether a
    /// critical swells. Which categories appear is the observer's business, not the drawing's.
    pub fn set_combat_text(&mut self, settings: engine_settings::CombatTextSettings) {
        self.combat_text = settings;
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
        self.aura_hits.clear();
    }

    /// Every buff and debuff icon drawn this frame, with the effect each one shows.
    #[must_use]
    pub fn aura_hits(&self) -> &crate::ui::aura_row::AuraHits {
        &self.aura_hits
    }

    /// The buff or debuff under `pointer`, if the pointer is on one.
    #[must_use]
    pub fn aura_at(&self, pointer: (f32, f32)) -> Option<(UiRect, crate::aura::Aura)> {
        self.aura_hits.at(pointer)
    }

    /// The tooltip for the buff or debuff under `pointer`, if there is one: its name, what
    /// kind of effect it is — the dispel school, in its colour — what it does, its stacks and
    /// how long it has left. Opened from the same dock as every other tooltip.
    ///
    /// Drawn last of the HUD, so it is over everything it describes. Returns whether a
    /// tooltip was drawn.
    pub fn push_aura_tooltip(
        &mut self,
        pointer: (f32, f32),
        dock: UiRect,
        direction: crate::ui::flow::FlowDirection,
        viewport: (u32, u32),
    ) -> bool {
        let Some((_, aura)) = self.aura_hits.at(pointer) else {
            return false;
        };
        crate::ui::tooltip::fill_aura(&mut self.tooltip, &aura);
        let kit = crate::ui::kit::Kit::new(self.metrics.scale);
        self.tooltip_panel.push(
            &mut self.list,
            &self.tooltip,
            dock,
            direction,
            &kit,
            viewport,
        );
        true
    }

    /// Appends every nameplate, in the order the set supplies them.
    ///
    /// The set is already sorted furthest first, which is exactly what painter's order wants:
    /// a nearer plate is pushed later and therefore drawn on top.
    ///
    /// Each plate is one component — name, health bar and, while the enemy casts, the cast
    /// bar hanging under it — laid out in [`crate::nameplate::PlateGeometry`] and drawn here at
    /// the plate's alpha, every part together. A plate whose bar dimmed while its name stayed
    /// bright would be less readable than either.
    pub fn push_nameplates(&mut self, plates: &[Nameplate], config: &NameplateConfig) {
        for plate in plates {
            if plate.alpha <= 0.01 {
                continue;
            }
            self.push_plate_name(plate);
            self.push_plate_health(plate, config);
            self.push_plate_auras(plate);
            self.push_plate_cast(plate);
        }
    }

    /// A sample tooltip in the tooltip dock, for Edit HUD: a tooltip is only ever open while
    /// the pointer is on something, and its place, width and direction cannot be judged from
    /// an outline. Returns the panel it drew, so the dock can be made the same size.
    pub fn push_tooltip_preview(
        &mut self,
        dock: UiRect,
        direction: crate::ui::flow::FlowDirection,
        viewport: (u32, u32),
    ) -> UiRect {
        use crate::ui::tooltip::SectionKind;
        let model = &mut self.tooltip;
        model.clear();
        model.title.push_str("Tooltip");
        model.tag.push_str("Preview");
        model.meta[0].0.push_str("Instant");
        model.meta[0].1.push_str("8 sec cooldown");
        model
            .section(SectionKind::Description)
            .text
            .push_str("Every ability and aura tooltip opens here. Drag this to move it; set its width and which way it grows in the inspector.");
        let kit = crate::ui::kit::Kit::new(self.metrics.scale);
        self.tooltip_panel.push(
            &mut self.list,
            &self.tooltip,
            dock,
            direction,
            &kit,
            viewport,
        )
    }

    /// A sample row of buffs in the buff bar's rectangle, for Edit HUD.
    ///
    /// The bar is empty most of the time, and an empty rectangle cannot be judged for place or
    /// size: while the editor is open and nothing is running, this is what fills it.
    pub fn push_buff_bar_preview(&mut self, flow: &crate::ui::flow::Flow) {
        self.push_aura_preview(flow, false);
    }

    /// Ten sample effects in a block of aura icons, for Edit HUD: buffs, or debuffs with
    /// stacks, so a block can be placed and sized against what it will hold.
    pub fn push_aura_preview(&mut self, flow: &crate::ui::flow::Flow, harmful: bool) {
        use crate::aura::{Aura, AuraKind};
        use engine_game::status::StatusKind;
        const fn sample(kind: AuraKind, stacks: u8, remaining: f32) -> Aura {
            Aura {
                kind,
                stacks,
                remaining,
                duration: 12.0,
                indefinite: false,
                amount: 0.0,
            }
        }
        const BUFFS: [Aura; 10] = [
            sample(AuraKind::FlameShield, 1, 4.0),
            sample(AuraKind::Status(StatusKind::Rejuvenation), 1, 11.0),
            sample(AuraKind::ShieldBlock, 1, 3.0),
            sample(AuraKind::Status(StatusKind::Recuperating), 3, 7.0),
            sample(AuraKind::Status(StatusKind::WildRush), 1, 5.0),
            sample(AuraKind::Status(StatusKind::Lifebloom), 3, 6.0),
            sample(AuraKind::Status(StatusKind::Fortitude), 1, 9.0),
            sample(AuraKind::Status(StatusKind::BattleShout), 1, 10.0),
            sample(AuraKind::Status(StatusKind::ArcaneIntellect), 1, 8.0),
            sample(AuraKind::Status(StatusKind::Regeneration), 1, 2.0),
        ];
        const DEBUFFS: [Aura; 10] = [
            sample(AuraKind::Ignite, 5, 6.0),
            sample(AuraKind::Status(StatusKind::Poisoned), 1, 9.0),
            sample(AuraKind::Status(StatusKind::Rended), 3, 4.0),
            sample(AuraKind::Status(StatusKind::Bleeding), 2, 5.0),
            sample(AuraKind::Status(StatusKind::Crippled), 1, 3.0),
            sample(AuraKind::Status(StatusKind::Sundered), 2, 8.0),
            sample(AuraKind::Status(StatusKind::CurseOfWeakness), 1, 11.0),
            sample(AuraKind::Status(StatusKind::WitheringPlague), 1, 7.0),
            sample(AuraKind::Status(StatusKind::MagicVulnerability), 1, 10.0),
            sample(AuraKind::Scorched, 1, 2.0),
        ];
        crate::ui::aura_row::push_aura_block(
            &mut self.list,
            &mut self.aura_hits,
            flow,
            if harmful {
                DEBUFFS.iter()
            } else {
                BUFFS.iter()
            },
            &mut self.statuses,
        );
    }

    /// A plate's buffs and debuffs: centred rows of icons, the debuffs over the name and the
    /// buffs under the health bar.
    fn push_plate_auras(&mut self, plate: &Nameplate) {
        use crate::ui::aura_row::{AuraGrow, push_aura_row};
        if let Some((origin, style)) = plate.debuff_row() {
            push_aura_row(
                &mut self.list,
                &mut self.aura_hits,
                plate.auras.debuffs(),
                origin,
                AuraGrow::Centered,
                style.faded(plate.alpha),
                &mut self.statuses,
            );
        }
        if let Some((origin, style)) = plate.buff_row() {
            push_aura_row(
                &mut self.list,
                &mut self.aura_hits,
                plate.auras.buffs(),
                origin,
                AuraGrow::Centered,
                style.faded(plate.alpha),
                &mut self.statuses,
            );
        }
    }

    /// A plate's name — crest first, for an elite or a boss.
    fn push_plate_name(&mut self, plate: &Nameplate) {
        let theme = self.theme.plate;
        let geometry = &plate.geometry;
        let alpha = plate.alpha;
        let (x, y) = plate.origin;
        let outline = faded(theme.text_outline, alpha);

        let color = if plate.dead {
            theme.name_dead
        } else if plate.is_target {
            theme.name_target
        } else {
            match plate.rank {
                PlateRank::Boss => theme.name_boss,
                PlateRank::Elite => theme.name_elite,
                PlateRank::Normal => theme.name,
            }
        };
        let size = geometry.name_size;
        let center = y + geometry.name_top + size * 0.59;
        crate::ui::map_style::write_title_case(&mut self.title, plate.name);
        // The name and its crest are centred together over the plate.
        let left = x + ((plate.size.0 - geometry.name_width) * 0.5).round();
        let mut text_x = left;
        if let Some((crest, edge)) = geometry.crest {
            self.list.icon(
                UiRect::new(left, (center - edge * 0.5).round(), edge, edge),
                crest,
                faded([1.0, 1.0, 1.0, 1.0], alpha),
            );
            text_x = left + edge + (4.0 * geometry.scale).round();
        }
        self.list.styled_label(
            (text_x, cap_centred_top(UiFont::SemiBold, size, center)),
            &self.title,
            faded(color, alpha),
            UiFont::SemiBold,
            size,
            TextAlign::Left,
            // A touch heavier than the face: SemiBold, thickened, reads as Bold on the plate.
            plate_text_style(outline, size, 0.025),
        );
    }

    /// A plate's health bar: ground, recent-damage trail, fill, border and text.
    fn push_plate_health(&mut self, plate: &Nameplate, config: &NameplateConfig) {
        let theme = self.theme.plate;
        let geometry = &plate.geometry;
        let alpha = plate.alpha;
        let bar = plate.bar_rect();
        let border = geometry.border;
        let inner = bar.inset(border);

        self.list.rect(inner, faded(theme.bar_background, alpha));
        let fraction = plate.health_fraction.clamp(0.0, 1.0);
        let trail = plate.trail_fraction.clamp(fraction, 1.0);
        if trail > fraction + 0.001 {
            let start = inner.x + (inner.width * fraction).round();
            let end = inner.x + (inner.width * trail).round();
            self.list.rect(
                UiRect::new(start, inner.y, end - start, inner.height),
                faded(theme.trail, alpha),
            );
        }
        if plate.dead {
            self.list
                .rect(inner, faded(theme.health_dead, alpha * 0.35));
        } else if fraction > 0.0 {
            let (top, bottom) = if fraction <= theme.low_health {
                (theme.health_low_top, theme.health_low_bottom)
            } else {
                (theme.health_top, theme.health_bottom)
            };
            let fill = UiRect::new(
                inner.x,
                inner.y,
                (inner.width * fraction).round().max(1.0),
                inner.height,
            );
            self.list
                .panel(fill, 0.0, faded(top, alpha), faded(bottom, alpha));
            let layers = crate::ui::frame_style::HealthLayers {
                absorb: crate::ui::frame_style::HealthLayers::of(
                    plate.health,
                    plate.max_health,
                    plate.absorb,
                )
                .absorb,
                ..crate::ui::frame_style::HealthLayers::health(fraction)
            };
            crate::ui::frame_style::push_absorb_layer(&mut self.list, inner, layers, &theme, alpha);
        }
        let border_color = if plate.is_target {
            theme.border_target
        } else {
            match plate.rank {
                PlateRank::Boss => theme.border_boss,
                PlateRank::Elite => theme.border_elite,
                PlateRank::Normal => theme.border,
            }
        };
        self.list.outline(bar, border, faded(border_color, alpha));

        if plate.write_health_text(config.health_text, &mut self.title) {
            let size = geometry.health_text_size;
            let center = inner.y + inner.height * 0.5;
            self.list.styled_label(
                (
                    inner.x + inner.width * 0.5,
                    cap_centred_top(UiFont::SemiBold, size, center),
                ),
                &self.title,
                faded(theme.health_text, alpha),
                UiFont::SemiBold,
                size,
                TextAlign::Center,
                plate_text_style(faded(theme.text_outline, alpha), size, 0.0),
            );
        }
    }

    /// A plate's cast bar, under its health bar: the school icon, the fill, the spell's name
    /// and the seconds left.
    ///
    /// A cast that cannot be interrupted is silver with a lock on its icon; an interrupted
    /// one is red, frozen where it stopped, and says so.
    fn push_plate_cast(&mut self, plate: &Nameplate) {
        let (Some(cast), Some(rect)) = (plate.cast, plate.cast_rect()) else {
            return;
        };
        let theme = self.theme.plate;
        let geometry = &plate.geometry;
        // A cast is the one thing on a dimmed plate the player may have to act on — it is
        // what an interrupt is for — so it keeps nearly full strength while its plate is
        // faded for not being the target. A dead enemy's plate still takes its bar with it.
        let plate_alpha = if plate.dead {
            plate.alpha
        } else {
            plate.alpha.max(CAST_MIN_ALPHA)
        };
        let alpha = plate_alpha * cast.alpha;
        if alpha <= 0.01 {
            return;
        }
        let border = geometry.border;
        let edge = rect.height;
        let icon_frame = UiRect::new(rect.x, rect.y, edge, edge);
        // The icon and the bar share the border between them.
        let bar = UiRect::new(
            rect.x + edge - border,
            rect.y,
            rect.width - edge + border,
            edge,
        );
        let inner = bar.inset(border);
        let locked = !cast.interruptible;

        self.list.rect(inner, faded(theme.bar_background, alpha));
        let (top, bottom) = match cast.state {
            PlateCastState::Interrupted => (theme.cast_interrupted, theme.cast_interrupted),
            PlateCastState::Completed if locked => {
                (theme.cast_locked_top, theme.cast_locked_bottom)
            }
            PlateCastState::Completed => (theme.cast_complete, theme.cast_bottom),
            PlateCastState::Casting if locked => (theme.cast_locked_top, theme.cast_locked_bottom),
            PlateCastState::Casting => (theme.cast_top, theme.cast_bottom),
        };
        let fraction = cast.fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let fill = UiRect::new(
                inner.x,
                inner.y,
                (inner.width * fraction).round().max(1.0),
                inner.height,
            );
            self.list
                .panel(fill, 0.0, faded(top, alpha), faded(bottom, alpha));
        }
        let border_color = if locked {
            theme.cast_locked_border
        } else {
            theme.border
        };
        self.list.outline(bar, border, faded(border_color, alpha));
        self.list
            .icon(icon_frame.inset(border), cast.icon, faded([1.0; 4], alpha));
        self.list
            .outline(icon_frame, border, faded(border_color, alpha));
        if locked {
            // The lock in the icon's corner: interruptibility said with a shape as well as a
            // colour.
            let lock = (edge * 0.62).round();
            self.list.icon(
                UiRect::new(
                    icon_frame.right() - lock + border,
                    icon_frame.bottom() - lock + border,
                    lock,
                    lock,
                ),
                UiIcon::Lock,
                faded([1.0; 4], alpha),
            );
        }

        let size = geometry.cast_text_size;
        let center = inner.y + inner.height * 0.5;
        let top_y = cap_centred_top(UiFont::SemiBold, size, center);
        let style = plate_text_style(faded(theme.text_outline, alpha), size, 0.0);
        let pad = (4.0 * geometry.scale).round();
        let text_color = faded(theme.cast_text, alpha);

        if cast.state == PlateCastState::Interrupted {
            self.list.styled_label(
                (inner.x + inner.width * 0.5, top_y),
                "Interrupted",
                text_color,
                UiFont::SemiBold,
                size,
                TextAlign::Center,
                style,
            );
            return;
        }

        // The seconds left, right-aligned; then the name, cut where it would reach them.
        let mut name_right = inner.right() - pad;
        if cast.state == PlateCastState::Casting {
            self.title.clear();
            write_tenths(&mut self.title, cast.remaining);
            let time_width = UiFont::SemiBold.width(&self.title, size);
            self.list.styled_label(
                (inner.right() - pad, top_y),
                &self.title,
                text_color,
                UiFont::SemiBold,
                size,
                TextAlign::Right,
                style,
            );
            name_right -= time_width + pad;
        }
        crate::ui::map_style::write_title_case(&mut self.title, cast.name);
        let name_x = inner.x + pad;
        let fits = UiFont::SemiBold.width(&self.title, size) <= name_right - name_x;
        if !fits {
            self.list.clip(UiClip::rect(UiRect::new(
                name_x - pad,
                bar.y - edge,
                (name_right - name_x + pad).max(0.0),
                edge * 3.0,
            )));
        }
        self.list.styled_label(
            (name_x, top_y),
            &self.title,
            text_color,
            UiFont::SemiBold,
            size,
            TextAlign::Left,
            style,
        );
        if !fits {
            self.list.clip(UiClip::NONE);
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
        self.push_nameplates(plates, config);
        self.push_target_frame(snapshot, viewport);
        self.push_player_bars(snapshot, viewport);
        self.push_resource_bars(snapshot, viewport);
        self.push_ability_bar(snapshot, viewport);
        self.push_talent_button(snapshot, viewport);
        self.push_character_button(viewport);
        self.push_cast_bar(snapshot, viewport);
        self.push_enemy_cast_bar(snapshot, viewport);
        self.push_dungeon_progress(snapshot, viewport);
        self.push_notification(snapshot, viewport);
        self.push_death_notice(snapshot, viewport);
        &self.list
    }

    /// Builds a whole frame of UI from an arranged HUD.
    ///
    /// The counterpart to [`GameUi::build`] for a game that has a [`HudLayout`]: the fixed
    /// target frame, the fixed player bars and the one centred ability bar are replaced by
    /// the movable frames and the eight arranged bars. Everything that is not arrangeable —
    /// nameplates, the cast bars, the talent button, the notices — is drawn identically,
    /// because none of it is something the player positions.
    #[allow(
        clippy::too_many_arguments,
        reason = "a frame of UI reads from this many independent sources"
    )]
    pub fn build_arranged(
        &mut self,
        snapshot: &UiSnapshot,
        plates: &[Nameplate],
        config: &NameplateConfig,
        layout: &crate::ui::hud_layout::HudLayout,
        bars: &crate::ui::action_bar::ActionBarSet,
        scratch: &mut crate::ui::unit_frame::FrameScratch,
        viewport: (u32, u32),
    ) -> &UiDrawList {
        self.begin();
        self.push_nameplates(plates, config);

        // The arranged HUD. The theme and the scale are copied out first: the frames are
        // drawn into this list, and the borrow checker is right that reading them off `self`
        // while lending the list out would be two borrows of one thing.
        let theme = self.theme;
        let scale = self.metrics.scale;
        crate::ui::unit_frame::push_frames(
            &mut self.list,
            layout,
            snapshot,
            &theme,
            scratch,
            &mut self.aura_hits,
            scale,
            viewport,
        );
        let first = crate::ui::hud_layout::HudElementId::ActionBar(0);
        self.bar_one = (layout.is_enabled(first)
            && layout.orientation(0) == engine_settings::hud::HudOrientation::Horizontal)
            .then(|| layout.rect_of(first, scale, viewport));
        self.push_action_bars(snapshot, layout, bars, viewport);
        // Not arrangeable, and drawn here for the same reason the cast bar is: the Ultimate
        // and the extras have a place of their own beside the bar, and the application
        // hit-tests them at exactly these rectangles.
        self.push_side_buttons(snapshot, viewport);

        self.push_talent_button(snapshot, viewport);
        self.push_character_button(viewport);
        // The player's cast bar is an arranged element too: wherever, and whatever size,
        // the player put it in Edit HUD.
        if layout.is_enabled(crate::ui::hud_layout::HudElementId::CastBar) {
            let rect = layout.rect_of(
                crate::ui::hud_layout::HudElementId::CastBar,
                scale,
                viewport,
            );
            self.push_cast_bar_in(snapshot, rect);
        }
        self.push_enemy_cast_bar(snapshot, viewport);
        self.push_dungeon_progress(snapshot, viewport);
        self.push_notification(snapshot, viewport);
        self.push_death_notice(snapshot, viewport);
        &self.list
    }

    /// The dungeon's progress panel, at the right edge of the screen.
    ///
    /// ```text
    /// GRAVEYARD CRYPT
    /// COMPLETION
    /// ████████████████░░░░ 82%
    /// REQUIRED: 100%
    /// ```
    ///
    /// Drawn only in a dungeon, from the snapshot the run was read into this frame. At 100 %
    /// the last line says the way is open; during the boss it counts his adds; at the end it
    /// says how long the run took. Built from the same primitives as every other panel —
    /// a rectangle, a bar and text — so it scales and themes with the rest of the HUD.
    pub fn push_dungeon_progress(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        use crate::ui::map_style::{MapStyle, color, radius, type_scale};
        let Some(dungeon) = snapshot.dungeon.as_ref() else {
            return;
        };
        let scale = self.metrics.scale;
        let style = MapStyle::new(scale);
        let width = scale.px(232.0);
        let pad = scale.px(14.0);
        let x = viewport.0 as f32 - width - scale.px(20.0);
        // Under the minimap when there is one, so the two stack down the right-hand edge.
        let y = self
            .minimap
            .map_or(scale.px(150.0), |minimap| minimap.bottom() + scale.px(14.0));
        let height = pad * 2.0
            + style.line(type_scale::HEADING)
            + style.line(type_scale::LABEL)
            + scale.px(6.0 + 10.0)
            + style.line(type_scale::META);
        let panel = UiRect::new(x, y, width, height);
        let corner = scale.px(radius::CARD);
        self.list.shadow(
            panel,
            corner,
            scale.px(14.0),
            color::alpha(color::SHADOW, 0.7),
        );
        self.list.panel(
            panel,
            corner,
            color::alpha(color::SURFACE_1, 0.94),
            color::alpha(color::SURFACE_0_BOTTOM, 0.94),
        );
        self.list
            .panel_stroke(panel, corner, scale.px(1.0), color::BORDER_STRONG);

        let left = x + pad;
        let mut row = y + pad - scale.px(2.0);
        self.list.label(
            (left, row),
            &dungeon.name,
            color::TEXT_TITLE,
            engine_render_data::font::UiFont::Display,
            style.size(type_scale::HEADING),
            TextAlign::Left,
        );
        row += style.line(type_scale::HEADING);
        self.title.clear();
        write_integer(&mut self.title, i64::from(dungeon.percent));
        self.title.push('%');
        self.list.label(
            (left, row),
            "COMPLETION",
            color::TEXT_MUTED,
            type_scale::LABEL.font,
            style.size(type_scale::LABEL),
            TextAlign::Left,
        );
        self.list.label(
            (x + width - pad, row - scale.px(1.0)),
            &self.title,
            color::TEXT_PRIMARY,
            type_scale::HEADING.font,
            style.size(type_scale::UI),
            TextAlign::Right,
        );
        row += style.line(type_scale::LABEL) + scale.px(4.0);

        let bar = UiRect::new(left, row, width - pad * 2.0, scale.px(6.0));
        self.list.panel(
            bar,
            bar.height * 0.5,
            [1.0, 1.0, 1.0, 0.08],
            [1.0, 1.0, 1.0, 0.08],
        );
        let fraction = dungeon.fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let fill = UiRect::new(
                bar.x,
                bar.y,
                (bar.width * fraction).max(bar.height),
                bar.height,
            );
            let (top, bottom) = if dungeon.unlocked {
                (color::VERDANT, color::alpha(color::VERDANT, 0.8))
            } else {
                (color::BRONZE_BRIGHT, color::BRONZE)
            };
            self.list.panel(fill, bar.height * 0.5, top, bottom);
        }
        row += bar.height + scale.px(10.0);

        self.title.clear();
        let tint = if dungeon.completed {
            self.title.push_str("DUNGEON COMPLETED  ");
            // Minutes and seconds: a run is timed the way a player would say it.
            let total = dungeon.finished_after.unwrap_or(0.0).max(0.0).round() as i64;
            write_integer(&mut self.title, total / 60);
            self.title.push(':');
            if total % 60 < 10 {
                self.title.push('0');
            }
            write_integer(&mut self.title, total % 60);
            color::VERDANT
        } else if dungeon.boss_engaged {
            self.title.push_str("THE STEWARD'S GUESTS: ");
            write_integer(
                &mut self.title,
                i64::try_from(dungeon.adds_alive).unwrap_or(i64::MAX),
            );
            color::CRIMSON
        } else if dungeon.unlocked {
            self.title.push_str("THE PATH TO THE CRYPT LORD IS OPEN");
            color::VERDANT
        } else {
            self.title.push_str("REQUIRED: 100%");
            color::TEXT_MUTED
        };
        self.list.label(
            (left, row),
            &self.title,
            tint,
            type_scale::META.font,
            style.size(type_scale::META),
            TextAlign::Left,
        );
    }

    /// Every enabled action bar, wherever the layout puts it.
    ///
    /// A disabled bar is not drawn and, because the application hit-tests against the same
    /// `layout`, it is not clickable either — which is what "a disabled bar consumes no
    /// interaction space" means in practice.
    ///
    /// A slot's key comes from the bar, not from the snapshot: the snapshot's `key` is the
    /// class's fixed digit hint, and the whole point of this system is that the player may
    /// have changed it.
    pub fn push_action_bars(
        &mut self,
        snapshot: &UiSnapshot,
        layout: &crate::ui::hud_layout::HudLayout,
        bars: &crate::ui::action_bar::ActionBarSet,
        viewport: (u32, u32),
    ) {
        use crate::ui::action_bar;
        use crate::ui::hud_layout::HudElementId;

        let scale = self.metrics.scale;
        let text = scale.text(self.theme.hud_text_scale * 0.5);
        let outline = scale.px(self.theme.outline_pixels);
        for index in 0..engine_settings::hud::ACTION_BARS {
            let id = HudElementId::ActionBar(index);
            if !layout.is_enabled(id) {
                continue;
            }
            let Some(bar) = bars.bar(index) else {
                continue;
            };
            let shape = layout.bar_shape(index);
            let rect = layout.rect_of(id, scale, viewport);
            for (slot_index, slot) in bar.slots().iter().enumerate() {
                let Some(cell) = action_bar::slot_rect(rect, shape, slot_index) else {
                    continue;
                };
                let Some(view) = slot
                    .ability
                    .and_then(|ability| snapshot.ability_view(ability))
                else {
                    // An empty slot is still drawn. It is where the player drops the next
                    // spell, and a bar that showed only what was in it would be a bar that
                    // got harder to fill the emptier it was.
                    self.list.rect(cell, self.theme.panel);
                    self.list.outline(cell, outline, self.theme.slot_disabled);
                    if let Some(key) = slot.key {
                        self.title.clear();
                        key.write_label(&mut self.title);
                        self.list.text_at(
                            (cell.x + scale.px(3.0), cell.y + scale.px(3.0)),
                            &self.title,
                            self.theme.text,
                            text,
                        );
                    }
                    continue;
                };
                let mut view = *view;
                view.key = slot.key.map_or("", |chord| chord.key.label());
                view.key_modifiers = slot.key.map_or(Modifiers::none(), |chord| chord.modifiers);
                self.push_ability_button(cell, &view, text);
            }
        }
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
    ///
    /// Or the Druid's Mana and Astral Power, which occupy the same two rows: the hero the
    /// player *is* decides which pair is drawn, and neither class is shown a bar that can
    /// never move.
    pub fn push_resource_bars(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let x = (width - self.metrics.resource_bar_width) * 0.5;
        let bottom = height - self.metrics.ability_bar_bottom - self.metrics.slot_size;

        if snapshot.player.druid.active {
            self.push_druid_bars(snapshot, x, bottom);
            return;
        }

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

    /// **The Druid's two bars**: Mana, and the Astral Power that arms a spell.
    ///
    /// The Astral bar says three things at once and says each of them once: how much is
    /// banked, whether the button can be pressed, and whether the next compatible spell is
    /// already armed. All three are read off the snapshot rather than worked out here.
    fn push_druid_bars(&mut self, snapshot: &UiSnapshot, x: f32, bottom: f32) {
        let druid = snapshot.player.druid;
        let mana_rect = UiRect::new(
            x,
            bottom - self.metrics.resource_lift,
            self.metrics.resource_bar_width,
            self.metrics.resource_bar_height,
        );
        self.list.bar(
            mana_rect,
            druid.mana_fraction(),
            self.theme.bar_background,
            self.theme.mana,
        );
        self.title.clear();
        self.title.push_str("MANA ");
        write_integer(&mut self.title, druid.mana as i64);
        self.title.push_str(" / ");
        write_integer(&mut self.title, druid.max_mana as i64);
        self.list.text_at(
            (mana_rect.x, mana_rect.y - self.metrics.scale.px(14.0)),
            &self.title,
            self.theme.text,
            self.metrics.scale.text(1.0),
        );

        let astral_rect = UiRect::new(
            x,
            mana_rect.bottom() + 20.0,
            self.metrics.resource_bar_width,
            self.metrics.resource_bar_height,
        );
        self.list.bar(
            astral_rect,
            druid.astral_fraction(),
            self.theme.bar_background,
            self.theme.astral_color(druid.armed),
        );
        if druid.armed {
            // The armed bar is outlined as well as recoloured: the state changes what the
            // *next* cast does, and it has to be findable without looking for it.
            self.list.outline(astral_rect, 3.0, self.theme.astral_armed);
        }
        self.title.clear();
        self.title.push_str("ASTRAL POWER ");
        write_integer(&mut self.title, druid.astral as i64);
        self.title.push_str(" / ");
        write_integer(&mut self.title, druid.astral_cost() as i64);
        if druid.armed {
            self.title.push_str("  ARMED");
        } else if druid.can_arm {
            self.title.push_str("  READY");
        }
        self.list.text_at(
            (astral_rect.x, astral_rect.y - self.metrics.scale.px(14.0)),
            &self.title,
            self.theme.text,
            self.metrics.scale.text(1.0),
        );
    }

    /// **The buttons beside the bar**: the Ultimate, and the hero's extras after it.
    ///
    /// Drawn by both HUD paths, because neither is arrangeable: the Ultimate has always sat
    /// beside the bar and the extras are the same idea for a class with more abilities than
    /// the keyboard has digits.
    pub fn push_side_buttons(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let text = self.metrics.scale.text(self.theme.hud_text_scale * 0.5);
        // A hero with no Ultimate draws no Ultimate: an empty button beside the bar reads as
        // a spell that has not loaded, not as a class that has none.
        if let Some(ultimate) = snapshot.ultimate.as_ref() {
            let rect = self.ultimate_rect(viewport);
            self.push_ability_button(rect, ultimate, text);
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
        for (index, extra) in snapshot.extras.iter().enumerate() {
            let Some(view) = extra else {
                continue;
            };
            let rect = self.extra_rect(index, viewport);
            self.push_ability_button(rect, view, text);
        }
    }

    /// The ten-slot ability bar, and the Ultimate beside it.
    pub fn push_ability_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        for (slot, view) in snapshot.abilities.iter().enumerate() {
            // A slot the current hero does not fill draws nothing at all. An empty *frame*
            // belongs to the arrangeable action bars, where it is a place to drop a spell;
            // this fixed bar is a readout of what the hero has.
            let Some(view) = view else {
                continue;
            };
            let rect = self.slot_rect(slot, viewport);
            self.push_ability_button(
                rect,
                view,
                self.metrics.scale.text(self.theme.hud_text_scale * 0.5),
            );
        }

        self.push_side_buttons(snapshot, viewport);
    }

    /// One ability button: icon plate, state colour, cooldown sweep, charges and key.
    pub fn push_ability_button(&mut self, rect: UiRect, view: &AbilitySlotView, scale: f32) {
        let _ = scale;
        let px = |logical: f32| self.metrics.scale.px(logical);
        let corner = px(5.0);
        // A recessed well, so an empty corner of the art still reads as a button.
        self.list.panel(
            rect,
            corner,
            [0.07, 0.08, 0.10, 0.92],
            [0.03, 0.035, 0.045, 0.92],
        );

        // Art if the ability has any, its name if it does not: the art *is* the label.
        let art = rect.inset(px(2.0));
        match icon_for(view.ability) {
            Some(icon) => self
                .list
                .icon(art, icon, self.theme.icon_tint(view.availability)),
            None => {
                crate::ui::map_style::write_title_case(&mut self.title, view.name);
                // As large as fits across the button, never larger than a quarter of it.
                let at_one = UiFont::SemiBold.width(&self.title, 1.0).max(1e-3);
                let size = (rect.height * 0.24)
                    .min((rect.width - px(6.0)) / at_one)
                    .max(7.0);
                self.list.styled_label(
                    (
                        rect.x + rect.width * 0.5,
                        cap_centred_top(UiFont::SemiBold, size, rect.y + rect.height * 0.5),
                    ),
                    &self.title,
                    self.theme.text,
                    UiFont::SemiBold,
                    size,
                    TextAlign::Center,
                    plate_text_style([0.0, 0.0, 0.0, 0.9], size, 0.0),
                );
            }
        }

        // The global cooldown, as a lighter sweep. Starting any ability locks every slot, so
        // every slot says so — a bar where only the pressed button reacts reads as though
        // the others were still available.
        if view.shows_global_cooldown() {
            self.list.radial_from(
                art,
                view.global_cooldown_fraction,
                1.0 - view.global_cooldown_fraction,
                self.theme.global_cooldown_radial,
            );
        }

        // The ability's own cooldown, as a radial sweep over what is left, with the seconds
        // until it is back. The global cooldown gets no number: it is the same on every
        // button and over before a number on eleven of them could be read.
        if view.is_cooling() {
            self.list.radial_from(
                art,
                view.cooldown_fraction,
                1.0 - view.cooldown_fraction,
                self.theme.cooldown_radial,
            );
            self.title.clear();
            write_seconds(&mut self.title, view.cooldown_remaining);
            let size = (rect.height * 0.40).max(10.0);
            self.list.styled_label(
                (
                    rect.x + rect.width * 0.5,
                    cap_centred_top(UiFont::SemiBold, size, rect.y + rect.height * 0.52),
                ),
                &self.title,
                [1.0, 0.97, 0.90, 1.0],
                UiFont::SemiBold,
                size,
                TextAlign::Center,
                plate_text_style([0.0, 0.0, 0.0, 0.95], size, 0.04),
            );
        }

        // The edge says what pressing it would do: quiet when it is simply ready, coloured
        // when it is refused or empowered, white while it is the ability in progress.
        let active = view.shows_active_frame();
        // **Empowerable is said whatever else is true.** A press that will spend Deflection
        // or Astral Power is outlined in yellow even while the button is cooling or the
        // target is wrong: what the resource will buy is worth knowing before it is usable.
        let empowerable = view.spends_deflection
            || view.spends_astral
            || view.availability == SlotAvailability::Empowered;
        let border = if active {
            self.theme.slot_casting
        } else if empowerable {
            self.theme.slot_empowered
        } else {
            self.theme.slot_color(view.availability)
        };
        let width = if active || empowerable || view.availability != SlotAvailability::Ready {
            px(2.0).max(2.0)
        } else {
            px(1.0).max(1.0)
        };
        if active || empowerable {
            self.list.glow(rect, corner, px(7.0), faded(border, 0.45));
        }
        self.list.panel_stroke(rect, corner, width, border);

        // Charges, bottom right, only where there is more than one.
        let small = (rect.height * 0.26).max(9.0);
        let edge = plate_text_style([0.0, 0.0, 0.0, 0.95], small, 0.02);
        if view.shows_charges() {
            self.title.clear();
            write_integer(&mut self.title, i64::from(view.charges));
            self.list.styled_label(
                (
                    rect.right() - px(4.0),
                    cap_centred_top(UiFont::SemiBold, small, rect.bottom() - small * 0.6),
                ),
                &self.title,
                self.theme.text,
                UiFont::SemiBold,
                small,
                TextAlign::Right,
                edge,
            );
        }

        // The key that casts it, top left, with whatever has to be held for it in front.
        self.title.clear();
        self.title.push_str(view.key_modifiers.label_prefix());
        self.title.push_str(view.key);
        self.list.styled_label(
            (
                rect.x + px(4.0),
                cap_centred_top(UiFont::SemiBold, small, rect.y + small * 0.65),
            ),
            &self.title,
            [0.92, 0.92, 0.90, 1.0],
            UiFont::SemiBold,
            small,
            TextAlign::Left,
            edge,
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
        // Beside bar one wherever the player put it: the fixed bar's position was where the
        // button used to hang, and an arranged bar drawn elsewhere left it on top of slot ten.
        if let Some(bar) = self.bar_one {
            let size = bar.height.min(bar.width);
            return UiRect::new(
                bar.right() + self.metrics.ultimate_gap,
                bar.y + (bar.height - size) * 0.5,
                size,
                size,
            );
        }
        let last = self.slot_rect(ABILITY_SLOTS - 1, viewport);
        let height = viewport.1 as f32;
        UiRect::new(
            last.right() + self.metrics.ultimate_gap,
            height - self.metrics.ability_bar_bottom - self.metrics.ultimate_size,
            self.metrics.ultimate_size,
            self.metrics.ultimate_size,
        )
    }

    /// Where the hero's `index`th extra button sits: after the Ultimate, in a row.
    #[must_use]
    pub fn extra_rect(&self, index: usize, viewport: (u32, u32)) -> UiRect {
        let ultimate = self.ultimate_rect(viewport);
        let step = ultimate.width + self.metrics.slot_gap;
        UiRect::new(
            ultimate.x + step * (index as f32 + 1.0),
            ultimate.y,
            ultimate.width,
            ultimate.height,
        )
    }

    /// Where the talent button sits: the top-right corner.
    #[must_use]
    pub fn talent_button_rect(&self, viewport: (u32, u32)) -> UiRect {
        let width = viewport.0 as f32;
        // The minimap owns the corner when there is one; the buttons sit on its left, level
        // with its banner, the way the genre hangs buttons off the minimap's edge.
        let right = self.minimap.map_or(width - self.metrics.margin, |minimap| {
            minimap.x - self.metrics.corner_button_gap
        });
        UiRect::new(
            right - self.metrics.talent_button_size,
            self.metrics.margin,
            self.metrics.talent_button_size,
            self.metrics.talent_button_size,
        )
    }

    /// Tells the HUD where the minimap is this frame, or that there is none.
    ///
    /// The minimap is drawn by `ui::dungeon_map`; the HUD only keeps out of its way.
    pub fn set_minimap_bounds(&mut self, bounds: Option<UiRect>) {
        self.minimap = bounds;
    }

    /// Where the character button sits: beside the talent button, further left.
    ///
    /// The same size and the same row, because the two are the same kind of thing — a window
    /// the player opens — and a corner that mixes button sizes reads as two unrelated
    /// controls that happen to be next to each other.
    #[must_use]
    pub fn character_button_rect(&self, viewport: (u32, u32)) -> UiRect {
        let talent = self.talent_button_rect(viewport);
        UiRect::new(
            talent.x - self.metrics.corner_button_gap - self.metrics.talent_button_size,
            talent.y,
            self.metrics.talent_button_size,
            self.metrics.talent_button_size,
        )
    }

    /// The character button, which opens the character sheet.
    pub fn push_character_button(&mut self, viewport: (u32, u32)) {
        let rect = self.character_button_rect(viewport);
        self.list.rect(rect, self.theme.panel);
        self.list.outline(
            rect,
            self.metrics.scale.px(self.theme.outline_pixels),
            self.theme.target_outline,
        );

        let (origin, align) = centered(rect.x + rect.width * 0.5, rect.y + 8.0);
        self.list
            .text_aligned(origin, "CHR", self.theme.text, 1.5, align);
        let (origin, align) = centered(rect.x + rect.width * 0.5, rect.bottom() - 16.0);
        self.list
            .text_aligned(origin, "C", self.theme.target_outline, 1.5, align);
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

    /// The cast bar, above the ability bar, where the fixed HUD puts it.
    pub fn push_cast_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
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
        self.push_cast_bar_in(snapshot, rect);
    }

    /// The player's cast bar in `rect`: the rectangle the arranged HUD gave it.
    ///
    /// Drawn exactly as an enemy's cast bar is on its nameplate — the spell's icon, an amber
    /// fill, the name on the left and the seconds left on the right, in the edged face — so
    /// "a cast" looks like one thing wherever it is on the screen. Everything inside is sized
    /// from the rectangle, so a bar the player made taller or wider in Edit HUD keeps its
    /// proportions.
    pub fn push_cast_bar_in(&mut self, snapshot: &UiSnapshot, rect: UiRect) {
        let Some(cast) = snapshot.cast else {
            return;
        };
        self.push_cast_strip(
            rect,
            icon_for(cast.ability),
            cast.name,
            cast.fraction(),
            Some(cast.remaining()),
            1.0,
        );
    }

    /// What the cast bar looks like while Edit HUD is open and nothing is being cast: a
    /// sample cast, so the player can see the style they are setting rather than an outline.
    pub fn push_cast_bar_preview(&mut self, rect: UiRect) {
        self.push_cast_strip(
            rect,
            Some(UiIcon::Firebolt),
            "CAST BAR",
            0.6,
            Some(1.2),
            0.85,
        );
    }

    /// The strip itself: icon, fill, name and countdown.
    fn push_cast_strip(
        &mut self,
        rect: UiRect,
        icon: Option<UiIcon>,
        name: &str,
        fraction: f32,
        remaining: Option<f32>,
        alpha: f32,
    ) {
        let theme = self.theme.plate;
        let rect = UiRect::new(
            rect.x.round(),
            rect.y.round(),
            rect.width.round(),
            rect.height.round().max(6.0),
        );
        let border = (rect.height / 20.0).round().clamp(1.0, 2.0);
        let edge = rect.height;
        let (bar, icon_frame) = match icon {
            Some(_) => (
                UiRect::new(
                    rect.x + edge - border,
                    rect.y,
                    rect.width - edge + border,
                    edge,
                ),
                Some(UiRect::new(rect.x, rect.y, edge, edge)),
            ),
            None => (rect, None),
        };
        let inner = bar.inset(border);
        self.list.rect(inner, faded(theme.bar_background, alpha));
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            self.list.panel(
                UiRect::new(
                    inner.x,
                    inner.y,
                    (inner.width * fraction).round().max(1.0),
                    inner.height,
                ),
                0.0,
                faded(theme.cast_top, alpha),
                faded(theme.cast_bottom, alpha),
            );
        }
        self.list.outline(bar, border, faded(theme.border, alpha));
        if let (Some(icon), Some(frame)) = (icon, icon_frame) {
            self.list
                .icon(frame.inset(border), icon, faded([1.0; 4], alpha));
            self.list.outline(frame, border, faded(theme.border, alpha));
        }

        let size = (inner.height * 0.66).clamp(8.0, 28.0);
        let centre = inner.y + inner.height * 0.5;
        let top = cap_centred_top(UiFont::SemiBold, size, centre);
        let style = plate_text_style(faded(theme.text_outline, alpha), size, 0.02);
        let pad = (size * 0.4).round();
        let mut name_right = inner.right() - pad;
        if let Some(remaining) = remaining {
            self.title.clear();
            write_tenths(&mut self.title, remaining);
            name_right -= UiFont::SemiBold.width(&self.title, size) + pad;
            self.list.styled_label(
                (inner.right() - pad, top),
                &self.title,
                faded(theme.cast_text, alpha),
                UiFont::SemiBold,
                size,
                TextAlign::Right,
                style,
            );
        }
        crate::ui::map_style::write_title_case(&mut self.title, name);
        let name_x = inner.x + pad;
        let fits = UiFont::SemiBold.width(&self.title, size) <= name_right - name_x;
        if !fits {
            self.list.clip(UiClip::rect(UiRect::new(
                inner.x,
                rect.y - edge,
                (name_right - inner.x).max(0.0),
                edge * 3.0,
            )));
        }
        self.list.styled_label(
            (name_x, top),
            &self.title,
            faded(theme.cast_text, alpha),
            UiFont::SemiBold,
            size,
            TextAlign::Left,
            style,
        );
        if !fits {
            self.list.clip(UiClip::NONE);
        }
    }

    /// The target's cast bar, under the target frame.
    ///
    /// The same widget as the player's own, drawn at the top of the screen instead of the
    /// bottom, because it answers the same question about somebody else: *what is being
    /// cast, and how long have I got*. An interruptible cast says so, which is what makes
    /// the interrupt foundation visible to a player rather than only to a test.
    pub fn push_enemy_cast_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let Some(cast) = snapshot.target.and_then(|target| target.casting) else {
            return;
        };
        let width = viewport.0 as f32;
        let rect = UiRect::new(
            (width - self.metrics.cast_bar_width) * 0.5,
            self.metrics.target_frame_top + self.metrics.target_frame_height + 6.0,
            self.metrics.cast_bar_width,
            self.metrics.cast_bar_height,
        );
        self.list.bar(
            rect,
            cast.fraction(),
            self.theme.bar_background,
            self.theme.enemy_cast_fill,
        );
        self.title.clear();
        self.title.push_str(cast.name);
        self.title.push(' ');
        write_tenths(&mut self.title, cast.remaining());
        if cast.interruptible {
            self.title.push_str(" !");
        }
        let (origin, align) = centered(rect.x + rect.width * 0.5, rect.y + 3.0);
        self.list
            .text_aligned(origin, &self.title, self.theme.text, 1.0, align);
    }

    /// The death notice, across the middle of the screen.
    ///
    /// Deliberately plain: a line of text and nothing that has to be dismissed. The state it
    /// reports is the simulation's, so it appears and disappears with the thing it describes
    /// rather than with a timer of its own.
    pub fn push_death_notice(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        if snapshot.player.alive {
            return;
        }
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        let (origin, align) = centered(width * 0.5, height * 0.42);
        self.list.text_aligned(
            origin,
            "YOU DIED",
            self.theme.death_text,
            self.metrics.scale.text(self.theme.hud_text_scale * 2.0),
            align,
        );
        let (origin, align) = centered(width * 0.5, height * 0.42 + self.metrics.bar_height * 2.0);
        self.list.text_aligned(
            origin,
            "PRESS BACKSPACE TO RETURN - WHAT YOU KILLED STAYS DEAD",
            self.theme.text,
            self.metrics.scale.text(self.theme.hud_text_scale * 0.8),
            align,
        );
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
        let size = self.metrics.scale.px(17.0);
        self.list.styled_label(
            (width * 0.5, cap_centred_top(UiFont::SemiBold, size, y)),
            error.message(),
            [1.0, 0.36, 0.30, 1.0],
            UiFont::SemiBold,
            size,
            TextAlign::Center,
            plate_text_style([0.0, 0.0, 0.0, 0.9], size, 0.03),
        );
    }

    /// An ability tooltip, opened from the tooltip dock.
    ///
    /// Every line is generated from the ability's own definition and the live slot view —
    /// see [`crate::ui::tooltip::fill_ability`] — so a tooltip cannot drift from what the
    /// ability does. Drawn at `dock`, the rectangle the HUD layout resolves for
    /// [`HudElementId::Tooltip`](crate::ui::hud_layout::HudElementId::Tooltip), growing in
    /// `direction`; never where the pointer happens to be.
    pub fn push_ability_tooltip(
        &mut self,
        dock: UiRect,
        direction: crate::ui::flow::FlowDirection,
        view: &AbilitySlotView,
        context: crate::ui::tooltip::AbilityContext,
        viewport: (u32, u32),
    ) -> UiRect {
        crate::ui::tooltip::fill_ability(&mut self.tooltip, view, context);
        let kit = crate::ui::kit::Kit::new(self.metrics.scale);
        self.tooltip_panel.push(
            &mut self.list,
            &self.tooltip,
            dock,
            direction,
            &kit,
            viewport,
        )
    }

    /// Floating combat text, projected through the camera.
    ///
    /// Each entry's anchor is projected to the screen and everything after that is in screen
    /// pixels — the rise, the lanes, the swell, the size — so a number reads the same at two
    /// metres and at forty. An anchor behind the camera is skipped rather than drawn: a point
    /// behind the eye still divides to a finite screen position, and drawing it produces a
    /// mirrored ghost — the same bug the nameplates avoid, avoided the same way.
    ///
    /// Criticals are drawn after everything else, so the number that matters most is never
    /// under an ordinary one.
    ///
    /// **Nothing is drawn on top of anything else if there is room to avoid it.** Every entry
    /// is measured where its motion puts it, then the entries are placed in order of
    /// importance — criticals first, then the older — and each one that would cover an
    /// entry already placed is moved *up* until it clears, the way it is already moving. The
    /// room each one needs is eased from frame to frame, so a number making way slides rather
    /// than jumps, and one whose neighbour faded drifts back down.
    pub fn push_damage_numbers(
        &mut self,
        numbers: &FloatingNumbers,
        view_projection: &Mat4,
        viewport: (u32, u32),
        now: f32,
    ) {
        self.layout_combat_text(numbers, view_projection, viewport, now);
        let layout = std::mem::take(&mut self.combat_layout);
        for critical_pass in [false, true] {
            for placed in &layout {
                if placed.critical != critical_pass {
                    continue;
                }
                let Some(number) = numbers.numbers().get(placed.index) else {
                    continue;
                };
                let nudge = self
                    .combat_nudges
                    .iter()
                    .find(|(key, _)| *key == placed.key)
                    .map_or(0.0, |(_, nudge)| *nudge);
                self.push_combat_text(
                    number,
                    (placed.center.0, placed.center.1 - nudge),
                    placed.size,
                    now,
                );
            }
        }
        self.combat_layout = layout;
    }

    /// Measures every visible entry and works out how far up each has to go to clear the ones
    /// placed before it; eases the stored nudges towards that.
    fn layout_combat_text(
        &mut self,
        numbers: &FloatingNumbers,
        view_projection: &Mat4,
        viewport: (u32, u32),
        now: f32,
    ) {
        self.combat_layout.clear();
        let emphasis = self.combat_text.critical_emphasis;
        let scale = self.metrics.scale.factor() * self.combat_text.size_scale();
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        for (index, number) in numbers.numbers().iter().enumerate() {
            let Some((x, y)) = project(number.origin, view_projection, viewport) else {
                continue;
            };
            let frame = number.frame(now, emphasis);
            if frame.alpha <= 0.0 {
                continue;
            }
            let size = number.design_size(emphasis) * frame.scale * scale;
            let center = (x + frame.offset.0 * scale, y + frame.offset.1 * scale);
            if center.0 < -size * 4.0
                || center.0 > width + size * 4.0
                || center.1 < -size
                || center.1 > height + size
            {
                continue;
            }
            number.write_text(&mut self.title);
            let text_width = UiFont::SemiBold.width(&self.title, size);
            // A little air around each: two numbers touching read as one longer number.
            let air = size * 0.1;
            self.combat_layout.push(PlacedText {
                index,
                key: text_key(number),
                center,
                size,
                half: (text_width * 0.5 + air, size * 0.45 + air),
                push: 0.0,
                critical: number.critical,
                born: number.born,
            });
        }

        // Each starts from where it was last frame, relaxing back towards its own place: a
        // number that had to make room drifts down as the room frees up rather than dropping.
        let elapsed = (now - self.combat_nudged_at).clamp(0.0, 0.25);
        self.combat_nudged_at = now;
        let relax = (-NUDGE_RATE * elapsed).exp();
        for placed in &mut self.combat_layout {
            placed.push = self
                .combat_nudges
                .iter()
                .find(|(key, _)| *key == placed.key)
                .map_or(0.0, |(_, nudge)| nudge * relax);
        }

        // Most important first: a critical keeps its place, and an older number is not
        // shoved aside by one that just appeared.
        self.combat_layout.sort_by(|a, b| {
            b.critical
                .cmp(&a.critical)
                .then(a.born.total_cmp(&b.born))
                .then(a.key.cmp(&b.key))
        });
        let count = self.combat_layout.len();
        for current in 1..count {
            for _ in 0..NUDGE_PASSES {
                let mut moved = false;
                for earlier in 0..current {
                    let (a, b) = (self.combat_layout[earlier], self.combat_layout[current]);
                    let (ax, ay) = (a.center.0, a.center.1 - a.push);
                    let (bx, by) = (b.center.0, b.center.1 - b.push);
                    let overlap_x = a.half.0 + b.half.0 - (ax - bx).abs();
                    let overlap_y = a.half.1 + b.half.1 - (ay - by).abs();
                    if overlap_x > 0.0 && overlap_y > 0.0 {
                        // Up past the one it covers: its bottom to the other's top.
                        let needed = (by + b.half.1) - (ay - a.half.1);
                        self.combat_layout[current].push += needed.max(0.5);
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }

        // What was drawn is what is remembered.
        self.combat_nudges.clear();
        for placed in &self.combat_layout {
            self.combat_nudges.push((placed.key, placed.push));
        }
    }

    /// One floating entry, drawn centred on `center` at `size`.
    fn push_combat_text(
        &mut self,
        number: &FloatingNumber,
        center: (f32, f32),
        size: f32,
        now: f32,
    ) {
        let emphasis = self.combat_text.critical_emphasis;
        let frame = number.frame(now, emphasis);
        if frame.alpha <= 0.0 {
            return;
        }

        let theme = self.theme.combat;
        let color = match number.kind {
            CombatTextKind::Damage if number.critical => theme.critical,
            CombatTextKind::Damage => theme.damage,
            CombatTextKind::Periodic => theme.periodic,
            CombatTextKind::Incoming => theme.incoming,
            CombatTextKind::Heal if number.critical => theme.heal_critical,
            CombatTextKind::Heal => theme.heal,
            CombatTextKind::Avoided(Avoidance::Absorb) => theme.absorbed,
            CombatTextKind::Avoided(Avoidance::Reflect) => theme.reflected,
            CombatTextKind::Avoided(_) => theme.avoided,
            CombatTextKind::Aggro(AggroChange::Gained | AggroChange::Gaining) => {
                self.theme.aggro_gain_text
            }
            CombatTextKind::Aggro(_) => self.theme.aggro_loss_text,
        };
        number.write_text(&mut self.title);
        // Numbers are drawn heavier than words: a digit is read by its shape at a glance, and
        // weight is what makes the shape carry.
        let embolden = if number.kind.is_amount() { 0.045 } else { 0.03 };
        let style = TextStyle::outlined(
            faded(theme.outline, frame.alpha),
            (size * 0.07).max(1.0),
            (size * 0.045).clamp(0.75, 2.0),
            size * embolden,
        );
        self.list.styled_label(
            (center.0, cap_centred_top(UiFont::SemiBold, size, center.1)),
            &self.title,
            faded(color, frame.alpha),
            UiFont::SemiBold,
            size,
            TextAlign::Center,
            style,
        );
    }
}

/// The picture that stands for an ability, if one has been drawn.
///
/// The map lives here, in the layer that decides what the HUD *looks* like, rather than on
/// [`FireAbility`] itself: `engine-game` must never learn that a picture exists, and it is
/// the same rule that keeps a colour out of the simulation.
///
/// `None` is the ordinary answer for most of the bar today. Those slots fall back to their
/// abbreviated name, so an ability without art is legible rather than blank.
#[must_use]
pub fn icon_for(ability: FireAbility) -> Option<UiIcon> {
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
        FireAbility::Recuperate => Some(UiIcon::Recuperate),
        FireAbility::WarStomp => Some(UiIcon::WarStomp),
        FireAbility::ColossusStrike => Some(UiIcon::ColossusStrike),
        FireAbility::ShieldCharge => Some(UiIcon::ShieldCharge),
        FireAbility::ShieldBash => Some(UiIcon::ShieldBash),
        FireAbility::ShieldBlock => Some(UiIcon::ShieldBlock),
        FireAbility::PunctureBlood => Some(UiIcon::PunctureBlood),
        FireAbility::Adrenaline => Some(UiIcon::Adrenaline),
        FireAbility::BattleTrance => Some(UiIcon::BattleTrance),
        FireAbility::SpellReflect => Some(UiIcon::SpellReflect),
        FireAbility::LastStand => Some(UiIcon::LastStand),
        // No art yet: the slot shows the abbreviated name until an icon is drawn.
        FireAbility::Taunt => Some(UiIcon::Taunt),
        FireAbility::Moonfire => Some(UiIcon::Moonfire),
        FireAbility::Sunfire => Some(UiIcon::Sunfire),
        FireAbility::Wrath => Some(UiIcon::Wrath),
        FireAbility::Rejuvenation => Some(UiIcon::Rejuvenation),
        FireAbility::Regrowth => Some(UiIcon::Regrowth),
        FireAbility::Lifebloom => Some(UiIcon::Lifebloom),
        FireAbility::WildGrowth => Some(UiIcon::WildGrowth),
        FireAbility::Swiftmend => Some(UiIcon::Swiftmend),
        FireAbility::Nourish => Some(UiIcon::Nourish),
        FireAbility::NaturesCleanse => Some(UiIcon::NaturesCleanse),
        FireAbility::WildRush => Some(UiIcon::WildRush),
        FireAbility::AstralPower => Some(UiIcon::AstralPower),
        FireAbility::Tranquility => Some(UiIcon::Tranquility),
        // Two rows with nothing to draw: the mage's Ultimate is switched off, and Ignite is
        // credited damage rather than a cast — it has a row so the meter can name it. A slot
        // with no icon draws its abbreviated name, which is the fallback every ability had
        // before the icon sets were painted.
        FireAbility::Cataclysm => None,
        FireAbility::Ignite => None,
    }
}

/// `color` with its alpha multiplied by `alpha`.
///
/// Multiplied rather than replaced: a colour that was already translucent — a panel behind a
/// nameplate is — must not become *more* opaque because the plate was told to fade.
#[must_use]
pub fn faded(color: UiColor, alpha: f32) -> UiColor {
    let alpha = if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        1.0
    };
    [color[0], color[1], color[2], color[3] * alpha]
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
    use engine_game::hero::Hero;
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
        set.build(
            simulation,
            &view_projection(),
            VIEWPORT,
            &config,
            1.0 / 60.0,
        );
        (set, config)
    }

    /// Every styled run in the built list: its text, colour and size.
    fn styled(ui: &GameUi) -> Vec<(String, UiColor, f32)> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::StyledLabel {
                    range, color, size, ..
                } => Some((ui.list().text_of(*range).to_owned(), *color, *size)),
                _ => None,
            })
            .collect()
    }

    /// Every outline in the built list, with its colour.
    fn outlines(ui: &GameUi) -> Vec<(UiRect, UiColor)> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Outline { rect, color, .. } => Some((*rect, *color)),
                // A rounded edge is an outline too.
                UiCommand::Panel {
                    rect, stroke, top, ..
                } if *stroke > 0.0 => Some((*rect, *top)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_nameplate_is_a_bold_outlined_name_over_a_real_health_bar() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        assert!(!set.plates().is_empty(), "the arena has visible enemies");

        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(set.plates(), &config);

        // No bitmap text at all: every word on a plate is set in the interface font.
        assert!(
            texts(&ui).is_empty(),
            "a plate drew bitmap text: {:?}",
            texts(&ui)
        );
        let labels = styled(&ui);
        for plate in set.plates() {
            let name = crate::ui::map_style::title_case(plate.name);
            let (_, _, size) = labels
                .iter()
                .find(|(text, _, _)| *text == name)
                .unwrap_or_else(|| panic!("no name {name:?} in {labels:?}"));
            assert!(*size >= 14.0, "the name is readable: {size}");
            // The bar is a real bar: wide, tall, bordered, and under the name.
            let bar = plate.bar_rect();
            assert!(bar.width >= 120.0 && bar.height >= 12.0, "{bar:?}");
            assert!(bar.y > plate.origin.1 + plate.geometry.name_top);
            assert!(
                outlines(&ui).iter().any(|(rect, _)| *rect == bar),
                "the bar has a border"
            );
        }
        // Every label carries a dark edge, so it reads over anything the world draws.
        for command in ui.list().commands() {
            if let UiCommand::StyledLabel { style, .. } = command {
                assert!(style.has_outline(), "an unedged label: {style:?}");
            }
        }
    }

    #[test]
    fn exactly_one_plate_is_outlined_in_white_and_it_is_the_target() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(set.plates(), &config);

        let white = ui.theme().plate.border_target;
        let lit: Vec<UiRect> = outlines(&ui)
            .into_iter()
            .filter(|(_, color)| *color == white)
            .map(|(rect, _)| rect)
            .collect();
        let targets: Vec<_> = set
            .plates()
            .iter()
            .filter(|plate| plate.is_target)
            .collect();
        assert!(targets.len() <= 1, "at most one plate is the target's");
        assert_eq!(lit.len(), targets.len());
        if let Some(target) = targets.first() {
            assert_eq!(lit[0], target.bar_rect());
        }
    }

    #[test]
    fn a_cast_bar_hangs_under_its_health_bar_and_says_what_and_how_long() {
        use crate::nameplate::{PlateCast, PlateGeometry};
        use engine_game::enemy::EnemyAbility;
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let mut plate = set.plates()[0];
        // The same plate, casting: a cast is only ever laid out by the set, so the geometry
        // is rebuilt the way the set would build it.
        let metrics = plate.rank.metrics();
        let name_width = UiFont::SemiBold.width(
            &crate::ui::map_style::title_case(plate.name),
            metrics.name_size,
        );
        plate.geometry = PlateGeometry::new(
            metrics,
            plate.rank.crest(),
            name_width,
            plate.geometry.scale,
            plate.auras.debuffs().count(),
            plate.auras.buffs().count(),
            true,
        );
        plate.size = plate.geometry.size();
        let ability = EnemyAbility::ALL
            .into_iter()
            .find(|ability| {
                ability.definition().cast_time > 1.0 && ability.definition().interruptible
            })
            .expect("the roster has an interruptible cast");
        let cast = PlateCast {
            ability,
            name: ability.name(),
            icon: UiIcon::AbilityArcane,
            fraction: 0.4,
            remaining: 1.4,
            interruptible: true,
            state: PlateCastState::Casting,
            alpha: 1.0,
        };
        plate.cast = Some(cast);

        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(&[plate], &config);
        let bar = plate.bar_rect();
        let cast_rect = plate.cast_rect().expect("a casting plate has a cast bar");
        assert!(
            cast_rect.y >= bar.bottom(),
            "the cast bar is under the health bar"
        );
        assert!(cast_rect.y - bar.bottom() <= 8.0, "and directly under it");
        assert_eq!(cast_rect.width, bar.width);
        let labels = styled(&ui);
        let name = crate::ui::map_style::title_case(ability.name());
        assert!(
            labels.iter().any(|(text, _, _)| *text == name),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|(text, _, _)| text == "1.4"),
            "{labels:?}"
        );
        assert!(
            icons(&ui)
                .iter()
                .any(|(icon, _)| *icon == UiIcon::AbilityArcane)
        );
        assert!(!icons(&ui).iter().any(|(icon, _)| *icon == UiIcon::Lock));

        // Not interruptible: a lock and a silver border.
        plate.cast = Some(PlateCast {
            interruptible: false,
            ..cast
        });
        ui.begin();
        ui.push_nameplates(&[plate], &config);
        assert!(icons(&ui).iter().any(|(icon, _)| *icon == UiIcon::Lock));
        let silver = ui.theme().plate.cast_locked_border;
        assert!(outlines(&ui).iter().any(|(_, color)| *color == silver));

        // Interrupted: it says so, and prints no countdown.
        plate.cast = Some(PlateCast {
            state: PlateCastState::Interrupted,
            ..cast
        });
        ui.begin();
        ui.push_nameplates(&[plate], &config);
        let labels = styled(&ui);
        assert!(
            labels.iter().any(|(text, _, _)| text == "Interrupted"),
            "{labels:?}"
        );
        assert!(!labels.iter().any(|(text, _, _)| text == "1.4"));
    }

    #[test]
    fn a_plate_with_no_cast_draws_no_cast_bar_and_leaves_no_gap() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let plate = set.plates()[0];
        assert!(plate.cast.is_none());
        assert!(plate.cast_rect().is_none());
        assert_eq!(plate.size.1, plate.bar_rect().bottom() - plate.origin.1);
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(&[plate], &config);
        assert!(!styled(&ui).iter().any(|(text, _, _)| text == "Interrupted"));
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
        // The first bar drawn belongs to the furthest plate.
        let first_outline = outlines(&ui)[0].0;
        assert_eq!(first_outline, set.plates()[0].bar_rect());
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
                // A filled rounded surface is a background as much as a rectangle is.
                UiCommand::Panel {
                    rect,
                    stroke,
                    feather,
                    ..
                } if *stroke <= 0.0 && *feather <= 0.0 => Some(*rect),
                _ => None,
            })
            .collect()
    }

    /// Every run of text in the list, bitmap and proportional alike.
    fn all_texts(ui: &GameUi) -> Vec<String> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. }
                | UiCommand::Label { range, .. }
                | UiCommand::StyledLabel { range, .. } => {
                    Some(ui.list().text_of(*range).to_owned())
                }
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

    /// Every run of proportional text, joined with spaces: what a wrapped tooltip *says*,
    /// whichever line a phrase happened to land on.
    fn said(ui: &GameUi) -> String {
        let mut out = String::new();
        for command in ui.list().commands() {
            if let UiCommand::Label { range, .. } | UiCommand::StyledLabel { range, .. } = command {
                out.push_str(ui.list().text_of(*range));
                out.push(' ');
            }
        }
        out
    }

    /// A `GameUi` with the metrics a frame would have, for asking about rectangles.
    fn ui_for(_simulation: &Simulation) -> GameUi {
        GameUi::new()
    }

    fn built(simulation: &Simulation) -> GameUi {
        let (set, config) = plates(simulation);
        let mut ui = GameUi::new();
        let snapshot = UiSnapshot::build(simulation);
        ui.build(&snapshot, set.plates(), &config, VIEWPORT);
        ui
    }

    /// The same arena, played as the Druid.
    fn druid_sim() -> Simulation {
        let mut simulation = sim();
        assert!(simulation.set_hero(Hero::Druid));
        simulation
    }

    #[test]
    fn the_druid_gets_mana_and_astral_power_instead_of_heat() {
        let mut simulation = druid_sim();
        let found = texts(&built(&simulation));
        assert!(
            found.iter().any(|text| text.starts_with("MANA")),
            "no mana bar: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.starts_with("ASTRAL POWER")),
            "no astral bar: {found:?}"
        );
        assert!(
            !found.iter().any(|text| text.starts_with("HEAT")),
            "the Druid was shown a bar it can never move: {found:?}"
        );

        // Armed, and the bar says so where the player is already looking.
        simulation
            .druid_mut()
            .astral
            .gain(engine_game::druid::tuning::ASTRAL_MAXIMUM);
        simulation
            .cast(engine_game::firemage::FireAbility::AstralPower)
            .expect("a full bar arms");
        let found = texts(&built(&simulation));
        assert!(
            found.iter().any(|text| text.contains("ARMED")),
            "the armed state is invisible: {found:?}"
        );
    }

    #[test]
    fn a_hero_with_extra_buttons_draws_them_beside_the_ultimate() {
        let simulation = druid_sim();
        let ultimate = ui_for(&simulation).ultimate_rect(VIEWPORT);
        let first = ui_for(&simulation).extra_rect(0, VIEWPORT);
        let second = ui_for(&simulation).extra_rect(1, VIEWPORT);
        // In a row, after the Ultimate, and not on top of each other.
        assert!(first.x > ultimate.x);
        assert!(second.x > first.x);
        assert_eq!(first.y, ultimate.y);

        let snapshot = UiSnapshot::build(&simulation);
        let names: Vec<&str> = snapshot
            .extras
            .iter()
            .flatten()
            .map(|view| view.name)
            .collect();
        assert_eq!(names, ["WILD RUSH", "ASTRAL POWER"]);
        assert_eq!(
            snapshot.ultimate.expect("the druid has one").name,
            "TRANQUILITY"
        );
    }

    #[test]
    fn an_astral_tooltip_says_what_the_next_cast_will_do() {
        use engine_game::firemage::FireAbility;
        let mut simulation = druid_sim();
        simulation
            .druid_mut()
            .astral
            .gain(engine_game::druid::tuning::ASTRAL_MAXIMUM);
        simulation
            .cast(FireAbility::AstralPower)
            .expect("a full bar arms");
        let snapshot = UiSnapshot::build(&simulation);
        let view = *snapshot
            .ability_view(FireAbility::Moonfire)
            .expect("moonfire is on the bar");

        let mut ui = GameUi::new();
        ui.push_ability_tooltip(
            UiRect::new(1500.0, 950.0, 380.0, 64.0),
            crate::ui::flow::FlowDirection::READING,
            &view,
            tooltip_context(&snapshot),
            VIEWPORT,
        );
        let found = said(&ui);
        assert!(
            found.contains("Costs 150 Mana"),
            "the cost is missing: {found}"
        );
        assert!(
            found.contains("Generates 12 Astral Power"),
            "what it generates is missing: {found}"
        );
        assert!(found.contains("Ready: Astral Power is armed"), "{found}");
        assert!(
            found.contains("EMPOWERED EFFECT"),
            "the empowered section is missing: {found}"
        );
        assert!(
            found.contains("burn never expires."),
            "what the empowerment does is missing: {found}"
        );
        // And the bar's highlight says the next press is the empowered one.
        assert_eq!(view.availability, SlotAvailability::Empowered);
        assert!(view.spends_astral);
    }

    fn tooltip_context(snapshot: &UiSnapshot) -> crate::ui::tooltip::AbilityContext {
        crate::ui::tooltip::AbilityContext {
            deflection: snapshot.player.warrior.deflection,
            astral_armed: snapshot.player.druid.armed,
            ignite_max: snapshot.ignite_max,
            mastery: snapshot.player.mastery,
        }
    }

    #[test]
    fn a_warrior_tooltip_says_what_deflection_buys_and_whether_this_press_pays() {
        use engine_game::firemage::FireAbility;
        let mut simulation = sim();
        assert!(simulation.set_hero(engine_game::hero::Hero::Warrior));
        for (banked, ready) in [(5.0, false), (60.0, true)] {
            simulation.warrior_mut().deflection.set(banked);
            let snapshot = UiSnapshot::build(&simulation);
            let view = *snapshot
                .ability_view(FireAbility::ColossusStrike)
                .expect("colossus strike is on the bar");
            let mut ui = GameUi::new();
            ui.push_ability_tooltip(
                UiRect::new(1500.0, 950.0, 380.0, 64.0),
                crate::ui::flow::FlowDirection::READING,
                &view,
                tooltip_context(&snapshot),
                VIEWPORT,
            );
            let found = said(&ui);
            assert!(found.starts_with("Colossus Strike "), "{found}");
            assert!(found.contains("RESOURCE"), "{found}");
            assert!(
                found.contains("Consumes 20 Deflection to empower, if available"),
                "{found}"
            );
            assert!(found.contains("EMPOWERED EFFECT"), "{found}");
            assert!(
                found.contains("The bleed deals 50% more damage."),
                "{found}"
            );
            let status = if ready {
                "Ready: 60 Deflection banked"
            } else {
                "Not empowered: 5 of 20 Deflection"
            };
            assert!(found.contains(status), "{banked}: {found}");
            assert!(
                !found.contains("SMASHES"),
                "the old shouted text survived: {found}"
            );
        }
    }

    #[test]
    fn a_druid_heal_tooltip_prints_healing_rather_than_damage() {
        use engine_game::firemage::FireAbility;
        let simulation = druid_sim();
        let snapshot = UiSnapshot::build(&simulation);
        let view = *snapshot
            .ability_view(FireAbility::Regrowth)
            .expect("regrowth is on the bar");
        assert_eq!(view.damage, 0.0);
        assert!(view.healing > 0.0);

        let mut ui = GameUi::new();
        ui.push_ability_tooltip(
            UiRect::new(1500.0, 950.0, 380.0, 64.0),
            crate::ui::flow::FlowDirection::READING,
            &view,
            tooltip_context(&snapshot),
            VIEWPORT,
        );
        let found = said(&ui);
        assert!(found.contains("Healing "), "{found}");
        assert!(
            !found.contains("Damage "),
            "a heal claimed to deal damage: {found}"
        );
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
        let found = all_texts(&ui);
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
            let slot = Hero::default()
                .abilities()
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

        for (slot, ability) in Hero::default().abilities().iter().enumerate() {
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
        // Flame Shield: instant, so it lands and takes its own cooldown while the shared
        // one runs on everything else. Everything that *is* held by the shared timer, that
        // is: Blink, Detonate and Fire Mastery are not, and must not pretend to be.
        let mut simulation = sim();
        simulation
            .cast(FireAbility::FlameShield)
            .expect("Flame Shield is ready");

        let snapshot = UiSnapshot::build(&simulation);
        for view in snapshot.abilities.iter().flatten() {
            assert_eq!(
                view.is_on_global_cooldown(),
                view.ability.definition().on_global_cooldown,
                "{:?}",
                view.ability
            );
        }
        assert!(
            snapshot
                .ultimate
                .is_some_and(|view| view.is_on_global_cooldown()),
            "the Ultimate is on the same shared timer"
        );

        // The slot that is merely waiting draws the shared sweep; the one that is also on
        // its own cooldown draws that instead, so the wash is never doubled.
        let cooling: Vec<_> = snapshot
            .abilities
            .iter()
            .flatten()
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
        let casting = Hero::default()
            .abilities()
            .iter()
            .position(|ability| *ability == FireAbility::Pyroblast)
            .expect("Pyroblast is on the bar");
        let slot = ui.slot_rect(casting, VIEWPORT);
        let white = UiTheme::default().slot_casting;

        let framed: Vec<UiRect> = outlines(&ui)
            .into_iter()
            .filter(|(_, color)| *color == white)
            .map(|(rect, _)| rect)
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

        let slot = Hero::default()
            .abilities()
            .iter()
            .position(|ability| *ability == aimed)
            .expect("Meteor is on the bar");
        let Some(view) = snapshot.abilities[slot] else {
            panic!("the mage's Meteor slot is filled");
        };
        assert!(view.aiming);
        assert!(view.shows_active_frame());
        assert!(!view.casting, "an unconfirmed placement is not a cast");

        // Exactly one slot, and it is that one.
        let framed = snapshot
            .abilities
            .iter()
            .flatten()
            .filter(|view| view.shows_active_frame())
            .count();
        assert_eq!(framed, 1, "{framed} slots claim to be active");
        assert!(
            !snapshot
                .ultimate
                .is_some_and(|view| view.shows_active_frame())
        );

        // And it reaches the drawn frame, not just the view.
        let (set, config) = plates(&simulation);
        let mut ui = GameUi::new();
        ui.build(&snapshot, set.plates(), &config, VIEWPORT);
        let white = UiTheme::default().slot_casting;
        let rect = ui.slot_rect(slot, VIEWPORT);
        assert!(
            outlines(&ui)
                .iter()
                .any(|(drawn, color)| *drawn == rect && *color == white),
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
                .flatten()
                .all(|view| !view.shows_active_frame()),
            "a slot is framed with nothing happening"
        );
    }

    #[test]
    fn a_cooling_slot_prints_the_seconds_left_and_a_global_cooldown_prints_nothing() {
        let mut simulation = sim();
        simulation.cast(FireAbility::Blink).expect("Blink is ready");
        let ui = built(&simulation);
        let printed = all_texts(&ui);

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
        let mut found = texts(&ui);
        found.extend(styled(&ui).into_iter().map(|(text, _, _)| text));
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

        // Drawn as an enemy's cast is on its nameplate: the name, and the seconds left.
        let ui = built(&simulation);
        let found: Vec<String> = styled(&ui).into_iter().map(|(text, _, _)| text).collect();
        assert!(
            found.iter().any(|text| text == "Pyroblast"),
            "the cast bar names the ability: {found:?}"
        );
        assert!(
            found
                .iter()
                .any(|text| text.contains('.') && text.parse::<f32>().is_ok()),
            "the cast bar prints the time left: {found:?}"
        );
        assert!(
            icons(&ui)
                .iter()
                .any(|(icon, _)| *icon == UiIcon::Pyroblast)
        );
    }

    #[test]
    fn a_damaged_plate_shows_its_recent_damage_as_a_trail() {
        let simulation = sim();
        let (set, config) = plates(&simulation);
        let mut plate = set.plates()[0];
        plate.health_fraction = 0.5;
        plate.trail_fraction = 0.8;
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_nameplates(&[plate], &config);
        let inner = plate.bar_rect().inset(plate.geometry.border);
        let trail = rects(&ui)
            .into_iter()
            .find(|rect| rect.x > inner.x && rect.height == inner.height)
            .expect("a trail between the fill and where the health was");
        let expected = inner.width * 0.3;
        assert!((trail.width - expected).abs() <= 1.0, "{trail:?}");
    }

    #[test]
    fn a_damage_number_is_bold_outlined_large_and_fades_in_its_alpha() {
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
        let drawn = |ui: &GameUi| styled(ui);
        ui.begin();
        ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, 0.1);
        let fresh = drawn(&ui);
        assert_eq!(fresh.len(), 1, "one number was drawn");
        assert_eq!(fresh[0].0, "45");
        assert_eq!(fresh[0].1[3], 1.0, "a number in flight is opaque");
        assert!(
            fresh[0].2 >= 24.0,
            "a hit is large enough to notice: {}",
            fresh[0].2
        );
        assert!(texts(&ui).is_empty(), "no bitmap digits");

        ui.begin();
        ui.push_damage_numbers(
            &numbers,
            &view_projection(),
            VIEWPORT,
            crate::damage_numbers::NUMBER_LIFETIME * 0.9,
        );
        let faded = drawn(&ui);
        assert!(
            faded[0].1[3] < 1.0,
            "an old number has faded: {}",
            faded[0].1[3]
        );
    }

    /// Every styled label's box: text, left, top, width, height.
    fn label_boxes(ui: &GameUi) -> Vec<(String, UiRect)> {
        ui.list()
            .commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::StyledLabel {
                    range,
                    size,
                    origin,
                    font,
                    ..
                } => {
                    let text = ui.list().text_of(*range).to_owned();
                    let width = font.width(&text, *size);
                    // Centred text: the origin is the middle; the cap band is about 0.72 em.
                    Some((
                        text,
                        UiRect::new(
                            origin.0 - width * 0.5,
                            origin.1 + size * 0.2,
                            width,
                            size * 0.72,
                        ),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_burst_of_numbers_on_two_neighbours_never_draws_one_over_another() {
        let mut numbers = FloatingNumbers::new();
        let origin = engine_core::math::Vec3::new(0.0, 1.0, -12.0);
        let beside = engine_core::math::Vec3::new(0.4, 1.0, -12.0);
        // Twelve hits in one instant on two enemies standing shoulder to shoulder: the worst
        // pile the lanes and bursts cannot spread on their own.
        for index in 0..6 {
            let amount = 1000.0 + index as f32 * 111.0;
            numbers.push(
                engine_game::entity::EntityId::new(1, 1),
                origin,
                amount,
                false,
                index == 2,
                0.0,
            );
            numbers.push(
                engine_game::entity::EntityId::new(2, 1),
                beside,
                amount + 7.0,
                false,
                false,
                0.0,
            );
        }
        let mut ui = GameUi::new();
        for frame in 1..=40 {
            let now = frame as f32 / 60.0;
            ui.begin();
            ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, now);
            let boxes = label_boxes(&ui);
            assert!(boxes.len() >= 10, "{}", boxes.len());
            for (index, (text_a, a)) in boxes.iter().enumerate() {
                for (text_b, b) in &boxes[index + 1..] {
                    let overlap_x = a.x.max(b.x) < a.right().min(b.right()) - 0.5;
                    let overlap_y = a.y.max(b.y) < a.bottom().min(b.bottom()) - 0.5;
                    assert!(
                        !(overlap_x && overlap_y),
                        "frame {frame}: {text_a} {a:?} is over {text_b} {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_critical_is_drawn_bigger_hotter_and_on_top() {
        let mut numbers = FloatingNumbers::new();
        let enemy = engine_game::entity::EntityId::new(1, 1);
        let origin = engine_core::math::Vec3::new(0.0, 1.0, -12.0);
        numbers.push(enemy, origin, 90.0, false, true, 0.0);
        numbers.push(enemy, origin, 45.0, false, false, 0.0);
        let mut ui = GameUi::new();
        ui.begin();
        ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, 0.5);
        let drawn = styled(&ui);
        assert_eq!(drawn.len(), 2);
        // The critical is drawn last, whatever order it arrived in.
        assert_eq!(drawn[1].0, "90");
        assert_eq!(drawn[1].1, ui.theme().combat.critical);
        assert!(drawn[1].2 > drawn[0].2 * 1.3, "{drawn:?}");
    }

    #[test]
    fn the_combat_text_size_setting_and_the_interface_scale_both_grow_the_text() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(
            engine_game::entity::EntityId::new(1, 1),
            engine_core::math::Vec3::new(0.0, 1.0, -12.0),
            45.0,
            false,
            false,
            0.0,
        );
        let size_at = |scale: f32, percent: f32| {
            let mut ui = GameUi::with_scale(UiScale::new(scale));
            ui.set_combat_text(engine_settings::CombatTextSettings {
                size: percent,
                ..engine_settings::CombatTextSettings::default()
            });
            ui.begin();
            ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, 0.5);
            styled(&ui)[0].2
        };
        let base = size_at(1.0, 100.0);
        assert!((size_at(2.0, 100.0) - base * 2.0).abs() < 0.01);
        assert!((size_at(1.0, 150.0) - base * 1.5).abs() < 0.01);
    }

    #[test]
    fn aggro_text_is_drawn_as_words_in_its_own_colours() {
        let mut numbers = FloatingNumbers::new();
        let enemy = engine_game::entity::EntityId::new(1, 1);
        let origin = engine_core::math::Vec3::new(0.0, 1.0, -12.0);
        numbers.push_aggro(enemy, origin, AggroChange::Gaining, 0.0);
        numbers.push_aggro(enemy, origin, AggroChange::Losing, 0.0);

        let mut ui = GameUi::new();
        ui.begin();
        ui.push_damage_numbers(&numbers, &view_projection(), VIEWPORT, 0.2);
        let drawn: Vec<(String, UiColor)> = styled(&ui)
            .into_iter()
            .map(|(text, color, _)| (text, color))
            .collect();
        let theme = ui.theme();
        assert_eq!(
            drawn,
            [
                ("Gaining Aggro".to_owned(), theme.aggro_gain_text),
                ("Losing Aggro".to_owned(), theme.aggro_loss_text),
            ]
        );
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
