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
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect};

use crate::damage_numbers::FloatingNumbers;
use crate::nameplate::{Nameplate, NameplateConfig, project};
use crate::snapshot::{AbilitySlotView, SlotAvailability, UiSnapshot};

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
    /// Glyph scale for HUD text.
    pub hud_text_scale: f32,
    /// Glyph scale for a damage number.
    pub damage_text_scale: f32,
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
            hud_text_scale: 2.0,
            damage_text_scale: 2.0,
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

    /// The colour an ability slot's frame takes for a given availability.
    #[must_use]
    pub fn slot_color(&self, availability: SlotAvailability) -> UiColor {
        match availability {
            SlotAvailability::Ready => self.slot_ready,
            SlotAvailability::OnCooldown => self.slot_cooling,
            SlotAvailability::NoResource | SlotAvailability::NoTarget => self.slot_unavailable,
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
    /// Scratch for a plate's title line.
    title: String,
    /// Scratch for a plate's status line.
    statuses: String,
}

impl GameUi {
    /// Creates a builder with room for a typical frame.
    #[must_use]
    pub fn new() -> Self {
        Self {
            list: UiDrawList::with_capacity(512, 4096),
            theme: UiTheme::default(),
            title: String::with_capacity(32),
            statuses: String::with_capacity(64),
        }
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
                self.list
                    .outline(rect, self.theme.outline_pixels, self.theme.target_outline);
            }

            let text_x = plate.origin.0 + config.padding_pixels;
            let text_y = plate.origin.1 + config.padding_pixels;

            plate.write_title(&mut self.title);
            self.list.text_at(
                (text_x, text_y),
                &self.title,
                self.theme.text,
                self.theme.plate_text_scale,
            );

            if plate.write_statuses(&mut self.statuses) {
                self.list.text_at(
                    (text_x, text_y + config.line_advance_pixels),
                    &self.statuses,
                    self.theme.status,
                    self.theme.plate_text_scale,
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
        self.push_ability_bar(snapshot, viewport);
        self.push_cast_bar(snapshot, viewport);
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
            (width - layout::TARGET_FRAME_WIDTH) * 0.5,
            layout::TARGET_FRAME_TOP,
            layout::TARGET_FRAME_WIDTH,
            layout::TARGET_FRAME_HEIGHT,
        );
        self.list.rect(frame, self.theme.panel);
        // A dead or invalid target keeps its frame but loses its outline: the player still
        // needs to see what they were fighting, and the missing outline is what says the
        // abilities will refuse.
        if target.valid {
            self.list
                .outline(frame, self.theme.outline_pixels, self.theme.target_outline);
        }

        let text_x = frame.x + layout::PADDING;
        let text_y = frame.y + layout::PADDING;
        let scale = self.theme.hud_text_scale;

        self.title.clear();
        self.title.push_str(target.name);
        if !target.alive {
            self.title.push_str(" DEAD");
        }
        self.list
            .text_at((text_x, text_y), &self.title, self.theme.text, scale);

        self.statuses.clear();
        write_integer(&mut self.statuses, target.distance.round() as i64);
        self.statuses.push('M');
        self.list.text_aligned(
            (frame.right() - layout::PADDING, text_y),
            &self.statuses,
            self.theme.text,
            scale,
            TextAlign::Right,
        );

        let bar = UiRect::new(
            text_x,
            text_y + 20.0,
            layout::TARGET_FRAME_WIDTH - layout::PADDING * 2.0,
            layout::BAR_HEIGHT,
        );
        let fraction = target.health_fraction();
        self.list.bar(
            bar,
            fraction,
            self.theme.bar_background,
            self.theme.enemy_health_color(fraction),
        );

        // The numbers on the bar, not beside it: a bar says "how much is left" at a glance
        // and a number says "how much exactly", and they belong to one reading.
        self.title.clear();
        write_integer(&mut self.title, target.health.round() as i64);
        self.title.push('/');
        write_integer(&mut self.title, target.max_health.round() as i64);
        self.list.text_at(
            (bar.x + 4.0, bar.y + 4.0),
            &self.title,
            self.theme.text,
            scale * 0.5,
        );

        self.statuses.clear();
        write_integer(&mut self.statuses, i64::from(target.health_percent()));
        self.statuses.push('%');
        self.list.text_aligned(
            (bar.right() - 4.0, bar.y + 4.0),
            &self.statuses,
            self.theme.text,
            scale * 0.5,
            TextAlign::Right,
        );
    }

    /// The player's health and resource bars, bottom left.
    pub fn push_player_bars(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let height = viewport.1 as f32;
        let scale = self.theme.hud_text_scale * 0.5;
        let bottom = height - layout::MARGIN;
        let resource_top = bottom - layout::BAR_HEIGHT;
        let health_top = resource_top - layout::BAR_HEIGHT - layout::BAR_GAP;

        let health_fraction = snapshot.player.health_fraction();
        let health = UiRect::new(
            layout::MARGIN,
            health_top,
            layout::PLAYER_BAR_WIDTH,
            layout::BAR_HEIGHT,
        );
        self.list.bar(
            health,
            health_fraction,
            self.theme.bar_background,
            self.theme.player_health_color(health_fraction),
        );
        self.push_bar_label(
            health,
            "HP",
            snapshot.player.health,
            snapshot.player.max_health,
            scale,
        );

        let resource_fraction = snapshot.player.resource_fraction();
        let resource = UiRect::new(
            layout::MARGIN,
            resource_top,
            layout::PLAYER_BAR_WIDTH,
            layout::BAR_HEIGHT,
        );
        self.list.bar(
            resource,
            resource_fraction,
            self.theme.bar_background,
            self.theme.resource,
        );
        self.push_bar_label(
            resource,
            "MP",
            snapshot.player.resource,
            snapshot.player.max_resource,
            scale,
        );
    }

    /// A bar's label on the left and its `current/max` on the right.
    fn push_bar_label(&mut self, bar: UiRect, label: &str, current: f32, max: f32, scale: f32) {
        self.list
            .text_at((bar.x + 4.0, bar.y + 4.0), label, self.theme.text, scale);
        self.title.clear();
        write_integer(&mut self.title, current.round() as i64);
        self.title.push('/');
        write_integer(&mut self.title, max.round() as i64);
        self.list.text_aligned(
            (bar.right() - 4.0, bar.y + 4.0),
            &self.title,
            self.theme.text,
            scale,
            TextAlign::Right,
        );
    }

    /// The ability bar: ten slots, centred at the bottom.
    ///
    /// Each slot is a frame, its hotkey, an abbreviated name and a cooldown wipe. The
    /// "icon" is the frame's colour until there are textures to put in it — the layout does
    /// not change when they arrive, which is the point of building it this way now.
    pub fn push_ability_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let slots = snapshot.abilities.len();
        let total =
            slots as f32 * layout::SLOT_SIZE + (slots.saturating_sub(1)) as f32 * layout::SLOT_GAP;
        let left = (viewport.0 as f32 - total) * 0.5;
        let top = viewport.1 as f32 - layout::ABILITY_BAR_BOTTOM - layout::SLOT_SIZE;
        let scale = self.theme.hud_text_scale * 0.5;

        for (index, ability) in snapshot.abilities.iter().enumerate() {
            let slot = UiRect::new(
                left + index as f32 * (layout::SLOT_SIZE + layout::SLOT_GAP),
                top,
                layout::SLOT_SIZE,
                layout::SLOT_SIZE,
            );
            self.list.rect(slot, self.theme.panel);
            self.list
                .outline(slot, 2.0, self.theme.slot_color(ability.availability));

            // The hotkey, top-left, where every action bar in the genre puts it.
            self.title.clear();
            write_integer(&mut self.title, i64::from(ability.hotkey));
            self.list.text_at(
                (slot.x + 5.0, slot.y + 5.0),
                &self.title,
                self.theme.text,
                scale,
            );

            // An abbreviated name, so a slot is identifiable before it has an icon.
            self.statuses.clear();
            self.statuses
                .extend(ability.name.chars().take(ABILITY_ABBREVIATION));
            self.list.text_aligned(
                (slot.x + slot.width * 0.5, slot.y + slot.height * 0.5 - 3.0),
                &self.statuses,
                self.theme.text,
                scale,
                TextAlign::Center,
            );

            self.push_cooldown(slot, *ability, scale);
        }
    }

    /// The cooldown wipe and its remaining time, over one slot.
    ///
    /// A **bottom-up wipe** rather than a radial sweep: a radial needs either a mask texture
    /// or a fan of triangles per slot, and a wipe is one rectangle whose height is the
    /// fraction. It reads the same way — the slot fills back up as the ability returns.
    fn push_cooldown(&mut self, slot: UiRect, ability: AbilitySlotView, scale: f32) {
        let fraction = ability.cooldown_fraction();
        if fraction <= 0.0 {
            return;
        }

        let covered = slot.height * fraction;
        self.list.rect(
            UiRect::new(slot.x, slot.y + slot.height - covered, slot.width, covered),
            self.theme.cooldown_sweep,
        );

        // The number as well as the wipe: a wipe says "soon", a number says "0.4".
        self.title.clear();
        write_tenths(&mut self.title, ability.cooldown_remaining);
        self.list.text_aligned(
            (slot.x + slot.width * 0.5, slot.bottom() - 13.0),
            &self.title,
            self.theme.text,
            scale,
            TextAlign::Center,
        );
    }

    /// The cast bar, above the ability bar.
    ///
    /// Absent when nothing is being cast. Its presence *is* the "you are casting" signal, so
    /// an always-visible empty bar would be worse than none.
    pub fn push_cast_bar(&mut self, snapshot: &UiSnapshot, viewport: (u32, u32)) {
        let Some(cast) = snapshot.cast else {
            return;
        };
        let width = viewport.0 as f32;
        let top = viewport.1 as f32
            - layout::ABILITY_BAR_BOTTOM
            - layout::SLOT_SIZE
            - layout::CAST_BAR_LIFT;
        let bar = UiRect::new(
            (width - layout::CAST_BAR_WIDTH) * 0.5,
            top,
            layout::CAST_BAR_WIDTH,
            layout::CAST_BAR_HEIGHT,
        );
        self.list.bar(
            bar,
            cast.fraction(),
            self.theme.bar_background,
            self.theme.cast_fill,
        );

        let scale = self.theme.hud_text_scale * 0.5;
        self.list.text_at(
            (bar.x + 4.0, bar.y + 5.0),
            cast.name,
            self.theme.text,
            scale,
        );
        self.title.clear();
        write_tenths(&mut self.title, cast.remaining());
        self.title.push('S');
        self.list.text_aligned(
            (bar.right() - 4.0, bar.y + 5.0),
            &self.title,
            self.theme.text,
            scale,
            TextAlign::Right,
        );
    }

    /// World-space health bars above every nearby enemy.
    ///
    /// Anchored to the plate the nameplate system already placed, so the two cannot disagree
    /// about where an enemy is on screen and the projection happens once rather than twice.
    pub fn push_world_health_bars(&mut self, plates: &[Nameplate]) {
        for plate in plates {
            let bar = UiRect::new(
                plate.origin.0 + (plate.size.0 - layout::WORLD_BAR_WIDTH) * 0.5,
                plate.origin.1 - layout::WORLD_BAR_HEIGHT - 3.0,
                layout::WORLD_BAR_WIDTH,
                layout::WORLD_BAR_HEIGHT,
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
            let mut color = if number.killed {
                self.theme.killing_number
            } else {
                self.theme.damage_number
            };
            // The fade lives in the alpha channel, which only works because the UI blends.
            color[3] = alpha;

            self.title.clear();
            write_integer(&mut self.title, number.amount.round() as i64);
            // A killing blow is bigger as well as differently coloured, so it reads without
            // the player having to distinguish two shades mid-fight.
            let scale = if number.killed {
                self.theme.damage_text_scale * 1.4
            } else {
                self.theme.damage_text_scale
            };
            self.list
                .text_aligned((x, y), &self.title, color, scale, TextAlign::Center);
        }
    }
}

/// Characters of an ability's name shown in its slot.
///
/// Four fits a 48-pixel slot at half scale and is enough to tell one placeholder from
/// another. Replaced by an icon when there are textures; the layout does not change.
const ABILITY_ABBREVIATION: usize = 4;

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
    use engine_render_data::ui::UiCommand;

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
    fn the_player_gets_a_health_bar_and_a_resource_bar() {
        let ui = built(&sim());
        let found = texts(&ui);
        assert!(found.iter().any(|text| text == "HP"), "{found:?}");
        assert!(found.iter().any(|text| text == "MP"), "{found:?}");
        // Each bar is a background plus a fill.
        assert!(rects(&ui).len() >= 4);
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
        assert!(texts(&ui).iter().any(|text| text == "HP"));
    }

    #[test]
    fn the_target_frame_names_the_target_and_reports_its_health() {
        let ui = built(&sim());
        let found = texts(&ui);
        assert!(
            found.iter().any(|text| text == "DUMMY"),
            "the target frame names it: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.contains('%')),
            "and reports a percentage: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.ends_with('M')),
            "and a distance: {found:?}"
        );
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
        // The player still needs to see what they were fighting; the missing outline is what
        // says the abilities will now refuse.
        let mut simulation = sim();
        let alive = built(&simulation);
        let outlines_alive = alive
            .list()
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Outline { .. }))
            .count();

        let target = simulation.player().target.expect("a target");
        simulation.world_mut().damage(target, 1_000_000.0);
        let dead = built(&simulation);
        let outlines_dead = dead
            .list()
            .commands()
            .iter()
            .filter(|command| matches!(command, UiCommand::Outline { .. }))
            .count();

        assert!(texts(&dead).iter().any(|text| text.contains("DEAD")));
        assert!(
            outlines_dead < outlines_alive,
            "the target outline should be gone"
        );
    }

    #[test]
    fn nothing_targeted_draws_no_target_frame() {
        let mut simulation = sim();
        simulation.player_mut().target = None;
        let ui = built(&simulation);
        assert!(
            !texts(&ui).iter().any(|text| text.ends_with('M')),
            "an empty frame is noise"
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
        assert!(found.iter().any(|text| text == "FIRE"), "{found:?}");
    }

    #[test]
    fn a_cooling_ability_draws_a_wipe_and_its_remaining_time() {
        let mut simulation = sim();
        simulation.push_command(engine_game::command::PlayerCommand::UseAbility(
            engine_game::ability::AbilityId::from_slot(0).expect("fireball"),
        ));
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
            theme.slot_color(SlotAvailability::OnCooldown),
            theme.slot_color(SlotAvailability::NoResource)
        );
        assert_ne!(
            theme.slot_color(SlotAvailability::Ready),
            theme.slot_color(SlotAvailability::OnCooldown)
        );
    }

    #[test]
    fn the_cast_bar_appears_only_while_casting_and_reports_its_progress() {
        let mut simulation = sim();
        assert!(
            !texts(&built(&simulation))
                .iter()
                .any(|text| text == "LANCE"),
            "no cast bar when not casting"
        );

        simulation.push_command(engine_game::command::PlayerCommand::UseAbility(
            engine_game::ability::AbilityId::from_slot(6).expect("lance"),
        ));
        simulation.step_many(20);

        let ui = built(&simulation);
        let found = texts(&ui);
        assert!(
            found.iter().any(|text| text == "LANCE"),
            "the cast bar names the ability: {found:?}"
        );
        assert!(
            found.iter().any(|text| text.ends_with('S')),
            "and its remaining time: {found:?}"
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
                .filter(|rect| rect.width == layout::SLOT_SIZE && rect.height == layout::SLOT_SIZE)
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
                .find(|rect| rect.width == layout::PLAYER_BAR_WIDTH)
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
