//! Nameplates: what each nearby enemy is, how hurt it is, and what it is casting.
//!
//! ```text
//!             [crest] Skeleton Bonecaster         <- name, bold, outlined
//!            ┌──────────────────────────────┐
//!            │██████████████████▒▒▒░░░░ 72% │    <- health: fill, recent-damage trail, text
//!            └──────────────────────────────┘
//!            ┌──┬───────────────────────────┐
//!            │◆ │█████████░░ Shadow Bolt 1.4│    <- cast bar, only while it casts
//!            └──┴───────────────────────────┘
//!                          |
//!                     (the monster)
//! ```
//!
//! A nameplate is a *projection*, not an object: gameplay knows nothing about it, and the
//! renderer is handed pixels and colours. This module is where the two meet — it takes the
//! simulation and the camera's view-projection and produces a list of laid-out plates in
//! screen space, which [`crate::game_ui::GameUi`] then draws.
//!
//! Everything here is arithmetic over `engine-core` types, so the projection, the culling
//! rules, the plate's geometry and the cast bar's state are all unit tested with no device and
//! no window.
//!
//! ## One component, anchored on its health bar
//!
//! The name, the health bar and the cast bar are one plate with one origin, measured together
//! here and drawn together by the interface. The **health bar** is what sits on the anchor —
//! the point above the monster's head — and the cast bar hangs *below* it. A plate that
//! starts casting therefore grows downward instead of lifting its name and bar off the
//! monster, and the collapsed cast bar leaves no gap when nothing is being cast.
//!
//! ## Sizes are logical pixels
//!
//! Every measurement in [`PlateMetrics`] is a logical pixel at the design size, multiplied by
//! [`NameplateConfig::scale`] — the interface scale times the player's nameplate size — and by
//! a gentle falloff with distance. A plate is therefore the same size on the screen whatever
//! the perspective, legible at forty metres and not enormous at two, and a 4K display at an
//! interface scale of two draws exactly the plate a 1080p display draws at one.
//!
//! ## What decides whether a plate is drawn
//!
//! 1. The entity is hostile to the player and alive — or died a moment ago, in which case its
//!    plate fades out over [`DEATH_FADE_SECONDS`] instead of vanishing mid-swing.
//! 2. It is within [`NameplateConfig::max_distance`] of the **player**, not of the camera:
//!    the question a player is asking is "what is near me", and a camera pulled back would
//!    otherwise change which enemies have plates.
//! 3. Its anchor point is in front of the camera and inside the viewport. Behind the camera
//!    the projection is a mirrored ghost, which is the classic nameplate bug.
//!
//! ## The cast bar is the simulation's cast
//!
//! Progress, duration and interruptibility are read straight off the enemy's own cast every
//! frame — the numbers its AI is counting down — so there is no second timer to drift. The
//! two things the brain has already forgotten by the time the plate is drawn, an interrupted
//! cast and a completed one, are remembered from the `CastEnded` event for a moment so the
//! bar can say so: red and frozen for an interrupt, a brief fade for a cast that landed.

use crate::aura::AuraList;
use crate::ui::aura_row::AuraRowStyle;
use engine_core::math::{Mat4, Vec3, Vec4};
use engine_game::enemy::{EnemyAbility, MonsterRole, MonsterTier};
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::{CastEnd, Simulation};
use engine_game::world::{EntityKind, horizontal_distance};
use engine_render_data::font::UiFont;
use engine_render_data::ui::{UiIcon, UiRect};
use engine_settings::NameplateHealthText;

/// How far an enemy may be from the player and still have a nameplate, in metres.
pub const NAMEPLATE_MAX_DISTANCE: f32 = engine_core::tuning::nameplates::MAX_DISTANCE_METRES;

/// Plates drawn at once, nearest first, before the rest are dropped.
///
/// A bound rather than a preference. Twenty covers everything a player is fighting; past it
/// the plates are a crowd nobody can read and a cost the interface does not need to pay.
pub const MAX_NAMEPLATES: usize = engine_core::tuning::nameplates::MAX_PLATES;

/// Buff and debuff icons a plate shows before it stops listing them: two lines of six.
///
/// A bound so a pack under a dozen effects each cannot grow its plates down the screen.
pub const MAX_NAMEPLATE_AURAS: usize = 12;

/// Icons on one line under a plate's health bar.
pub const NAMEPLATE_AURAS_PER_LINE: usize = 6;

/// Seconds a plate takes to fade out after its enemy dies.
///
/// Long enough that the killing blow's plate is seen to empty rather than vanish under the
/// cursor, short enough that a cleared pack is clear.
pub const DEATH_FADE_SECONDS: f32 = 0.6;

/// Seconds an interrupted cast stays on its bar, red and frozen where it was stopped.
pub const INTERRUPT_SHOW_SECONDS: f32 = 0.9;

/// Seconds a completed cast's bar takes to fade.
pub const COMPLETE_FADE_SECONDS: f32 = 0.25;

/// Seconds the recent-damage trail holds before it drains towards the health.
const TRAIL_HOLD_SECONDS: f32 = 0.35;

/// How fast the recent-damage trail drains, in fractions of the bar per second.
const TRAIL_DRAIN_PER_SECOND: f32 = 0.9;

/// How important an enemy is, as its plate shows it.
///
/// Read from the monster's own table — its tier and its role — never from its name, so a new
/// boss is drawn as a boss by being entered as one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlateRank {
    /// A pack monster, and anything without a table (a training dummy).
    #[default]
    Normal,
    /// An elite, or an empowered monster a boss has called.
    Elite,
    /// A boss.
    Boss,
}

impl PlateRank {
    /// The rank of `entity`, from its archetype.
    #[must_use]
    pub fn of(simulation: &Simulation, entity: EntityId) -> Self {
        let Some(brain) = simulation.brain(entity) else {
            return Self::Normal;
        };
        let archetype = brain.kind().archetype();
        if archetype.tier == MonsterTier::Boss || archetype.role == MonsterRole::Boss {
            Self::Boss
        } else if archetype.tier == MonsterTier::Empowered || archetype.role == MonsterRole::Elite {
            Self::Elite
        } else {
            Self::Normal
        }
    }

    /// The plate's measurements at this rank, in logical pixels.
    #[must_use]
    pub fn metrics(self) -> PlateMetrics {
        match self {
            Self::Normal => PlateMetrics::NORMAL,
            Self::Elite => PlateMetrics::ELITE,
            Self::Boss => PlateMetrics::BOSS,
        }
    }

    /// The small crest drawn before the name, if this rank has one.
    ///
    /// A shape as well as a colour, so the rank reads for a player who cannot tell the two
    /// border colours apart.
    #[must_use]
    pub fn crest(self) -> Option<UiIcon> {
        match self {
            Self::Normal => None,
            Self::Elite => Some(UiIcon::CrestElite),
            Self::Boss => Some(UiIcon::MapBoss),
        }
    }
}

/// A plate's measurements at one rank, in logical pixels at the design size.
///
/// The typography and the bars in one table, so a rank's plate is a row rather than a set of
/// special cases through the drawing code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateMetrics {
    /// The name's size, in pixels per em.
    pub name_size: f32,
    /// The health bar's width.
    pub bar_width: f32,
    /// The health bar's height, border included.
    pub bar_height: f32,
    /// The health text's size, in pixels per em.
    pub health_text_size: f32,
    /// The cast bar's height, border included.
    pub cast_height: f32,
    /// The cast bar's text size, in pixels per em.
    pub cast_text_size: f32,
    /// A buff or debuff icon's edge, logical pixels.
    pub aura_edge: f32,
    /// The debuff row's icons, as a multiple of `aura_edge`: the player's Enemy Debuff Size.
    pub debuff_scale: f32,
    /// The buff row's icons, as a multiple of `aura_edge`.
    pub buff_scale: f32,
    /// The border around the bars.
    pub border: f32,
}

impl PlateMetrics {
    /// A pack monster's plate: the size the rest are scaled from.
    pub const NORMAL: Self = Self {
        name_size: 18.0,
        bar_width: 168.0,
        bar_height: 15.0,
        health_text_size: 12.0,
        cast_height: 17.0,
        cast_text_size: 12.5,
        aura_edge: 26.0,
        debuff_scale: 1.0,
        buff_scale: 1.0,
        border: 1.0,
    };

    /// An elite's plate: a step larger, so it is picked out of its pack.
    pub const ELITE: Self = Self {
        name_size: 19.0,
        bar_width: 192.0,
        bar_height: 17.0,
        health_text_size: 13.0,
        cast_height: 18.0,
        cast_text_size: 13.0,
        aura_edge: 27.0,
        debuff_scale: 1.0,
        buff_scale: 1.0,
        border: 1.0,
    };

    /// A boss's plate: the most prominent thing in the room.
    pub const BOSS: Self = Self {
        name_size: 21.0,
        bar_width: 236.0,
        bar_height: 20.0,
        health_text_size: 14.0,
        cast_height: 19.0,
        cast_text_size: 13.5,
        aura_edge: 29.0,
        debuff_scale: 1.0,
        buff_scale: 1.0,
        border: 2.0,
    };

    /// The same plate with its icon rows at the player's sizes.
    #[must_use]
    pub fn with_aura_scales(mut self, debuffs: f32, buffs: f32) -> Self {
        let clean = |value: f32| {
            if value.is_finite() && value > 0.0 {
                value
            } else {
                1.0
            }
        };
        self.debuff_scale = clean(debuffs);
        self.buff_scale = clean(buffs);
        self
    }
}

/// Pixels between the name and the health bar, in logical pixels.
const NAME_GAP: f32 = 2.0;

/// Pixels between the health bar and the cast bar, in logical pixels.
const CAST_GAP: f32 = 4.0;

/// Pixels between a row of buff or debuff icons and what it sits against — the name above
/// the bar, the bar itself — in logical pixels.
const AURA_GAP: f32 = 3.0;

/// Pixels between a crest and the name, in logical pixels.
const CREST_GAP: f32 = 4.0;

/// How tall a line of text is allowed to be, as a multiple of its size: the caps and the
/// descenders with a little air, tighter than the face's own line height because a plate is a
/// single line.
const LINE_BOX: f32 = 1.18;

/// Where every part of a plate sits, relative to its top-left corner, in physical pixels.
///
/// Worked out once per plate per frame, in [`NameplateSet::build`], and then used by both the
/// layout and the drawing: a click is tested against the rectangle the plate was drawn in,
/// because they are the same rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateGeometry {
    /// Physical pixels per logical pixel for this plate, distance included.
    pub scale: f32,
    /// The debuffs: the top of their row, over the name, and how they are drawn. `None`
    /// when there are none to show.
    pub debuffs: Option<(f32, AuraRowStyle)>,
    /// The buffs: the top of their row, under the health bar, and how they are drawn.
    /// `None` when there are none to show.
    pub buffs: Option<(f32, AuraRowStyle)>,
    /// The top of the name's line box.
    pub name_top: f32,
    /// The name's size, in pixels per em.
    pub name_size: f32,
    /// The name's measured width, crest included.
    pub name_width: f32,
    /// The crest before the name, if the rank has one, and its edge.
    pub crest: Option<(UiIcon, f32)>,
    /// The health bar, border included.
    pub bar: UiRect,
    /// The border's thickness.
    pub border: f32,
    /// The health text's size, in pixels per em.
    pub health_text_size: f32,
    /// The cast bar, border included, when a cast is shown.
    pub cast: Option<UiRect>,
    /// The cast bar's text size, in pixels per em.
    pub cast_text_size: f32,
    /// A debuff icon's edge on this plate, in pixels, whether or not it has any.
    pub debuff_edge: f32,
    /// How many debuff icons the row over the name was laid out for.
    pub debuff_count: usize,
    /// How many buff icons the row under the bar was laid out for.
    pub buff_count: usize,
}

impl PlateGeometry {
    /// Lays out a plate of `metrics` at `scale` whose name measures `name_width_at_one`
    /// pixels at the design size, with `debuffs` icons over its name, `buffs` icons under its
    /// health bar and with or without a cast bar under those.
    ///
    /// Everything is rounded to whole pixels: a bar with a half-pixel edge is a bar with a grey
    /// seam, and a plate that moves by fractions shimmers.
    #[must_use]
    pub fn new(
        metrics: PlateMetrics,
        crest: Option<UiIcon>,
        name_width_at_one: f32,
        scale: f32,
        debuffs: usize,
        buffs: usize,
        casting: bool,
    ) -> Self {
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let debuff_count = debuffs.min(MAX_NAMEPLATE_AURAS);
        let buff_count = buffs.min(MAX_NAMEPLATE_AURAS);
        let row_style = |factor: f32| {
            AuraRowStyle::new(
                px(metrics.aura_edge * factor),
                NAMEPLATE_AURAS_PER_LINE,
                MAX_NAMEPLATE_AURAS,
            )
        };
        let name_size = metrics.name_size * scale;
        let crest_edge = crest.map(|_| (name_size * 0.95).round());
        let name_width =
            name_width_at_one * scale + crest_edge.map_or(0.0, |edge| edge + px(CREST_GAP));
        let bar_width = px(metrics.bar_width);
        let width = bar_width.max(name_width.ceil());

        // On top, the debuffs: what the player put on the enemy and is keeping up, where the
        // eye already goes to read the name. The health bar is the anchor, so the row grows
        // the plate upward without moving the bar off the monster's head.
        let mut y = 0.0;
        let debuffs = (debuff_count > 0).then(|| {
            let style = row_style(metrics.debuff_scale);
            let top = y;
            y += style.height_for(debuff_count) + px(AURA_GAP);
            (top, style)
        });
        let name_top = y;
        y += (name_size * LINE_BOX).round() + px(NAME_GAP);
        let bar = UiRect::new(
            ((width - bar_width) * 0.5).round(),
            y,
            bar_width,
            px(metrics.bar_height),
        );
        // Under the health bar, the enemy's own buffs; under those, its cast.
        let mut below = bar.bottom();
        let buffs = (buff_count > 0).then(|| {
            let style = row_style(metrics.buff_scale);
            let top = below + px(AURA_GAP);
            below = top + style.height_for(buff_count);
            (top, style)
        });
        let cast = casting.then(|| {
            UiRect::new(
                bar.x,
                below + px(CAST_GAP),
                bar.width,
                px(metrics.cast_height),
            )
        });
        Self {
            scale,
            debuffs,
            buffs,
            name_top,
            name_size,
            name_width,
            crest: crest.zip(crest_edge),
            bar,
            border: px(metrics.border),
            health_text_size: metrics.health_text_size * scale,
            cast,
            cast_text_size: metrics.cast_text_size * scale,
            debuff_edge: px(metrics.aura_edge * metrics.debuff_scale),
            debuff_count,
            buff_count,
        }
    }

    /// The whole plate's size.
    #[must_use]
    pub fn size(&self) -> (f32, f32) {
        let width = self.bar.width.max(self.name_width.ceil());
        let bars = self.cast.map_or(self.bar.bottom(), UiRect::bottom);
        let buffs = self
            .buffs
            .map_or(0.0, |(top, style)| top + style.height_for(self.buff_count));
        (width, bars.max(buffs))
    }

    /// How far below the plate's top its anchor is: the bottom of the health bar.
    #[must_use]
    pub fn anchor_depth(&self) -> f32 {
        self.bar.bottom()
    }
}

/// What a plate's cast bar is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlateCastState {
    /// The enemy is casting it now.
    Casting,
    /// It was interrupted: the bar holds where it stopped, red, for a moment.
    Interrupted,
    /// It landed: the bar fills and fades.
    Completed,
}

/// One enemy's cast, as its plate shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlateCast {
    /// Which attack.
    pub ability: EnemyAbility,
    /// Its name, as the table spells it.
    pub name: &'static str,
    /// The school icon drawn at the bar's left.
    pub icon: UiIcon,
    /// How far through it is, in `[0, 1]`.
    pub fraction: f32,
    /// Seconds left before it lands.
    pub remaining: f32,
    /// Whether the player could interrupt it.
    pub interruptible: bool,
    /// Casting, interrupted or completed.
    pub state: PlateCastState,
    /// What the bar is drawn at, in `[0, 1]`: one while casting, fading after it ends.
    pub alpha: f32,
}

/// One laid-out nameplate, in screen space.
///
/// `Copy` and free of heap storage — the aura list is a fixed array — so the whole set
/// lives in one reusable `Vec` that is cleared every frame without releasing anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nameplate {
    /// Which enemy this describes.
    pub entity: EntityId,
    /// Where the plate's *top-left corner* goes, in pixels from the top-left of the target.
    ///
    /// The position after the layout has separated it from its neighbours, which is where it
    /// is drawn and where a click on it is tested. [`Nameplate::preferred`] is where it
    /// wanted to be.
    pub origin: (f32, f32),
    /// Where the plate would sit if nothing else were on the screen: its health bar directly
    /// above its monster's head.
    ///
    /// Kept so the distance between the two is observable — "how far did the layout have to
    /// move this" is the number the whole design is about minimising, and a plate that has
    /// drifted from its monster is a bug that is otherwise invisible from the outside.
    pub preferred: (f32, f32),
    /// What the whole plate is drawn at, in `[0, 1]`.
    ///
    /// One for the target and for everything out of combat; the player's configured
    /// non-target opacity for the rest of a fight; falling to zero while a dead enemy's plate
    /// fades.
    pub alpha: f32,
    /// Plate size in pixels.
    pub size: (f32, f32),
    /// Where each part of it sits.
    pub geometry: PlateGeometry,
    /// Distance from the player, in metres.
    pub distance: f32,
    /// Whether this is the player's current target.
    pub is_target: bool,
    /// The monster's name, as its table spells it (upper case); drawn in title case.
    pub name: &'static str,
    /// How important it is.
    pub rank: PlateRank,
    /// Health remaining, in [0, 1].
    pub health_fraction: f32,
    /// Health remaining.
    pub health: f32,
    /// Health at full.
    pub max_health: f32,
    /// Damage absorbed before health moves, from [`Simulation::absorb_of`]: drawn as its own
    /// layer on the bar, like every other health bar.
    pub absorb: f32,
    /// Where the recent-damage trail ends, in [0, 1]: at or above the health, draining down.
    pub trail_fraction: f32,
    /// Whether the enemy has died and this is its plate fading out.
    pub dead: bool,
    /// Whether it is held immune by its pylons: the bar is tinted and outlined.
    pub immune: bool,
    /// What it is casting, if the plate is showing a cast.
    pub cast: Option<PlateCast>,
    /// Clip-space depth the plate would be drawn at, nearer smaller.
    ///
    /// The interface paints in order now and the set is sorted furthest first, which is what
    /// actually puts a nearer plate on top; this is kept for a caller that sorts by it.
    pub depth: f32,
    /// The buffs and debuffs on the enemy, debuffs first, as its plate shows them. Empty when
    /// the plate's aura row is turned off.
    pub auras: AuraList,
}

impl Nameplate {
    /// Health remaining as a whole percentage, in 0..=100.
    ///
    /// A living enemy never reads `0%`: rounding a sliver of health down to zero says it is
    /// dead when the next hit still has something to remove.
    #[must_use]
    pub fn health_percent(&self) -> u32 {
        let rounded = (self.health_fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
        if rounded == 0 && self.health_fraction > 0.0 {
            1
        } else {
            rounded
        }
    }

    /// Whether the plate shows anything on this enemy.
    #[must_use]
    pub fn has_auras(&self) -> bool {
        !self.auras.is_empty()
    }

    /// The centre-top of the debuff row over the name in screen pixels, and how it is drawn —
    /// while it has one.
    #[must_use]
    pub fn debuff_row(&self) -> Option<((f32, f32), AuraRowStyle)> {
        self.geometry.debuffs.map(|row| self.row_at(row))
    }

    /// Where the debuffs over this plate are, in screen pixels — or, on a plate with none,
    /// where `sample` of them would go. What Edit HUD outlines as the Enemy Debuffs frame.
    #[must_use]
    pub fn debuff_area(&self, sample: usize) -> UiRect {
        let centre = self.origin.0 + self.geometry.bar.x + self.geometry.bar.width * 0.5;
        if let Some(((x, top), style)) = self.debuff_row() {
            let count = self.geometry.debuff_count.max(1);
            let width = style.width_for(count);
            return UiRect::new(x - width * 0.5, top, width, style.height_for(count));
        }
        let edge = self.geometry.debuff_edge;
        let gap = (edge * 0.12).round().max(1.0);
        let count = sample.max(1) as f32;
        let width = count * edge + (count - 1.0) * gap;
        UiRect::new(
            centre - width * 0.5,
            self.origin.1 - edge - gap,
            width,
            edge,
        )
    }

    /// The centre-top of the buff row under the health bar in screen pixels, and how it is
    /// drawn — while it has one.
    #[must_use]
    pub fn buff_row(&self) -> Option<((f32, f32), AuraRowStyle)> {
        self.geometry.buffs.map(|row| self.row_at(row))
    }

    /// A row's top, centred over the health bar, in screen pixels.
    fn row_at(&self, (top, style): (f32, AuraRowStyle)) -> ((f32, f32), AuraRowStyle) {
        (
            (
                self.origin.0 + self.geometry.bar.x + self.geometry.bar.width * 0.5,
                self.origin.1 + top,
            ),
            style,
        )
    }

    /// The plate's health bar in screen pixels.
    #[must_use]
    pub fn bar_rect(&self) -> UiRect {
        offset(self.geometry.bar, self.origin)
    }

    /// The plate's cast bar in screen pixels, while it shows one.
    #[must_use]
    pub fn cast_rect(&self) -> Option<UiRect> {
        self.geometry.cast.map(|rect| offset(rect, self.origin))
    }

    /// Writes what the health bar prints, in the chosen form, into `out`.
    ///
    /// In place, into a buffer the caller reuses: a plate is drawn every frame for every
    /// visible enemy, and formatting into a fresh `String` each time would allocate per enemy
    /// per frame. Returns whether anything was written.
    pub fn write_health_text(&self, mode: NameplateHealthText, out: &mut String) -> bool {
        use std::fmt::Write as _;
        out.clear();
        if self.dead {
            return false;
        }
        match mode {
            NameplateHealthText::None => return false,
            NameplateHealthText::Percent => {
                let _ = write!(out, "{}%", self.health_percent());
            }
            NameplateHealthText::Value => write_abbreviated(out, self.health),
            NameplateHealthText::Both => {
                write_abbreviated(out, self.health);
                let _ = write!(out, " \u{00B7} {}%", self.health_percent());
            }
        }
        true
    }
}

/// Writes a health value the way a plate prints it: `850`, `12.4K`, `1.2M`.
pub(crate) fn write_abbreviated(out: &mut String, value: f32) {
    use std::fmt::Write as _;
    let value = value.max(0.0);
    if value >= 1_000_000.0 {
        let _ = write!(out, "{:.1}M", value / 1_000_000.0);
    } else if value >= 10_000.0 {
        let _ = write!(out, "{:.0}K", value / 1_000.0);
    } else if value >= 1_000.0 {
        let _ = write!(out, "{:.1}K", value / 1_000.0);
    } else {
        let _ = write!(out, "{}", value.ceil() as i64);
    }
}

/// `rect` moved by `by`.
fn offset(rect: UiRect, by: (f32, f32)) -> UiRect {
    UiRect::new(rect.x + by.0, rect.y + by.1, rect.width, rect.height)
}

/// How nameplates are placed and sized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameplateConfig {
    /// Whether plates are drawn at all.
    pub enabled: bool,
    /// How far an enemy may be from the player and still have a plate, in metres.
    pub max_distance: f32,
    /// How many plates are drawn at once. The nearest survive.
    pub max_plates: usize,
    /// Metres above the enemy's head the plate's health bar is anchored.
    pub height_offset: f32,
    /// Physical pixels per logical pixel: the interface scale times the player's plate size.
    pub scale: f32,
    /// What the health bar prints.
    pub health_text: NameplateHealthText,
    /// Whether a plate shows its enemy's cast.
    pub cast_bars: bool,
    /// Whether a plate shows the buffs and debuffs on its enemy.
    pub auras: bool,
    /// Whether plates are moved apart when they would cover one another.
    pub stacking: bool,
    /// Minimum vertical gap between two plates, as a multiple of a plate's height.
    pub vertical_margin: f32,
    /// Minimum horizontal gap between two plates, as a multiple of a plate's width.
    pub horizontal_margin: f32,
    /// Fraction of the top of the viewport no plate may enter.
    pub top_safe_zone: f32,
    /// What a non-target plate is drawn at while the player is in combat, in `[0, 1]`.
    pub non_target_alpha: f32,
    /// How fast a plate slides towards its resolved position, in fraction per second.
    ///
    /// Zero places it outright. The smoothing exists because a plate whose neighbour is
    /// walking in and out of collision with it would otherwise snap between two positions
    /// every few frames; it is fast enough that the plate is never visibly behind its body.
    pub smoothing_per_second: f32,
    /// How small a plate at the distance limit is, relative to one close by.
    ///
    /// A little smaller rather than the same size: the plates of a far pack stop competing
    /// with the one the player is fighting, and a room full of monsters reads in depth. One
    /// turns the falloff off.
    pub far_scale: f32,
    /// The debuff icons over a name, as a multiple of their design size.
    pub debuff_scale: f32,
    /// The buff icons under a health bar, as a multiple of their design size.
    pub buff_scale: f32,
}

impl Default for NameplateConfig {
    fn default() -> Self {
        let defaults = engine_settings::NameplateSettings::default();
        Self {
            enabled: defaults.enabled,
            max_distance: NAMEPLATE_MAX_DISTANCE,
            max_plates: MAX_NAMEPLATES,
            height_offset: defaults.height_metres(),
            scale: 1.0,
            health_text: defaults.health_text,
            cast_bars: defaults.cast_bars,
            auras: defaults.auras,
            stacking: defaults.stacking,
            vertical_margin: defaults.vertical_margin,
            horizontal_margin: defaults.horizontal_margin,
            top_safe_zone: defaults.top_safe_zone,
            non_target_alpha: defaults.non_target_alpha(),
            smoothing_per_second: crate::nameplate_layout::DEFAULT_SMOOTHING,
            far_scale: 0.82,
            debuff_scale: 1.0,
            buff_scale: 1.0,
        }
    }
}

/// Distance within which a plate is drawn at its full size, in metres.
const FULL_SIZE_DISTANCE: f32 = 15.0;

impl NameplateConfig {
    /// The rules the layout has to satisfy at `viewport`.
    ///
    /// The one place a nameplate setting becomes a layout constraint, so the settings screen
    /// and the resolver cannot be given different numbers.
    #[must_use]
    pub fn constraints(&self, viewport: (u32, u32)) -> crate::nameplate_layout::LayoutConstraints {
        crate::nameplate_layout::LayoutConstraints {
            viewport: (viewport.0 as f32, viewport.1 as f32),
            vertical_margin: self.vertical_margin,
            horizontal_margin: self.horizontal_margin,
            top_safe_zone: self.top_safe_zone,
            ..crate::nameplate_layout::LayoutConstraints::default()
        }
    }

    /// This configuration with everything the player can set taken from `settings`, drawn at
    /// the interface scale `ui_scale`.
    ///
    /// The conversion from displayed units to engine units lives in `engine-settings`; this
    /// is the one call that applies the result, so no caller has to remember which of the
    /// numbers are metres and which are multiples.
    #[must_use]
    pub fn with_settings(mut self, settings: engine_settings::NameplateSettings) -> Self {
        self.enabled = settings.enabled;
        self.max_distance = settings.max_distance;
        self.height_offset = settings.height_metres();
        self.health_text = settings.health_text;
        self.cast_bars = settings.cast_bars;
        self.auras = settings.auras;
        self.stacking = settings.stacking;
        self.vertical_margin = settings.vertical_margin;
        self.horizontal_margin = settings.horizontal_margin;
        self.top_safe_zone = settings.top_safe_zone;
        self.non_target_alpha = settings.non_target_alpha();
        self.debuff_scale = settings.debuff_scale();
        self.buff_scale = settings.buff_scale();
        self
    }

    /// This configuration drawn at `ui_scale` times the plate size in `settings`.
    #[must_use]
    pub fn at_scale(mut self, ui_scale: f32, settings: engine_settings::NameplateSettings) -> Self {
        let scale = ui_scale * settings.text_scale();
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        self
    }

    /// What a plate `distance` metres away is scaled by, on top of [`NameplateConfig::scale`].
    #[must_use]
    pub fn distance_scale(&self, distance: f32) -> f32 {
        let far = self.far_scale.clamp(0.25, 1.0);
        let span = (self.max_distance - FULL_SIZE_DISTANCE).max(1.0);
        let t = ((distance - FULL_SIZE_DISTANCE) / span).clamp(0.0, 1.0);
        1.0 + (far - 1.0) * t
    }
}

/// A cast the brain has already forgotten, remembered for its bar.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CastEcho {
    entity: EntityId,
    ability: EnemyAbility,
    fraction: f32,
    interruptible: bool,
    state: PlateCastState,
    at: f32,
    tick: u64,
}

/// What a plate remembers from one frame to the next: the recent-damage trail, and the cast
/// it was last seen casting.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlateMemory {
    entity: EntityId,
    trail: f32,
    hold: f32,
    last_health: f32,
    last_cast: Option<(EnemyAbility, f32, bool)>,
    seen: bool,
}

/// An enemy that has just died, whose plate is fading.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Death {
    entity: EntityId,
    at: f32,
    tick: u64,
}

/// Casts and deaths remembered at once.
const ECHO_SLOTS: usize = 32;

/// Builds the frame's nameplates.
///
/// Holds the list across frames and clears it rather than releasing it, so a steady scene
/// allocates nothing.
#[derive(Debug, Default, Clone)]
pub struct NameplateSet {
    plates: Vec<Nameplate>,
    /// Where the plates would go if none of them collided. Rebuilt every frame from the
    /// world, never carried over: this is the input to the layout, not its memory.
    boxes: Vec<crate::nameplate_layout::PlateBox>,
    /// The collision resolver, which remembers the previous frame's positions to smooth.
    layout: crate::nameplate_layout::NameplateLayout,
    /// Each plate's trail and last cast, carried from frame to frame.
    memory: Vec<PlateMemory>,
    /// Interrupted and completed casts, for the moment their bars stay up.
    echoes: Vec<CastEcho>,
    /// Enemies that have just died.
    deaths: Vec<Death>,
    /// Scratch for measuring a name in title case.
    scratch: String,
    /// Every enemy that could have a plate this frame, before the nearest are laid out.
    candidates: Vec<Candidate>,
}

/// An enemy that passed the cheap tests, waiting to find out whether it is near enough to be
/// laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Candidate {
    entity: EntityId,
    distance: f32,
    screen: (f32, f32),
    fading: Option<f32>,
}

impl NameplateSet {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(MAX_NAMEPLATES)
    }

    /// An empty set with room for `capacity` plates.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            plates: Vec::with_capacity(capacity),
            boxes: Vec::with_capacity(capacity),
            layout: crate::nameplate_layout::NameplateLayout::with_capacity(capacity),
            memory: Vec::with_capacity(capacity.max(MAX_NAMEPLATES) * 2),
            echoes: Vec::with_capacity(ECHO_SLOTS),
            deaths: Vec::with_capacity(ECHO_SLOTS),
            scratch: String::with_capacity(48),
            candidates: Vec::with_capacity(capacity.max(MAX_NAMEPLATES) * 4),
        }
    }

    /// The plates built by the last [`NameplateSet::build`].
    #[must_use]
    pub fn plates(&self) -> &[Nameplate] {
        &self.plates
    }

    /// How many plates fit before the list grows.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.plates.capacity()
    }

    /// The plate under `pointer`, if the pointer is on one.
    ///
    /// A nameplate is a box on the screen with an enemy's name and health in it, so it is a
    /// thing that can be clicked. The *nearest* plate wins where two overlap, which is the
    /// same order they are painted in — so the one the player can read is the one they
    /// select. A dead enemy's fading plate cannot be selected.
    ///
    /// Screen-space, and deliberately not a ray through the body: a plate floats above its
    /// enemy and is still a plate the player pointed at when the body under it is behind a
    /// pillar.
    #[must_use]
    pub fn at(&self, pointer: (f32, f32)) -> Option<&Nameplate> {
        self.plates
            .iter()
            .filter(|plate| {
                !plate.dead
                    && pointer.0 >= plate.origin.0
                    && pointer.0 <= plate.origin.0 + plate.size.0
                    && pointer.1 >= plate.origin.1
                    && pointer.1 <= plate.origin.1 + plate.size.1
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    /// Rebuilds the set for the current simulation state and camera.
    ///
    /// `viewport` is the render target size in pixels and `delta_seconds` is the frame's real
    /// time step, which the smoothing and the health trail read. Returns the plates, ordered
    /// furthest first, which is the order they are painted in.
    ///
    /// Every frame starts from the world again: the preferred position is projected afresh,
    /// the layout is resolved afresh, and no resolved position is ever written back as if it
    /// were where a monster is. That is what makes a plate follow its monster rather than
    /// slowly drift away from it.
    pub fn build(
        &mut self,
        simulation: &Simulation,
        view_projection: &Mat4,
        viewport: (u32, u32),
        config: &NameplateConfig,
        delta_seconds: f32,
    ) -> &[Nameplate] {
        self.plates.clear();
        let now = simulation.time() as f32;
        self.observe(simulation.events(), now);

        let (width, height) = viewport;
        if width == 0 || height == 0 || !config.enabled {
            return &self.plates;
        }

        let world = simulation.world();
        let player = simulation.player().entity;
        let Some(player_position) = world.position(player) else {
            return &self.plates;
        };
        let target = simulation.player().target;
        let max_distance = config.max_distance.max(0.0);

        // Two passes. The first is cheap and runs over every entity in the world: hostile,
        // alive or fading, in range, in front of the camera, roughly on screen. The second
        // does the real work — name, rank, statuses, cast, measured geometry — for the nearest
        // candidates only, until the plate limit is reached. A thousand enemies in range cost a
        // thousand distance checks and a sort, not a thousand laid-out plates.
        self.candidates.clear();
        // Generous bounds for the rough on-screen test: the widest plate, the tallest stack
        // above its anchor and the cast bar below it, at this scale. The exact test is made
        // once the plate is measured.
        let reach_x = PlateMetrics::BOSS.bar_width * 1.6 * config.scale;
        let reach_up = 90.0 * config.scale;
        let reach_down = 40.0 * config.scale;
        for entity in world.entities() {
            if entity == player || world.kind(entity) == Some(EntityKind::Player) {
                continue;
            }
            let hostile = world
                .faction(player)
                .zip(world.faction(entity))
                .is_some_and(|(mine, theirs)| mine.is_hostile_to(theirs));
            if !hostile {
                continue;
            }
            // A dead enemy keeps its plate only while that plate is fading.
            let fading = if world.is_alive(entity) {
                None
            } else {
                match self.deaths.iter().find(|death| death.entity == entity) {
                    Some(death) if now - death.at < DEATH_FADE_SECONDS => {
                        Some(1.0 - (now - death.at) / DEATH_FADE_SECONDS)
                    }
                    _ => continue,
                }
            };

            let Some(position) = world.position(entity) else {
                continue;
            };
            let distance = horizontal_distance(player_position, position);
            if distance > max_distance {
                continue;
            }

            let anchor =
                position + Vec3::new(0.0, world.height(entity) + config.height_offset, 0.0);
            let Some(screen) = project(anchor, view_projection, viewport) else {
                continue;
            };
            if screen.0 < -reach_x
                || screen.0 > width as f32 + reach_x
                || screen.1 < -reach_down
                || screen.1 > height as f32 + reach_up
            {
                continue;
            }
            self.candidates.push(Candidate {
                entity,
                distance,
                screen,
                fading,
            });
        }

        // Nearest first: which enemies get a plate has to be a rule ("the twenty nearest")
        // rather than an accident of iteration order. `sort_unstable_by`, not `sort_by`: the
        // stable sort allocates a merge buffer once the list passes about twenty entries,
        // which is one allocation per frame in any scene with more than twenty enemies in
        // range. The tie-break on entity id already makes the order total.
        self.candidates.sort_unstable_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| a.entity.cmp(&b.entity))
        });

        for index in 0..self.candidates.len() {
            if self.plates.len() >= config.max_plates {
                break;
            }
            let Candidate {
                entity,
                distance,
                screen: (screen_x, screen_y),
                fading,
            } = self.candidates[index];
            let Some(health) = world.health(entity) else {
                continue;
            };
            let mut auras = AuraList::default();
            if config.auras && fading.is_none() {
                plate_auras(simulation, entity, &mut auras);
            }

            let rank = PlateRank::of(simulation, entity);
            let name = simulation
                .brain(entity)
                .map(|brain| brain.kind().name())
                .filter(|name| !name.is_empty())
                .or_else(|| world.label(entity))
                .or_else(|| world.kind(entity).map(EntityKind::as_str))
                .unwrap_or("ENEMY");
            let cast = if config.cast_bars && fading.is_none() {
                self.cast_of(simulation, entity, now)
            } else {
                None
            };

            let metrics = rank
                .metrics()
                .with_aura_scales(config.debuff_scale, config.buff_scale);
            crate::ui::map_style::write_title_case(&mut self.scratch, name);
            let name_width = UiFont::SemiBold.width(&self.scratch, metrics.name_size);
            let scale = config.scale * config.distance_scale(distance);
            let geometry = PlateGeometry::new(
                metrics,
                rank.crest(),
                name_width,
                scale,
                auras.debuffs().count(),
                auras.buffs().count(),
                cast.is_some(),
            );
            let size = geometry.size();
            // Anchored on the bottom-centre of the health bar, so the bar sits above the head
            // and stays there whether or not a cast bar is hanging under it.
            let origin = (
                (screen_x - size.0 * 0.5).round(),
                (screen_y - geometry.anchor_depth()).round(),
            );
            if !overlaps_viewport(origin, size, viewport) {
                continue;
            }

            self.plates.push(Nameplate {
                entity,
                origin,
                preferred: origin,
                alpha: fading.unwrap_or(1.0),
                size,
                geometry,
                distance,
                is_target: target == Some(entity) && fading.is_none(),
                name,
                rank,
                health_fraction: if fading.is_some() {
                    0.0
                } else {
                    health.fraction()
                },
                health: health.current(),
                max_health: health.max(),
                absorb: if fading.is_some() {
                    0.0
                } else {
                    simulation.absorb_of(entity)
                },
                trail_fraction: health.fraction(),
                dead: fading.is_some(),
                immune: fading.is_none()
                    && world.statuses(entity).is_some_and(|statuses| {
                        statuses.has(engine_game::status::StatusKind::Shielded)
                    }),
                cast,
                depth: depth_for(distance, max_distance),
                auras,
            });
        }
        // Built nearest first; painted furthest first, so a nearer plate is drawn later and
        // therefore on top.
        self.plates.reverse();

        // Only what is actually going to be drawn takes part in the layout: a plate that was
        // culled for distance, for being behind the camera or for being past the plate limit
        // cannot push anything, because it is not there.
        if config.stacking {
            self.resolve_layout(config, viewport, delta_seconds);
        }
        self.remember(delta_seconds);
        self.fade_non_targets(simulation, config);
        &self.plates
    }

    /// Reads the casts that ended and the enemies that died from this frame's events.
    ///
    /// Idempotent: an event is recognised by its tick, so a frame that reads the same slice
    /// twice — the simulation did not advance — remembers each thing once.
    fn observe(&mut self, events: &[CombatEvent], now: f32) {
        // Forget what has finished showing, keeping the allocation.
        self.echoes.retain(|echo| {
            now - echo.at
                < match echo.state {
                    PlateCastState::Interrupted => INTERRUPT_SHOW_SECONDS,
                    _ => COMPLETE_FADE_SECONDS,
                }
                && now >= echo.at
        });
        self.deaths
            .retain(|death| now - death.at < DEATH_FADE_SECONDS && now >= death.at);

        for event in events {
            match *event {
                CombatEvent::CastEnded {
                    source,
                    ability,
                    end,
                    tick,
                } => {
                    let state = match end {
                        CastEnd::Interrupted => PlateCastState::Interrupted,
                        CastEnd::Completed => PlateCastState::Completed,
                        // Moved, cancelled, lost sight, died: the cast simply stops, and the
                        // bar comes down with it.
                        _ => continue,
                    };
                    let Some(enemy_ability) = ability.enemy() else {
                        continue;
                    };
                    if self
                        .echoes
                        .iter()
                        .any(|echo| echo.entity == source && echo.tick == tick)
                    {
                        continue;
                    }
                    // Where the bar was when it stopped: the last frame's reading of the
                    // brain's own progress. A completed cast is full by definition.
                    let (fraction, interruptible) = self
                        .memory
                        .iter()
                        .find(|memory| memory.entity == source)
                        .and_then(|memory| memory.last_cast)
                        .filter(|(last, _, _)| *last == enemy_ability)
                        .map_or(
                            (1.0, enemy_ability.definition().interruptible),
                            |(_, fraction, interruptible)| (fraction, interruptible),
                        );
                    let echo = CastEcho {
                        entity: source,
                        ability: enemy_ability,
                        fraction: if state == PlateCastState::Completed {
                            1.0
                        } else {
                            fraction
                        },
                        interruptible,
                        state,
                        at: now,
                        tick,
                    };
                    self.echoes.retain(|old| old.entity != source);
                    if self.echoes.len() >= ECHO_SLOTS {
                        self.echoes.remove(0);
                    }
                    self.echoes.push(echo);
                }
                CombatEvent::Death { entity, tick, .. } => {
                    if self
                        .deaths
                        .iter()
                        .any(|death| death.entity == entity && death.tick == tick)
                    {
                        continue;
                    }
                    self.deaths.retain(|death| death.entity != entity);
                    if self.deaths.len() >= ECHO_SLOTS {
                        self.deaths.remove(0);
                    }
                    self.deaths.push(Death {
                        entity,
                        at: now,
                        tick,
                    });
                }
                _ => {}
            }
        }
    }

    /// What `entity`'s cast bar shows: its live cast, or a cast that just ended.
    fn cast_of(&self, simulation: &Simulation, entity: EntityId, now: f32) -> Option<PlateCast> {
        if let Some(cast) = simulation.brain(entity).and_then(|brain| brain.cast()) {
            let definition = cast.ability.definition();
            return Some(PlateCast {
                ability: cast.ability,
                name: cast.ability.name(),
                icon: crate::ui::dungeon_map::ability_icon(definition),
                fraction: cast.fraction(),
                remaining: cast.remaining(),
                interruptible: cast.interruptible,
                state: PlateCastState::Casting,
                alpha: 1.0,
            });
        }
        let echo = self.echoes.iter().find(|echo| echo.entity == entity)?;
        let age = (now - echo.at).max(0.0);
        let alpha = match echo.state {
            // Held, then faded over its last third.
            PlateCastState::Interrupted => {
                let fade_from = INTERRUPT_SHOW_SECONDS * 0.66;
                if age <= fade_from {
                    1.0
                } else {
                    1.0 - (age - fade_from) / (INTERRUPT_SHOW_SECONDS - fade_from)
                }
            }
            _ => 1.0 - age / COMPLETE_FADE_SECONDS,
        };
        if alpha <= 0.0 {
            return None;
        }
        let definition = echo.ability.definition();
        Some(PlateCast {
            ability: echo.ability,
            name: echo.ability.name(),
            icon: crate::ui::dungeon_map::ability_icon(definition),
            fraction: echo.fraction,
            remaining: 0.0,
            interruptible: echo.interruptible,
            state: echo.state,
            alpha: alpha.clamp(0.0, 1.0),
        })
    }

    /// Carries each plate's trail and cast to the next frame, and fills in this frame's
    /// trail.
    ///
    /// The trail is the genre's "recent damage" band: health that was just lost stays on the
    /// bar in a pale colour for a moment and then drains away, so a big hit reads as big even
    /// though the fill itself moved the instant it landed. The fill is the simulation's
    /// health, never smoothed — damage feedback that lags is damage feedback that lies.
    fn remember(&mut self, delta_seconds: f32) {
        let dt = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, 0.25)
        } else {
            0.0
        };
        for memory in &mut self.memory {
            memory.seen = false;
        }
        for plate in &mut self.plates {
            let health = plate.health_fraction;
            let cast = plate
                .cast
                .filter(|cast| cast.state == PlateCastState::Casting)
                .map(|cast| (cast.ability, cast.fraction, cast.interruptible));
            match self
                .memory
                .iter_mut()
                .find(|memory| memory.entity == plate.entity)
            {
                Some(memory) => {
                    if health < memory.last_health - 1e-5 {
                        // A new hit: hold the trail where it was, then drain.
                        memory.hold = TRAIL_HOLD_SECONDS;
                    }
                    if health >= memory.trail {
                        memory.trail = health;
                    } else if memory.hold > 0.0 {
                        memory.hold -= dt;
                    } else {
                        memory.trail = (memory.trail - TRAIL_DRAIN_PER_SECOND * dt).max(health);
                    }
                    memory.last_health = health;
                    memory.last_cast = cast;
                    memory.seen = true;
                    plate.trail_fraction = memory.trail;
                }
                None => {
                    // A plate seen for the first time has no recent damage to show.
                    // Bounded by the plate limit: memory is kept only for plates drawn.
                    self.memory.push(PlateMemory {
                        entity: plate.entity,
                        trail: health,
                        hold: 0.0,
                        last_health: health,
                        last_cast: cast,
                        seen: true,
                    });
                    plate.trail_fraction = health;
                }
            }
        }
        // A plate not drawn this frame forgets its trail; if it returns, it returns clean.
        self.memory.retain(|memory| memory.seen);
    }

    /// Moves the plates apart, as little as they can be moved.
    fn resolve_layout(
        &mut self,
        config: &NameplateConfig,
        viewport: (u32, u32),
        delta_seconds: f32,
    ) {
        self.boxes.clear();
        for plate in &self.plates {
            let weight = if plate.is_target {
                TARGET_WEIGHT
            } else if plate.dead {
                // A fading plate is on its way out and should not push a living one aside.
                DEAD_WEIGHT
            } else {
                1.0
            };
            self.boxes.push(
                crate::nameplate_layout::PlateBox::new(
                    plate_id(plate.entity),
                    plate.preferred,
                    plate.size,
                )
                // The plate the player selected holds its ground and its neighbours give way:
                // the one plate they are certainly reading is the one that should not move.
                .with_weight(weight),
            );
        }
        let constraints = config.constraints(viewport);
        self.layout.resolve(&self.boxes, &constraints);
        let positions = self
            .layout
            .smooth(&self.boxes, delta_seconds, config.smoothing_per_second);
        for (plate, position) in self.plates.iter_mut().zip(positions) {
            // Whole pixels, so a plate settling onto its position does not shimmer.
            plate.origin = (position.0.round(), position.1.round());
        }
    }

    /// Dims everything that is not the target, while there is a fight on.
    ///
    /// Out of combat every plate is drawn in full: the setting is there to make a pull
    /// readable, and dimming the room a player is walking through would only make it harder
    /// to see what is in it.
    fn fade_non_targets(&mut self, simulation: &Simulation, config: &NameplateConfig) {
        if !player_in_combat(simulation) {
            return;
        }
        let alpha = config.non_target_alpha.clamp(0.0, 1.0);
        for plate in &mut self.plates {
            if !plate.is_target {
                plate.alpha *= alpha;
            }
        }
    }
}

/// What is on `entity`, for its plate: debuffs first, then buffs.
///
/// Debuffs lead because they are what the player put there and is keeping up — Ignite's
/// stacks, a bleed's clock — and the first icons are the ones never cut off by the limit.
fn plate_auras(simulation: &Simulation, entity: EntityId, out: &mut AuraList) {
    let all = crate::aura::auras_of(simulation, entity);
    out.clear();
    for aura in all.debuffs().chain(all.buffs()) {
        out.push(*aura);
    }
}

/// How much harder the target holds its position than an ordinary plate.
const TARGET_WEIGHT: f32 = 8.0;

/// How lightly a dead enemy's fading plate holds its position.
const DEAD_WEIGHT: f32 = 0.25;

/// One entity as the layout's opaque identity.
///
/// Slot and generation together, so a slot reused by a new spawn does not inherit the dead
/// occupant's smoothed position and slide its plate across the screen.
fn plate_id(entity: EntityId) -> u64 {
    ((entity.index() as u64) << 32) | u64::from(entity.generation())
}

/// Whether the player is in a fight right now.
///
/// Gameplay's own answer, not a second one: [`Simulation::player_in_combat`] is what decides
/// whether health regenerates, and a nameplate that dimmed on a different rule would be
/// telling the player something the game does not believe.
#[must_use]
pub fn player_in_combat(simulation: &Simulation) -> bool {
    simulation.player_in_combat()
}

/// Clip-space depth band nameplates are drawn in.
const NEAREST_PLATE_DEPTH: f32 = 0.010;

/// Depth of the furthest plate.
const FURTHEST_PLATE_DEPTH: f32 = 0.100;

/// Depth for a plate at `distance`.
///
/// Nearer is smaller, so a nearer plate would win a `LESS` test wherever two overlap.
fn depth_for(distance: f32, max_distance: f32) -> f32 {
    if max_distance <= 0.0 {
        return NEAREST_PLATE_DEPTH;
    }
    let fraction = (distance / max_distance).clamp(0.0, 1.0);
    NEAREST_PLATE_DEPTH + fraction * (FURTHEST_PLATE_DEPTH - NEAREST_PLATE_DEPTH)
}

/// Whether a plate's rectangle touches the viewport at all.
fn overlaps_viewport(origin: (f32, f32), size: (f32, f32), viewport: (u32, u32)) -> bool {
    let (x, y) = origin;
    let (plate_width, plate_height) = size;
    let (width, height) = (viewport.0 as f32, viewport.1 as f32);
    x + plate_width > 0.0 && y + plate_height > 0.0 && x < width && y < height
}

/// Projects a world-space point to pixels from the top-left of the viewport.
///
/// Returns `None` for a point that is behind the camera or outside the depth range. That
/// check is the whole reason this is a function rather than three lines at the call site: a
/// point behind the camera still divides to a finite position on screen, and drawing it
/// produces a mirrored ghost plate that follows the camera around — which is the classic
/// nameplate bug and is invisible in any test that only checks a point in front.
#[must_use]
pub fn project(point: Vec3, view_projection: &Mat4, viewport: (u32, u32)) -> Option<(f32, f32)> {
    let (width, height) = viewport;
    if width == 0 || height == 0 {
        return None;
    }

    let clip = *view_projection * point.extend(1.0);
    // Positive w is in front of the camera. Zero is the eye plane, which has no projection.
    if clip.w <= 0.0 {
        return None;
    }
    let ndc = clip.perspective_divide()?;
    // Depth in [0, 1] is the convention `Mat4::perspective` produces; outside it the point
    // is nearer than the near plane or beyond the far plane.
    if !(0.0..=1.0).contains(&ndc.z) {
        return None;
    }

    Some((
        (ndc.x * 0.5 + 0.5) * width as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32,
    ))
}

/// Projects with a `Vec4` already in clip space. Exposed for tests and for a caller that
/// has the clip-space point already.
#[must_use]
pub fn project_clip(clip: Vec4, viewport: (u32, u32)) -> Option<(f32, f32)> {
    if clip.w <= 0.0 || viewport.0 == 0 || viewport.1 == 0 {
        return None;
    }
    let ndc = clip.perspective_divide()?;
    Some((
        (ndc.x * 0.5 + 0.5) * viewport.0 as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport.1 as f32,
    ))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_enemy_debuff_size_scales_only_the_debuff_icons() {
        let plain = PlateGeometry::new(PlateMetrics::NORMAL, None, 80.0, 1.0, 2, 2, false);
        let larger = PlateGeometry::new(
            PlateMetrics::NORMAL.with_aura_scales(1.5, 1.0),
            None,
            80.0,
            1.0,
            2,
            2,
            false,
        );
        let edge = |row: Option<(f32, AuraRowStyle)>| row.map(|(_, style)| style.edge);
        assert!(
            edge(larger.debuffs) > edge(plain.debuffs),
            "the debuffs did not grow"
        );
        assert_eq!(
            edge(larger.buffs),
            edge(plain.buffs),
            "the buffs grew with them"
        );
        assert_eq!(
            larger.bar.width, plain.bar.width,
            "the plate itself was rescaled"
        );

        // From the setting through the configuration.
        let settings = engine_settings::NameplateSettings {
            debuff_size: 150.0,
            ..engine_settings::NameplateSettings::default()
        };
        let config = NameplateConfig::default().with_settings(settings);
        assert!((config.debuff_scale - 1.5).abs() < 1e-6);
        assert!((config.buff_scale - 1.0).abs() < 1e-6);
    }
    use crate::camera::ThirdPersonCamera;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::command::PlayerCommand;
    use engine_render_data::{Camera, Projection};

    const VIEWPORT: (u32, u32) = (1920, 1080);

    /// One frame of real time, for the layout's smoothing.
    const STEP: f32 = 1.0 / 60.0;

    /// The arena, with the camera behind the player looking at the dummies.
    fn scene() -> (Simulation, Camera) {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let controller = ThirdPersonCamera::following(simulation.player_position());
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");
        (simulation, camera)
    }

    fn config() -> NameplateConfig {
        NameplateConfig::default()
    }

    fn build(simulation: &Simulation, camera: &Camera, set: &mut NameplateSet) -> Vec<Nameplate> {
        set.build(
            simulation,
            &camera.view_projection(),
            VIEWPORT,
            &config(),
            STEP,
        )
        .to_vec()
    }

    #[test]
    fn a_pointer_on_a_plate_finds_the_enemy_it_belongs_to() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        // The nearest one: plates overlap in a group, and the near one is what the player
        // sees and therefore what a click on that patch of screen means.
        let plate = *plates
            .iter()
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
            .expect("the arena has dummies in front");

        // The middle of the plate, and each of its four corners.
        let centre = (
            plate.origin.0 + plate.size.0 * 0.5,
            plate.origin.1 + plate.size.1 * 0.5,
        );
        assert_eq!(set.at(centre).map(|found| found.entity), Some(plate.entity));
        for corner in [
            plate.origin,
            (plate.origin.0 + plate.size.0, plate.origin.1),
            (plate.origin.0, plate.origin.1 + plate.size.1),
            (plate.origin.0 + plate.size.0, plate.origin.1 + plate.size.1),
        ] {
            assert!(set.at(corner).is_some(), "{corner:?} is on the plate");
        }

        // A pixel outside it on every side is not on it.
        assert!(
            set.at((plate.origin.0 - 1.0, centre.1)).is_none()
                || set.at((plate.origin.0 - 1.0, centre.1)).map(|f| f.entity) != Some(plate.entity)
        );
        assert!(set.at((-50.0, -50.0)).is_none(), "nothing is off-screen");
    }

    #[test]
    fn the_nearest_plate_wins_where_two_overlap() {
        // Two plates on top of each other is the ordinary case in a pack, and the one the
        // player can read is the near one — which is the one the depth test draws on top.
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.len() > 1, "the arena has a group in it");

        for plate in &plates {
            let centre = (
                plate.origin.0 + plate.size.0 * 0.5,
                plate.origin.1 + plate.size.1 * 0.5,
            );
            let found = set.at(centre).expect("a plate is under its own centre");
            assert!(
                found.distance <= plate.distance,
                "a further plate was picked over a nearer one"
            );
        }
    }

    #[test]
    fn a_point_in_front_projects_into_the_viewport() {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        camera.set_position(Vec3::new(0.0, 0.0, 10.0));

        let (x, y) = project(Vec3::ZERO, &camera.view_projection(), VIEWPORT)
            .expect("the origin is in front of the camera");
        assert!((x - 960.0).abs() < 1.0, "{x}");
        assert!((y - 540.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn a_point_behind_the_camera_has_no_projection() {
        // The classic nameplate bug: a point behind the camera still divides to a finite
        // screen position, and drawing it produces a mirrored ghost that follows the camera.
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        camera.set_position(Vec3::new(0.0, 0.0, 10.0));

        assert_eq!(
            project(
                Vec3::new(0.0, 0.0, 20.0),
                &camera.view_projection(),
                VIEWPORT
            ),
            None
        );
        assert_eq!(project_clip(Vec4::new(1.0, 1.0, 1.0, -2.0), VIEWPORT), None);
    }

    #[test]
    fn screen_y_grows_downward() {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        camera.set_position(Vec3::new(0.0, 0.0, 10.0));
        let view_projection = camera.view_projection();

        let high = project(Vec3::new(0.0, 2.0, 0.0), &view_projection, VIEWPORT).expect("visible");
        let low = project(Vec3::new(0.0, -2.0, 0.0), &view_projection, VIEWPORT).expect("visible");
        assert!(high.1 < low.1, "{high:?} {low:?}");
    }

    #[test]
    fn a_degenerate_viewport_projects_nothing() {
        let camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        assert_eq!(
            project(Vec3::ZERO, &camera.view_projection(), (0, 1080)),
            None
        );
        assert_eq!(
            project(Vec3::ZERO, &camera.view_projection(), (1920, 0)),
            None
        );
    }

    #[test]
    fn every_nearby_enemy_gets_a_plate() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);

        // The arena's dummies are all within 40 m of the player and in front of the camera.
        assert_eq!(plates.len(), arena::dummies(simulation.world()).count());
        assert!(plates.iter().all(|plate| plate.name == "DUMMY"));
        assert!(plates.iter().all(|plate| plate.health_percent() == 100));
    }

    #[test]
    fn the_player_never_has_a_plate() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let player = simulation.player().entity;
        assert!(plates.iter().all(|plate| plate.entity != player));
    }

    #[test]
    fn only_the_current_target_is_outlined() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);

        let target = simulation.player().target.expect("a dummy is targeted");
        let outlined: Vec<EntityId> = plates
            .iter()
            .filter(|plate| plate.is_target)
            .map(|plate| plate.entity)
            .collect();
        assert_eq!(outlined, vec![target], "exactly one plate is the target");
    }

    #[test]
    fn an_enemy_beyond_the_distance_limit_has_no_plate() {
        let (mut simulation, camera) = scene();
        let far = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        if let Some(body) = simulation.world_mut().body_mut(far) {
            body.position = Vec3::new(0.0, 0.0, -80.0);
        }

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(
            plates.iter().all(|plate| plate.entity != far),
            "an enemy 80 m away kept its plate"
        );
        assert!(
            plates
                .iter()
                .all(|plate| plate.distance <= NAMEPLATE_MAX_DISTANCE),
            "a plate survived past the distance limit"
        );
    }

    #[test]
    fn the_limit_is_measured_from_the_player_not_the_camera() {
        // A camera pulled back must not change which enemies have plates.
        let (mut simulation, _) = scene();
        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        if let Some(body) = simulation.world_mut().body_mut(dummy) {
            body.position = Vec3::new(0.0, 0.0, -35.0);
        }

        let mut close = ThirdPersonCamera::following(simulation.player_position());
        close.distance = 3.0;
        let mut far = ThirdPersonCamera::following(simulation.player_position());
        far.distance = 25.0;

        let mut set = NameplateSet::new();
        let mut seen = Vec::new();
        for controller in [close, far] {
            let mut camera =
                Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
            controller
                .apply(&mut camera, 16.0 / 9.0)
                .expect("a valid aspect");
            let plates = build(&simulation, &camera, &mut set);
            seen.push(plates.iter().any(|plate| plate.entity == dummy));
        }
        assert_eq!(seen[0], seen[1], "the camera changed the distance rule");
        assert!(seen[0], "a dummy 35 m from the player should have a plate");
    }

    #[test]
    fn the_number_of_plates_is_bounded_and_the_nearest_survive() {
        // Each plate is text, and text is one quad per lit font cell. Without a bound, a
        // large pull would cost more than the frame it is drawn in.
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(200));
        simulation.step_once();
        let controller = ThirdPersonCamera::following(simulation.player_position());
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.len() <= MAX_NAMEPLATES, "{} plates", plates.len());

        // What survived is the nearest, not whatever came first out of the world.
        let kept: f32 = plates
            .iter()
            .map(|plate| plate.distance)
            .fold(0.0_f32, f32::max);
        let world = simulation.world();
        let player = simulation.player_position();
        let nearer_dropped = arena::dummies(world)
            .filter(|dummy| world.is_alive(*dummy))
            .filter_map(|dummy| world.position(dummy))
            .map(|position| engine_game::world::horizontal_distance(player, position))
            .filter(|distance| *distance < kept)
            .count();
        assert!(
            nearer_dropped <= plates.len(),
            "a nearer enemy was dropped in favour of a further one"
        );
    }

    #[test]
    fn a_dead_enemy_s_plate_fades_out_and_then_is_gone() {
        let (mut simulation, camera) = scene();
        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        simulation.world_mut().damage(dummy, 1_000_000.0);
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        // If the death was announced, the plate lingers a moment — empty, fading, and not
        // something a click can select; either way it never shows health.
        if let Some(plate) = plates.iter().find(|plate| plate.entity == dummy) {
            assert!(plate.dead, "a corpse's plate is marked dead");
            assert_eq!(plate.health_fraction, 0.0);
            assert!(plate.alpha <= 1.0);
            assert!(!plate.is_target);
            assert!(plate.cast.is_none());
            let centre = (
                plate.origin.0 + plate.size.0 * 0.5,
                plate.origin.1 + plate.size.1 * 0.5,
            );
            assert_ne!(set.at(centre).map(|found| found.entity), Some(dummy));
        }

        simulation.step_many(60);
        let plates = build(&simulation, &camera, &mut set);
        assert!(
            plates.iter().all(|plate| plate.entity != dummy),
            "the plate outlived its fade"
        );
    }

    #[test]
    fn an_enemy_behind_the_camera_has_no_plate() {
        let (simulation, _) = scene();
        let mut controller = ThirdPersonCamera::following(simulation.player_position());
        // Turn a half circle: the dummies are now behind the camera.
        controller.yaw = std::f32::consts::PI;
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.is_empty(), "{} ghost plates", plates.len());
    }

    #[test]
    fn health_is_reported_as_a_percentage_that_never_reads_zero_while_alive() {
        let (mut simulation, camera) = scene();
        let dummy = simulation.player().target.expect("a dummy is targeted");
        let max = simulation
            .world()
            .health(dummy)
            .expect("the dummy exists")
            .max();
        simulation.world_mut().damage(dummy, max * 0.75);

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == dummy)
            .expect("the target has a plate");
        assert_eq!(plate.health_percent(), 25);

        // One point of health left: a plate that rounds to 0% says it is dead.
        simulation.world_mut().damage(dummy, max * 0.25 - 1.0);
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == dummy)
            .expect("the target has a plate");
        assert_eq!(plate.health_percent(), 1);
    }

    #[test]
    fn the_health_text_prints_in_the_form_the_player_chose() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let mut plate = plates[0];
        plate.health = 12_400.0;
        plate.max_health = 20_000.0;
        plate.health_fraction = 0.62;
        let mut text = String::new();

        assert!(plate.write_health_text(NameplateHealthText::Percent, &mut text));
        assert_eq!(text, "62%");
        assert!(plate.write_health_text(NameplateHealthText::Value, &mut text));
        assert_eq!(text, "12K");
        assert!(plate.write_health_text(NameplateHealthText::Both, &mut text));
        assert_eq!(text, "12K \u{00B7} 62%");
        assert!(!plate.write_health_text(NameplateHealthText::None, &mut text));
        assert!(text.is_empty());

        plate.health = 1_530.0;
        assert!(plate.write_health_text(NameplateHealthText::Value, &mut text));
        assert_eq!(text, "1.5K");
        plate.health = 430.2;
        assert!(plate.write_health_text(NameplateHealthText::Value, &mut text));
        assert_eq!(text, "431", "a sliver of health is never rounded away");

        plate.dead = true;
        assert!(!plate.write_health_text(NameplateHealthText::Percent, &mut text));
    }

    #[test]
    fn the_effects_on_an_enemy_are_shown_as_icons_under_its_health_bar() {
        let (mut simulation, camera) = scene();
        let burning = simulation.player().target.expect("a dummy is targeted");
        // Ignite is the class's burn: it lives in its own tracker, not the status set.
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(burning, 2, maximum, duration);
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == burning)
            .expect("the burning dummy has a plate");

        assert!(plate.has_auras());
        let ignite = plate.auras.iter().next().expect("the burn is listed");
        assert_eq!(ignite.kind, crate::aura::AuraKind::Ignite);
        assert_eq!(ignite.stacks, 2);
        assert!(ignite.remaining > 0.0);
        // A debuff sits over the name: above the bar, and above the name's line box.
        let ((_, top), style) = plate
            .debuff_row()
            .expect("a debuffed plate has a row on top");
        assert!(top + style.edge <= plate.origin.1 + plate.geometry.name_top);
        assert!(top < plate.bar_rect().y);
        assert!(style.edge >= 20.0);
        assert!(plate.buff_row().is_none(), "a burn is not a buff");

        // An unaffected dummy has no row at all, so its plate is shorter.
        let plain = plates
            .iter()
            .find(|plate| plate.entity != burning)
            .expect("the arena has other dummies");
        assert!(!plain.has_auras());
        assert!(plain.debuff_row().is_none() && plain.buff_row().is_none());
        assert!(plain.size.1 < plate.size.1, "an effect adds a row");
        // The same plate laid out without the row has its health bar exactly where it was.
        let metrics = plate.rank.metrics();
        let name_width = UiFont::SemiBold.width(
            &crate::ui::map_style::title_case(plate.name),
            metrics.name_size,
        );
        let bare = PlateGeometry::new(
            metrics,
            plate.rank.crest(),
            name_width,
            plate.geometry.scale,
            0,
            0,
            false,
        );
        // The row on top pushes the name and the bar down inside the plate by exactly its own
        // height, and the plate is placed by its bar: on screen the bar has not moved.
        let row = style.height_for(1) + (3.0 * plate.geometry.scale).round().max(1.0);
        assert!(
            (plate.geometry.anchor_depth() - bare.anchor_depth() - row).abs() < 0.01,
            "the debuff row is not all that was added above the bar"
        );

        // A buff on the enemy goes under the bar instead.
        let with_buff = PlateGeometry::new(
            metrics,
            plate.rank.crest(),
            name_width,
            plate.geometry.scale,
            0,
            1,
            false,
        );
        let (buff_top, _) = with_buff.buffs.expect("a buff row");
        assert!(buff_top >= with_buff.bar.bottom());
        assert_eq!(with_buff.anchor_depth(), bare.anchor_depth());
    }

    #[test]
    fn the_aura_row_can_be_turned_off_in_the_settings() {
        let (mut simulation, camera) = scene();
        let burning = simulation.player().target.expect("a dummy is targeted");
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(burning, 2, maximum, duration);
        simulation.step_once();

        let settings = engine_settings::NameplateSettings {
            auras: false,
            ..engine_settings::NameplateSettings::default()
        };
        let config = NameplateConfig::default().with_settings(settings);
        assert!(!config.auras);
        let mut set = NameplateSet::new();
        set.build(
            &simulation,
            &camera.view_projection(),
            VIEWPORT,
            &config,
            0.016,
        );
        let plate = set
            .plates()
            .iter()
            .find(|plate| plate.entity == burning)
            .expect("the burning dummy has a plate");
        assert!(!plate.has_auras() && plate.debuff_row().is_none());
    }

    #[test]
    fn every_character_a_plate_can_emit_has_a_glyph() {
        // The interface fonts cover exactly their charset; anything else is drawn as a
        // question mark.
        let known = |character: char| {
            character == '?'
                || engine_render_data::font::glyph_index(character)
                    != engine_render_data::font::glyph_index('?')
        };
        let (mut simulation, camera) = scene();
        simulation.push_command(PlayerCommand::UseAbility(1));
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let mut line = String::new();
        for plate in &plates {
            crate::ui::map_style::write_title_case(&mut line, plate.name);
            assert!(line.chars().all(known), "{line:?}");
            for mode in NameplateHealthText::ALL {
                plate.write_health_text(mode, &mut line);
                assert!(line.chars().all(known), "{line:?}");
            }
            for aura in plate.auras.iter() {
                assert!(aura.kind.name().chars().all(known), "{:?}", aura.kind);
                assert!(
                    aura.kind.description().chars().all(known),
                    "{:?}",
                    aura.kind
                );
                crate::aura::write_remaining(&mut line, aura);
                assert!(line.chars().all(known), "{line:?}");
            }
        }
    }

    #[test]
    fn a_nearer_plate_is_drawn_in_front_of_a_further_one() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);

        for pair in plates.windows(2) {
            // Ordered furthest first, so depth must decrease along the list.
            assert!(pair[0].distance >= pair[1].distance, "order");
            assert!(
                pair[0].depth >= pair[1].depth,
                "depth does not follow distance"
            );
        }
        assert!(
            plates
                .iter()
                .all(|plate| plate.depth > 0.0 && plate.depth < 0.5)
        );
    }

    #[test]
    fn plates_are_anchored_above_the_enemy_and_centred_on_it() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let config = config();
        let plate = plates
            .iter()
            .find(|plate| plate.is_target)
            .expect("the target has a plate");

        let world = simulation.world();
        let head = world.position(plate.entity).expect("the dummy exists")
            + Vec3::new(0.0, world.height(plate.entity) + config.height_offset, 0.0);
        let (anchor_x, anchor_y) =
            project(head, &camera.view_projection(), VIEWPORT).expect("the head is visible");

        // The *preferred* position is the projection: that is the plate's ideal, and the
        // layout's whole job is to give up as little of it as it can.
        // Whole pixels, so within one of the exact projection.
        assert!(
            (plate.preferred.0 + plate.size.0 * 0.5 - anchor_x).abs() <= 1.0,
            "not centred: {plate:?}"
        );
        // The health bar's bottom edge is what sits on the anchor.
        assert!(
            (plate.preferred.1 + plate.geometry.anchor_depth() - anchor_y).abs() <= 1.0,
            "the health bar is not sitting above the head: {plate:?}"
        );
    }

    /// How far a plate ended up from where it wanted to be.
    fn displacement(plate: &Nameplate) -> f32 {
        let dx = plate.origin.0 - plate.preferred.0;
        let dy = plate.origin.1 - plate.preferred.1;
        (dx * dx + dy * dy).sqrt()
    }

    /// Whether two plates' rectangles touch.
    fn plates_overlap(a: &Nameplate, b: &Nameplate) -> bool {
        a.origin.0 < b.origin.0 + b.size.0
            && b.origin.0 < a.origin.0 + a.size.0
            && a.origin.1 < b.origin.1 + b.size.1
            && b.origin.1 < a.origin.1 + a.size.1
    }

    #[test]
    fn the_plates_of_a_crowd_do_not_overlap_and_stay_near_their_monsters() {
        // The arena's group stands close enough together that their preferred positions
        // collide, which is the case the layout exists for.
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(plates.len() > 2, "the arena has a group in it");

        for i in 0..plates.len() {
            for j in (i + 1)..plates.len() {
                assert!(
                    !plates_overlap(&plates[i], &plates[j]),
                    "two plates overlap: {:?} and {:?}",
                    plates[i].origin,
                    plates[j].origin
                );
            }
        }
        // And none of them was flung anywhere: every plate is still obviously its monster's.
        for plate in &plates {
            let limit = plate.size.1 * crate::nameplate_layout::DEFAULT_MAX_DISPLACEMENT;
            assert!(
                displacement(plate) <= limit + 0.5,
                "a plate is {} px from its monster",
                displacement(plate)
            );
        }
    }

    #[test]
    fn a_plate_with_nothing_near_it_is_left_exactly_where_its_monster_is() {
        // One enemy, alone: the layout must be a no-op. The rule the whole design turns on.
        let (mut simulation, camera) = scene();
        let dummies: Vec<EntityId> = arena::dummies(simulation.world()).collect();
        for dummy in dummies.iter().skip(1) {
            simulation.world_mut().damage(*dummy, 1_000_000.0);
        }
        simulation.step_once();

        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert_eq!(plates.len(), 1, "only one dummy is left alive");
        assert_eq!(plates[0].origin, plates[0].preferred);
    }

    #[test]
    fn no_plate_enters_the_top_safe_zone() {
        // The dummies are put up in the air, so their plates project into the top of the
        // screen and have to be pushed out of it.
        let (mut simulation, camera) = scene();
        for dummy in arena::dummies(simulation.world()).collect::<Vec<_>>() {
            if let Some(body) = simulation.world_mut().body_mut(dummy) {
                body.position.y += 6.0;
            }
        }

        let config = NameplateConfig::default();
        let mut set = NameplateSet::new();
        let plates = set
            .build(
                &simulation,
                &camera.view_projection(),
                VIEWPORT,
                &config,
                STEP,
            )
            .to_vec();
        assert!(!plates.is_empty(), "the dummies are still on screen");

        let safe = VIEWPORT.1 as f32 * config.top_safe_zone;
        for plate in &plates {
            assert!(
                plate.origin.1 >= safe - 0.01,
                "a plate is at {} with a {safe} px safe zone",
                plate.origin.1
            );
        }
    }

    #[test]
    fn the_height_setting_moves_the_anchor_and_the_size_setting_moves_the_box() {
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();

        let low =
            NameplateConfig::default().with_settings(engine_settings::NameplateSettings::default());
        let high = NameplateConfig::default().with_settings(
            engine_settings::NameplateSettings::default()
                .with(engine_settings::NameplateSetting::HeightAboveHead, 16.0),
        );
        let plate_at = |set: &mut NameplateSet, config: &NameplateConfig| {
            set.build(
                &simulation,
                &camera.view_projection(),
                VIEWPORT,
                config,
                STEP,
            )
            .iter()
            .find(|plate| plate.is_target)
            .copied()
            .expect("the target has a plate")
        };

        let near = plate_at(&mut set, &low);
        let far = plate_at(&mut set, &high);
        assert!(
            far.preferred.1 < near.preferred.1,
            "raising the height did not raise the plate: {} then {}",
            near.preferred.1,
            far.preferred.1
        );

        // The size setting reaches the plate through its scale, so a bigger size is a bigger
        // box: every part of it, bar and name alike.
        let big = low.at_scale(
            1.0,
            engine_settings::NameplateSettings::default()
                .with(engine_settings::NameplateSetting::Size, 22.0),
        );
        let large = plate_at(&mut set, &big);
        let small = plate_at(&mut set, &low);
        assert!(
            large.size.0 > small.size.0 * 1.5,
            "{:?} then {:?}",
            small.size,
            large.size
        );
        assert!(large.geometry.bar.height > small.geometry.bar.height);
        assert!(large.geometry.name_size > small.geometry.name_size);
    }

    #[test]
    fn only_the_target_keeps_full_opacity_and_only_while_there_is_a_fight() {
        // Out of combat every plate is drawn in full: the setting exists to make a pull
        // readable, not to dim a room the player is walking through.
        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        assert!(
            plates.iter().all(|plate| plate.alpha == 1.0),
            "a plate was dimmed out of combat"
        );

        // A real fight: an encounter, walked into, until its enemies are chasing.
        let config = ArenaConfig::with_group(0);
        let mut simulation = engine_game::arena::encounter_arena(
            &config,
            engine_game::arena::pack_encounter(&config, 3),
        );
        simulation.push_command(PlayerCommand::Teleport {
            position: simulation.player_position() + Vec3::new(0.0, 0.0, -14.0),
        });
        for _ in 0..120 {
            simulation.step_once();
        }
        assert!(
            crate::nameplate::player_in_combat(&simulation),
            "the pack never engaged"
        );
        simulation.select_nearest_target();

        let controller = ThirdPersonCamera::following(simulation.player_position());
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid");
        controller
            .apply(&mut camera, 16.0 / 9.0)
            .expect("a valid aspect");

        let config = NameplateConfig::default();
        let plates = set
            .build(
                &simulation,
                &camera.view_projection(),
                VIEWPORT,
                &config,
                STEP,
            )
            .to_vec();
        let target = plates.iter().find(|plate| plate.is_target);
        assert!(target.is_some(), "nothing is targeted");
        for plate in &plates {
            let expected = if plate.is_target {
                1.0
            } else {
                config.non_target_alpha
            };
            assert!(
                (plate.alpha - expected).abs() < 1e-6,
                "{:?} is drawn at {} rather than {expected}",
                plate.entity,
                plate.alpha
            );
        }
    }

    #[test]
    fn a_plate_follows_its_monster_and_does_not_drift_behind_it() {
        // The property a system that stored its resolved position would lose. The dummy walks
        // across the screen for ten seconds of frames; its plate stays on it throughout.
        let (mut simulation, camera) = scene();
        let dummy = simulation.player().target.expect("a dummy is targeted");
        let mut set = NameplateSet::new();

        for _ in 0..600 {
            if let Some(body) = simulation.world_mut().body_mut(dummy) {
                body.position.x += 0.02;
            }
            let plates = set
                .build(
                    &simulation,
                    &camera.view_projection(),
                    VIEWPORT,
                    &NameplateConfig::default(),
                    STEP,
                )
                .to_vec();
            let plate = plates
                .iter()
                .find(|plate| plate.entity == dummy)
                .expect("the dummy has a plate");
            let limit = plate.size.1 * crate::nameplate_layout::DEFAULT_MAX_DISPLACEMENT;
            assert!(
                displacement(plate) <= limit + 0.5,
                "the plate drifted {} px from its monster",
                displacement(plate)
            );
        }
    }

    #[test]
    fn rebuilding_reuses_the_allocation() {
        let (mut simulation, camera) = scene();
        let mut set = NameplateSet::new();
        build(&simulation, &camera, &mut set);
        let capacity = set.capacity();

        for _ in 0..600 {
            simulation.push_command(PlayerCommand::Move {
                forward: 1.0,
                right: 0.2,
                yaw: 0.1,
                face_camera: false,
            });
            simulation.step_once();
            set.build(
                &simulation,
                &camera.view_projection(),
                VIEWPORT,
                &config(),
                STEP,
            );
        }
        assert_eq!(set.capacity(), capacity, "the plate list reallocated");
    }

    /// The arena with an Ember Adept a few metres in front of the player, stepped until it is
    /// casting. Returns the caster.
    fn casting_adept(simulation: &mut Simulation) -> EntityId {
        let origin = simulation.player_position();
        let adept = simulation.spawn_enemy(
            engine_game::enemy::EnemyKind::Caster,
            origin + Vec3::new(1.5, 0.0, -9.0),
            0.0,
        );
        for _ in 0..(20 * 60) {
            simulation.step_once();
            if simulation
                .brain(adept)
                .is_some_and(|brain| brain.cast().is_some_and(|cast| cast.elapsed > 0.2))
            {
                return adept;
            }
        }
        panic!("the adept never started a cast");
    }

    #[test]
    fn a_boss_held_immune_shows_it_on_its_plate_aura_and_target_frame_until_it_is_not() {
        use engine_game::ability::AbilityId;
        use engine_game::status::{Status, StatusKind};
        let (mut simulation, camera) = scene();
        let origin = simulation.player_position();
        let boss = simulation.spawn_enemy(
            engine_game::enemy::EnemyKind::AshenPontiff,
            origin + Vec3::new(0.0, 0.0, -10.0),
            0.0,
        );
        let mut set = NameplateSet::new();
        let immune = |simulation: &Simulation, set: &mut NameplateSet| {
            build(simulation, &camera, set)
                .iter()
                .find(|plate| plate.entity == boss)
                .map(|plate| plate.immune)
        };
        assert_eq!(
            immune(&simulation, &mut set),
            Some(false),
            "plain plate first"
        );

        let status = Status::new(
            StatusKind::Shielded,
            EntityId::INVALID,
            AbilityId::NONE,
            600.0,
            0.0,
        );
        simulation
            .world_mut()
            .statuses_mut(boss)
            .expect("a boss has statuses")
            .apply(status);
        assert_eq!(
            immune(&simulation, &mut set),
            Some(true),
            "the plate is tinted"
        );
        // The aura row and the target frame say it too, through the same status.
        let auras = crate::aura::auras_of(&simulation, boss);
        let shield = auras
            .iter()
            .find(|aura| aura.kind == crate::aura::AuraKind::Status(StatusKind::Shielded))
            .expect("the aura row lists the shield");
        assert_eq!(
            shield.kind.icon(),
            engine_render_data::ui::UiIcon::AuraShielded
        );
        assert!(shield.kind.description().contains("pylons"));
        simulation.push_command(PlayerCommand::SelectTarget { entity: boss });
        simulation.step_once();
        // (A tick with no pylons standing clears the status, as it should: put it back.)
        simulation
            .world_mut()
            .statuses_mut(boss)
            .expect("statuses")
            .apply(status);
        let snapshot = crate::snapshot::UiSnapshot::build(&simulation);
        assert!(snapshot.target.as_ref().is_some_and(|target| target.immune));

        // And it goes when the status does.
        simulation
            .world_mut()
            .statuses_mut(boss)
            .expect("statuses")
            .consume(StatusKind::Shielded);
        assert_eq!(
            immune(&simulation, &mut set),
            Some(false),
            "the tint is gone"
        );
        let auras = crate::aura::auras_of(&simulation, boss);
        assert!(
            auras
                .iter()
                .all(|aura| aura.kind != crate::aura::AuraKind::Status(StatusKind::Shielded))
        );
    }

    #[test]
    fn a_cast_bar_reads_the_enemy_s_own_cast_and_nothing_else() {
        let (mut simulation, camera) = scene();
        let adept = casting_adept(&mut simulation);
        let mut set = NameplateSet::new();
        let plates = build(&simulation, &camera, &mut set);
        let plate = plates
            .iter()
            .find(|plate| plate.entity == adept)
            .expect("the adept has a plate");
        let cast = plate.cast.expect("its plate shows the cast");
        let truth = simulation
            .brain(adept)
            .and_then(|brain| brain.cast())
            .expect("it is casting");
        // The same numbers the AI is counting down: no second timer.
        assert_eq!(cast.ability, truth.ability);
        assert_eq!(cast.fraction, truth.fraction());
        assert_eq!(cast.remaining, truth.remaining());
        assert_eq!(cast.interruptible, truth.interruptible);
        assert_eq!(cast.state, PlateCastState::Casting);
        // Hanging under the health bar, as wide as it.
        let (bar, rect) = (plate.bar_rect(), plate.cast_rect().expect("a cast bar"));
        assert!(rect.y >= bar.bottom() && rect.width == bar.width);
        // The name is the monster's own, not the entity kind's.
        assert_eq!(plate.name, "EMBER ADEPT");

        // Cast bars switched off: the same plate, collapsed.
        let off = NameplateConfig {
            cast_bars: false,
            ..config()
        };
        let plates = set
            .build(&simulation, &camera.view_projection(), VIEWPORT, &off, STEP)
            .to_vec();
        let plate = plates
            .iter()
            .find(|plate| plate.entity == adept)
            .expect("the adept has a plate");
        assert!(plate.cast.is_none() && plate.cast_rect().is_none());
    }

    #[test]
    fn an_interrupted_cast_holds_red_where_it_stopped_and_then_comes_down() {
        let (mut simulation, camera) = scene();
        let adept = casting_adept(&mut simulation);
        let interruptible = |simulation: &Simulation| {
            simulation
                .brain(adept)
                .is_some_and(engine_game::enemy::brain::EnemyBrain::is_interruptible)
        };
        // Its first cast may be the one that cannot be interrupted; step to one that can.
        for _ in 0..(20 * 60) {
            if interruptible(&simulation) {
                break;
            }
            simulation.step_once();
        }
        assert!(
            interruptible(&simulation),
            "the adept never cast anything interruptible"
        );
        let mut set = NameplateSet::new();
        let before = build(&simulation, &camera, &mut set)
            .into_iter()
            .find(|plate| plate.entity == adept)
            .and_then(|plate| plate.cast)
            .expect("a cast to interrupt");
        // Interrupted here, between steps, so its announcement is in this frame's events:
        // in play the interrupting ability runs inside a step and the same event arrives the
        // same way.
        assert!(simulation.interrupt(adept), "the cast could be interrupted");

        let after = build(&simulation, &camera, &mut set)
            .into_iter()
            .find(|plate| plate.entity == adept)
            .and_then(|plate| plate.cast)
            .expect("the bar stays up to say it was interrupted");
        assert_eq!(after.state, PlateCastState::Interrupted);
        assert_eq!(after.ability, before.ability);
        assert!(
            (after.fraction - before.fraction).abs() < 0.05,
            "frozen where it stopped: {} then {}",
            before.fraction,
            after.fraction
        );
        // Still up a moment later, after the event itself has gone from the stream.
        simulation.step_many(10);
        let held = build(&simulation, &camera, &mut set)
            .into_iter()
            .find(|plate| plate.entity == adept)
            .and_then(|plate| plate.cast)
            .expect("the interrupted bar is held for its moment");
        assert_eq!(held.state, PlateCastState::Interrupted);

        simulation.step_many(((INTERRUPT_SHOW_SECONDS + 0.1) * 60.0) as u32);
        let later = build(&simulation, &camera, &mut set)
            .into_iter()
            .find(|plate| plate.entity == adept)
            .and_then(|plate| plate.cast);
        assert!(
            later.is_none_or(|cast| cast.state == PlateCastState::Casting),
            "the interrupted bar outstayed its moment: {later:?}"
        );
    }

    #[test]
    fn rank_comes_from_the_monster_s_table_not_its_name() {
        use engine_game::enemy::EnemyKind;
        let (mut simulation, _) = scene();
        let origin = simulation.player_position();
        let boss =
            simulation.spawn_enemy(EnemyKind::Boss, origin + Vec3::new(0.0, 0.0, -30.0), 0.0);
        let elite = simulation.spawn_enemy(
            EnemyKind::Revenant,
            origin + Vec3::new(4.0, 0.0, -30.0),
            0.0,
        );
        let pack =
            simulation.spawn_enemy(EnemyKind::Melee, origin + Vec3::new(8.0, 0.0, -30.0), 0.0);
        assert_eq!(PlateRank::of(&simulation, boss), PlateRank::Boss);
        assert_eq!(PlateRank::of(&simulation, elite), PlateRank::Elite);
        assert_eq!(PlateRank::of(&simulation, pack), PlateRank::Normal);
        let dummy = arena::dummies(simulation.world()).next().expect("a dummy");
        assert_eq!(PlateRank::of(&simulation, dummy), PlateRank::Normal);

        // Size carries rank as well as colour, and each step up has a crest.
        let (normal, elite, boss) = (
            PlateMetrics::NORMAL,
            PlateMetrics::ELITE,
            PlateMetrics::BOSS,
        );
        assert!(normal.bar_width < elite.bar_width && elite.bar_width < boss.bar_width);
        assert!(normal.name_size < boss.name_size);
        assert!(PlateRank::Normal.crest().is_none());
        assert!(PlateRank::Elite.crest().is_some() && PlateRank::Boss.crest().is_some());
    }

    #[test]
    fn a_hit_leaves_a_trail_that_holds_and_then_drains_to_the_health() {
        let (mut simulation, camera) = scene();
        let dummy = simulation.player().target.expect("a dummy is targeted");
        let mut set = NameplateSet::new();
        let find = |plates: &[Nameplate]| {
            *plates
                .iter()
                .find(|plate| plate.entity == dummy)
                .expect("the target has a plate")
        };
        let first = find(&build(&simulation, &camera, &mut set));
        assert_eq!(
            first.trail_fraction, first.health_fraction,
            "nothing to trail yet"
        );

        let max = simulation.world().health(dummy).expect("exists").max();
        simulation.world_mut().damage(dummy, max * 0.4);
        let hit = find(&build(&simulation, &camera, &mut set));
        assert!(
            (hit.health_fraction - 0.6).abs() < 0.01,
            "the fill moves at once"
        );
        assert!(
            hit.trail_fraction > 0.95,
            "the trail holds where the health was"
        );

        // A second later the trail has drained onto the health.
        for _ in 0..60 {
            build(&simulation, &camera, &mut set);
        }
        let settled = find(&build(&simulation, &camera, &mut set));
        assert!((settled.trail_fraction - settled.health_fraction).abs() < 1e-4);
    }

    #[test]
    fn a_distant_plate_is_a_little_smaller_and_stacking_can_be_turned_off() {
        let config = config();
        assert_eq!(config.distance_scale(5.0), 1.0);
        assert!(config.distance_scale(config.max_distance) < 0.9);
        assert!(config.distance_scale(config.max_distance) >= config.far_scale - 1e-5);

        let (simulation, camera) = scene();
        let mut set = NameplateSet::new();
        let loose = NameplateConfig {
            stacking: false,
            ..config
        };
        let plates = set
            .build(
                &simulation,
                &camera.view_projection(),
                VIEWPORT,
                &loose,
                STEP,
            )
            .to_vec();
        assert!(
            plates.iter().all(|plate| plate.origin == plate.preferred),
            "without stacking every plate sits exactly above its monster"
        );
        let hidden = NameplateConfig {
            enabled: false,
            ..config
        };
        assert!(
            set.build(
                &simulation,
                &camera.view_projection(),
                VIEWPORT,
                &hidden,
                STEP
            )
            .is_empty()
        );
    }
}
