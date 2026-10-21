//! Tooltips: one data model, one panel, one fixed place on the screen.
//!
//! ```text
//! TooltipModel  title · tag · meta rows · sections · footer        what to say
//!      │            sections: Description, Resource, Empowered Effect,
//!      │                      Additional Effect, Requirements, Details
//!      ▼
//! TooltipPanel  wraps every section to the panel's width, grows to fit,  how to show it
//!               docks at HudElementId::Tooltip and grows away from it
//! ```
//!
//! ## Spells at a fixed dock, auras at the pointer
//!
//! An ability tooltip that followed the pointer would land on top of the bar being read and
//! jump every time the pointer moved between buttons, so it opens from a dock (bottom right
//! by default), which the player can move, widen and point in another direction in Edit HUD
//! like any other element. A buff or debuff icon is small and far from that dock, so its
//! tooltip opens beside the pointer instead — see [`pointer_dock`] — at the dock's width.
//!
//! ## Text is wrapped, never clipped
//!
//! Every section is wrapped to the panel's inner width at its own type size and the panel is
//! as tall as what it holds. The width is the player's (a readable column, 380 logical pixels
//! by default); the height is the content's. A panel taller than the screen is held inside it
//! by the clamp, top edge first.
//!
//! ## Nothing per frame
//!
//! The model is rebuilt into reused strings each frame and wrapped only when its text or its
//! width changes — a fingerprint of both is kept — so holding the pointer over a button costs
//! no allocation.

use std::hash::{Hash, Hasher};

use engine_game::ability::text::{self as ability_text, ResourceMode};
use engine_game::character::stat::{HEALTH_PER_STAMINA, armor_reduction};
use engine_game::character::equipment::INVENTORY_SLOTS;
use engine_game::character::{
    BROKEN_PENALTY, EquipSlot, ItemProps, ItemSlot, ItemStack, SetBonus, SetDef, SetEffect, Stat,
    StatBlock, active_sets_of, item_by_key, set_by_id,
};
use engine_game::firemage::{FireAbility, FireMastery, TargetMode};
use engine_game::hero::Hero;
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect};

use super::flow::FlowDirection;
use super::kit::{Kit, tokens};
use super::map_style::{TextStyle, color, type_scale};
use crate::game_ui::{write_integer, write_tenths};
use crate::snapshot::{AbilitySlotView, CharacterView};

/// A tooltip's width when the player has not set one, logical pixels.
pub const DEFAULT_WIDTH: f32 = 380.0;

/// The dock's height in Edit HUD, logical pixels: a tooltip's opening edge, not its size.
pub const DOCK_HEIGHT: f32 = 64.0;

/// Sections a tooltip may hold.
const MAX_SECTIONS: usize = 12;

/// What a section is, which decides its heading and its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionKind {
    /// What the thing does. No heading: it is the tooltip.
    Description,
    /// Numbers: the damage or healing it does right now.
    Effect,
    /// What it costs, needs, consumes or generates.
    Resource,
    /// What spending a resource on it adds.
    Empowered,
    /// A further effect worth its own heading.
    Additional,
    /// What must be true for it to be used.
    Requirements,
    /// Time left, stacks, an absorb's size: the state of an effect that is running.
    Details,
    /// An item's stat lines, in the body size and the stat green.
    Stats,
    /// An item copy's quality, sharpening and total, or that it is broken.
    Quality,
    /// What an item's stats are worth to the character looking at it.
    Numbers,
    /// An item against the one it would replace; lines starting `+` are green, `-` red.
    Comparison,
    /// An item's equipment set: its name and how much of it is worn.
    SetName,
    /// Set pieces, worn first (green) then the rest (muted); one section per state.
    SetPiece,
    /// Set bonus tiers: the active ones (green) then the ones still to earn (muted).
    SetBonus,
    /// An item's gem sockets: a hollow marker for an empty one, the gem and its stat for a
    /// filled one.
    Sockets,
}

impl SectionKind {
    /// The heading the section is printed under, if it has one.
    #[must_use]
    pub fn heading(self) -> Option<&'static str> {
        match self {
            Self::Description
            | Self::Effect
            | Self::Details
            | Self::Stats
            | Self::Quality
            | Self::SetName
            | Self::SetPiece
            | Self::SetBonus
            | Self::Sockets => None,
            Self::Numbers => Some("WHAT IT DOES FOR YOU"),
            Self::Comparison => Some("COMPARED TO EQUIPPED"),
            Self::Resource => Some("RESOURCE"),
            Self::Empowered => Some("EMPOWERED EFFECT"),
            Self::Additional => Some("ADDITIONAL EFFECT"),
            Self::Requirements => Some("REQUIREMENTS"),
        }
    }

    /// The heading's colour.
    #[must_use]
    pub fn accent(self) -> UiColor {
        match self {
            Self::Resource => color::FROST,
            Self::Empowered => color::AMBER,
            Self::Requirements => color::EMBER,
            Self::Stats => color::VERDANT,
            Self::Quality => color::BRONZE_BRIGHT,
            Self::Numbers | Self::Comparison | Self::SetName => color::BRONZE_BRIGHT,
            _ => color::TEXT_MUTED,
        }
    }

    /// The body's type.
    #[must_use]
    pub fn style(self) -> TextStyle {
        match self {
            Self::Effect
            | Self::Details
            | Self::Numbers
            | Self::Comparison
            | Self::Quality
            | Self::SetName
            | Self::SetPiece
            | Self::SetBonus
            | Self::Sockets => tokens::TOOLTIP_META,
            _ => tokens::TOOLTIP_BODY,
        }
    }
}

/// One section of a tooltip.
#[derive(Debug, Clone, Default)]
pub struct TooltipSection {
    /// What it is.
    pub kind: Option<SectionKind>,
    /// Its text. `\n` starts a new paragraph line.
    pub text: String,
    /// The text's colour.
    pub color: UiColor,
    /// Whether it is the part that applies *right now*: drawn with an accent bar.
    pub highlight: bool,
}

/// Everything a tooltip says, as data.
///
/// Generic: an ability, a buff and anything later fill the same fields, and the panel draws
/// them the same way. The strings are kept between frames and rewritten in place.
#[derive(Debug, Clone, Default)]
pub struct TooltipModel {
    /// The name.
    pub title: String,
    /// The name's colour.
    pub title_color: UiColor,
    /// A short tag to the right of the name: `Buff`, `Magic`, a keybind.
    pub tag: String,
    /// The tag's colour.
    pub tag_color: UiColor,
    /// Up to two rows of left- and right-aligned facts: cast time and cooldown, range and
    /// radius.
    pub meta: [(String, String); 2],
    /// The sections, in order; only the first `len` are live.
    sections: Vec<TooltipSection>,
    len: usize,
    /// A muted last line.
    pub footer: String,
    /// The panel's edge.
    pub border: UiColor,
}

impl TooltipModel {
    /// An empty model with its storage reserved.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sections: (0..MAX_SECTIONS)
                .map(|_| TooltipSection::default())
                .collect(),
            ..Self::default()
        }
    }

    /// Empties the model for a new tooltip, keeping every allocation.
    pub fn clear(&mut self) {
        self.title.clear();
        self.title_color = color::TEXT_TITLE;
        self.tag.clear();
        self.tag_color = color::TEXT_SECONDARY;
        for (left, right) in &mut self.meta {
            left.clear();
            right.clear();
        }
        self.len = 0;
        self.footer.clear();
        self.border = color::alpha([1.0, 1.0, 1.0, 1.0], 0.12);
        if self.sections.len() < MAX_SECTIONS {
            self.sections
                .resize_with(MAX_SECTIONS, TooltipSection::default);
        }
    }

    /// Starts a section and returns it to be written into. Past the limit the last section
    /// is reused rather than growing.
    pub fn section(&mut self, kind: SectionKind) -> &mut TooltipSection {
        if self.sections.len() < MAX_SECTIONS {
            self.sections
                .resize_with(MAX_SECTIONS, TooltipSection::default);
        }
        let index = self.len.min(MAX_SECTIONS - 1);
        self.len = (self.len + 1).min(MAX_SECTIONS);
        let section = &mut self.sections[index];
        section.kind = Some(kind);
        section.text.clear();
        section.color = color::TEXT_PRIMARY;
        section.highlight = false;
        section
    }

    /// Gives back the section most recently started, for one that turned out to be empty.
    fn drop_last_section(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    /// The live sections.
    pub fn sections(&self) -> impl Iterator<Item = &TooltipSection> {
        self.sections.iter().take(self.len)
    }

    /// The live section of `kind`, if there is one.
    #[must_use]
    pub fn find(&self, kind: SectionKind) -> Option<&TooltipSection> {
        self.sections().find(|section| section.kind == Some(kind))
    }

    /// A fingerprint of everything that changes the panel's layout.
    fn fingerprint(&self, width: f32, scale: f32) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        width.to_bits().hash(&mut hasher);
        scale.to_bits().hash(&mut hasher);
        self.title.hash(&mut hasher);
        self.tag.hash(&mut hasher);
        for (left, right) in &self.meta {
            left.hash(&mut hasher);
            right.hash(&mut hasher);
        }
        for section in self.sections() {
            section.kind.hash(&mut hasher);
            section.text.hash(&mut hasher);
        }
        self.footer.hash(&mut hasher);
        hasher.finish()
    }
}

/// One wrapped line of a laid-out tooltip.
#[derive(Debug, Clone)]
struct Line {
    section: usize,
    text: String,
}

/// A dock at `pointer` for a tooltip `width` wide, and the way it grows: away from the
/// nearer screen edges, clear of the cursor by a small gap.
#[must_use]
pub fn pointer_dock(
    pointer: (f32, f32),
    width: f32,
    kit: &Kit,
    viewport: (u32, u32),
) -> (UiRect, FlowDirection) {
    let gap = kit.px(16.0);
    let width = width.max(kit.px(200.0)).round();
    let direction = FlowDirection {
        right_to_left: pointer.0 > viewport.0 as f32 * 0.5,
        bottom_to_top: pointer.1 > viewport.1 as f32 * 0.5,
    };
    let x = if direction.right_to_left {
        pointer.0 - gap - width
    } else {
        pointer.0 + gap
    };
    let y = if direction.bottom_to_top {
        pointer.1 - gap
    } else {
        pointer.1 + gap
    };
    (UiRect::new(x, y, width, 0.0), direction)
}

/// A dock beside `slot` for a tooltip `width` wide: to its right, or to its left when there is
/// no room, top-aligned with the slot and clamped on screen by the panel.
///
/// What an item's tooltip opens from. An item is read where it lies: a tooltip in a corner of
/// the screen, or one that follows the pointer onto the neighbouring slot, makes the player
/// look away from the thing they are reading about. Anchored to the slot's rectangle it
/// never covers the slot itself and does not move while the pointer wanders over it.
#[must_use]
pub fn slot_dock(
    slot: UiRect,
    width: f32,
    kit: &Kit,
    viewport: (u32, u32),
) -> (UiRect, FlowDirection) {
    let gap = kit.px(10.0);
    let margin = kit.px(8.0);
    let width = width.max(kit.px(200.0)).round();
    let fits_right = slot.right() + gap + width <= viewport.0 as f32 - margin;
    let direction = FlowDirection {
        right_to_left: !fits_right,
        bottom_to_top: false,
    };
    let x = if fits_right {
        slot.right() + gap
    } else {
        slot.x - gap - width
    };
    (UiRect::new(x, slot.y, width, 0.0), direction)
}

/// Draws a [`TooltipModel`] at the tooltip dock, wrapped, sized to fit and kept on screen.
#[derive(Debug, Clone, Default)]
pub struct TooltipPanel {
    fingerprint: Option<u64>,
    lines: Vec<Line>,
    /// The title, cut to fit beside the tag.
    title: String,
}

impl TooltipPanel {
    /// An empty panel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws `model` growing out of `dock` in `direction`. Returns the panel's rectangle.
    pub fn push(
        &mut self,
        list: &mut UiDrawList,
        model: &TooltipModel,
        dock: UiRect,
        direction: FlowDirection,
        kit: &Kit,
        viewport: (u32, u32),
    ) -> UiRect {
        let pad = kit.px(16.0);
        let width = dock.width.max(kit.px(200.0)).round();
        let inner = width - pad * 2.0;
        let print = kit.style.scale.factor();
        let fingerprint = model.fingerprint(inner, print);
        if self.fingerprint != Some(fingerprint) {
            self.layout(model, inner, kit);
            self.fingerprint = Some(fingerprint);
        }

        // Measure.
        let title_line = kit.style.line(tokens::TOOLTIP_TITLE);
        let meta_line = kit.style.line(tokens::TOOLTIP_META);
        let heading_line = kit.px(20.0);
        let gap = kit.px(10.0);
        let meta_rows = model
            .meta
            .iter()
            .filter(|(left, right)| !left.is_empty() || !right.is_empty())
            .count();
        let mut height = pad * 2.0 + title_line + meta_line * meta_rows as f32;
        let mut previous = None;
        for line in &self.lines {
            let section = &model.sections[line.section];
            let kind = section.kind.unwrap_or(SectionKind::Description);
            if previous != Some(line.section) {
                height += gap;
                if kind.heading().is_some() {
                    height += heading_line;
                }
                previous = Some(line.section);
            }
            height += kit.style.line(kind.style());
        }
        if !model.footer.is_empty() {
            height += gap + meta_line;
        }

        // Place: out of the dock, away from the screen edge it is anchored to.
        let margin = kit.px(8.0);
        let x = if direction.right_to_left {
            dock.right() - width
        } else {
            dock.x
        };
        let y = if direction.bottom_to_top {
            dock.bottom() - height
        } else {
            dock.y
        };
        let (screen_w, screen_h) = (viewport.0 as f32, viewport.1 as f32);
        let rect = UiRect::new(
            x.clamp(margin, (screen_w - width - margin).max(margin))
                .round(),
            y.clamp(margin, (screen_h - height - margin).max(margin))
                .round(),
            width,
            height.round(),
        );

        super::kit::panel(list, kit, rect, model.border);

        // Title and tag.
        let mut cursor = rect.y + pad;
        let centre = cursor + title_line * 0.5;
        if !model.tag.is_empty() {
            kit.text(
                list,
                rect.right() - pad,
                centre,
                &model.tag,
                tokens::TOOLTIP_META,
                model.tag_color,
                TextAlign::Right,
            );
        }
        kit.text(
            list,
            rect.x + pad,
            centre,
            &self.title,
            tokens::TOOLTIP_TITLE,
            model.title_color,
            TextAlign::Left,
        );
        cursor += title_line;
        for (left, right) in &model.meta {
            if left.is_empty() && right.is_empty() {
                continue;
            }
            let centre = cursor + meta_line * 0.5;
            kit.text(
                list,
                rect.x + pad,
                centre,
                left,
                tokens::TOOLTIP_META,
                color::TEXT_SECONDARY,
                TextAlign::Left,
            );
            kit.text(
                list,
                rect.right() - pad,
                centre,
                right,
                tokens::TOOLTIP_META,
                color::TEXT_SECONDARY,
                TextAlign::Right,
            );
            cursor += meta_line;
        }

        // Sections.
        let mut previous = None;
        let mut section_top = cursor;
        for line in &self.lines {
            let section = &model.sections[line.section];
            let kind = section.kind.unwrap_or(SectionKind::Description);
            if previous != Some(line.section) {
                Self::accent_bar(
                    list,
                    kit,
                    model,
                    previous,
                    section_top,
                    cursor,
                    rect.x + pad,
                );
                cursor += gap;
                section_top = cursor;
                if let Some(heading) = kind.heading() {
                    kit.text(
                        list,
                        rect.x + pad,
                        cursor + heading_line * 0.5,
                        heading,
                        type_scale::LABEL,
                        kind.accent(),
                        TextAlign::Left,
                    );
                    cursor += heading_line;
                }
                previous = Some(line.section);
            }
            let style = kind.style();
            let line_height = kit.style.line(style);
            let ink = if kind == SectionKind::Comparison {
                match line.text.as_bytes().first() {
                    Some(b'+') => color::VERDANT,
                    Some(b'-') => color::CRIMSON,
                    _ => section.color,
                }
            } else {
                section.color
            };
            kit.text(
                list,
                rect.x + pad,
                cursor + line_height * 0.5,
                &line.text,
                style,
                ink,
                TextAlign::Left,
            );
            cursor += line_height;
        }
        Self::accent_bar(
            list,
            kit,
            model,
            previous,
            section_top,
            cursor,
            rect.x + pad,
        );

        if !model.footer.is_empty() {
            cursor += gap;
            kit.text(
                list,
                rect.x + pad,
                cursor + meta_line * 0.5,
                &model.footer,
                tokens::TOOLTIP_META,
                color::TEXT_MUTED,
                TextAlign::Left,
            );
        }
        rect
    }

    /// The accent bar to the left of a section that applies right now.
    fn accent_bar(
        list: &mut UiDrawList,
        kit: &Kit,
        model: &TooltipModel,
        section: Option<usize>,
        top: f32,
        bottom: f32,
        text_x: f32,
    ) {
        let Some(section) = section else {
            return;
        };
        let live = &model.sections[section];
        if !live.highlight {
            return;
        }
        let accent = live.kind.unwrap_or(SectionKind::Description).accent();
        list.rect(
            UiRect::new(
                (text_x - kit.px(9.0)).round(),
                top.round(),
                kit.px(2.0).max(2.0),
                (bottom - top).round(),
            ),
            accent,
        );
    }

    /// Wraps every section to `inner` pixels.
    fn layout(&mut self, model: &TooltipModel, inner: f32, kit: &Kit) {
        self.lines.clear();
        for (index, section) in model.sections().enumerate() {
            let style = section.kind.unwrap_or(SectionKind::Description).style();
            for paragraph in section.text.split('\n') {
                if paragraph.is_empty() {
                    continue;
                }
                for text in kit.style.wrap(style, paragraph, inner) {
                    self.lines.push(Line {
                        section: index,
                        text,
                    });
                }
            }
        }
        let tag = if model.tag.is_empty() {
            0.0
        } else {
            kit.width(tokens::TOOLTIP_META, &model.tag) + kit.px(12.0)
        };
        self.title = kit.style.fit(
            tokens::TOOLTIP_TITLE,
            &model.title,
            (inner - tag).max(kit.px(60.0)),
        );
    }

    /// The wrapped lines of the last layout, for tests.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

/// What the tooltip of an ability needs to know beyond the ability: the live state that
/// decides what pressing it now would do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilityContext {
    /// Whether the Druid's Astral Power is armed.
    pub astral_armed: bool,
    /// The current maximum Ignite stacks.
    pub ignite_max: u8,
    /// Which stat Fire Mastery grants.
    pub mastery: FireMastery,
}

/// Fills `model` with `view`'s ability: name, timings, its description, what it does with its
/// class's resource and what that buys, and what it needs.
///
/// Every sentence comes from the ability table through [`ability_text`]; every number that
/// depends on the character — damage, healing, cast time — from the slot view the HUD already
/// builds. Nothing here is written for one ability.
pub fn fill_ability(model: &mut TooltipModel, view: &AbilitySlotView, context: AbilityContext) {
    use std::fmt::Write as _;
    let ability = view.ability;
    let definition = ability.definition();
    model.clear();
    super::map_style::write_title_case(&mut model.title, view.name);

    // Meta: how it is used, then where it reaches.
    let (cast, cooldown) = &mut model.meta[0];
    if definition.cast_time > 0.0 {
        write_seconds_phrase(cast, view.cast_time);
        cast.push_str(
            if definition.target == TargetMode::Friendly
                && definition.radius > 0.0
                && definition.cast_time >= definition.duration
                && definition.duration > 0.0
            {
                " channel"
            } else {
                " cast"
            },
        );
    } else {
        cast.push_str("Instant");
    }
    if view.max_charges > 1 {
        let _ = write!(cooldown, "{}/{} charges", view.charges, view.max_charges);
        if definition.cooldown > 0.0 {
            cooldown.push_str(" · ");
            write_seconds_phrase(cooldown, definition.cooldown);
            cooldown.push_str(" recharge");
        }
    } else if definition.cooldown > 0.0 {
        write_seconds_phrase(cooldown, definition.cooldown);
        cooldown.push_str(" cooldown");
    }
    let (reach, area) = &mut model.meta[1];
    match definition.target {
        // Around the caster: the radius says it, and "Self" beside it would read as "only you".
        TargetMode::SelfCast if definition.radius > 0.0 => {}
        TargetMode::SelfCast => reach.push_str("Self"),
        TargetMode::Movement => {
            write_integer(reach, definition.range as i64);
            reach.push_str(" m");
        }
        _ if definition.range > 0.0 && definition.range <= 5.0 => reach.push_str("Melee range"),
        _ if definition.range > 0.0 => {
            write_integer(reach, definition.range as i64);
            reach.push_str(" m range");
        }
        _ => {}
    }
    if definition.radius > 0.0 {
        write_integer(area, definition.radius as i64);
        area.push_str(" m radius");
    } else if matches!(definition.target, TargetMode::Line) && definition.width > 0.0 {
        write_integer(area, definition.width as i64);
        area.push_str(" m wide");
    }

    // What it does.
    ability_text::write_description(&mut model.section(SectionKind::Description).text, ability);

    // What it does right now, in numbers.
    if view.damage > 0.0 || view.healing > 0.0 {
        let periodic = definition.tick_coefficient > 0.0 && definition.coefficient <= 0.0;
        let section = model.section(SectionKind::Effect);
        if view.damage > 0.0 {
            section.text.push_str("Damage ");
            write_integer(&mut section.text, view.damage.round() as i64);
        }
        if view.healing > 0.0 {
            if !section.text.is_empty() {
                section.text.push_str("   ");
            }
            section.text.push_str("Healing ");
            write_integer(&mut section.text, view.healing.round() as i64);
        }
        if periodic {
            section.text.push_str(" per tick");
        }
        section.color = color::TEXT_SECONDARY;
    }

    // The resource: cost, optional empowerment, generation — and whether this press pays.
    let mut empowers = false;
    let mut will_empower = false;
    if definition.resource_uses().next().is_some() {
        let section = model.section(SectionKind::Resource);
        for entry in definition.resource_uses() {
            if !section.text.is_empty() {
                section.text.push('\n');
            }
            entry.write_line(&mut section.text);
            if entry.mode == ResourceMode::Empower {
                empowers = true;
                let ready = match entry.resource {
                    ability_text::Resource::AstralPower => view.spends_astral,
                    _ => false,
                };
                will_empower |= ready;
                section.text.push('\n');
                match entry.resource {
                    ability_text::Resource::AstralPower if ready => {
                        section.text.push_str("Ready: Astral Power is armed");
                    }
                    ability_text::Resource::AstralPower => {
                        section
                            .text
                            .push_str("Not empowered: Astral Power is not armed");
                    }
                    _ => {}
                }
            }
        }
        section.color = color::TEXT_PRIMARY;
    }
    if empowers && !definition.empowered.is_empty() {
        let section = model.section(SectionKind::Empowered);
        ability_text::write_empowered(&mut section.text, ability);
        section.highlight = will_empower;
        section.color = if will_empower {
            color::TEXT_PRIMARY
        } else {
            color::TEXT_SECONDARY
        };
    }

    // Requirements: what the row says must be true, phrased once for every ability.
    let mut requirements = String::new();
    if ability == FireAbility::Detonate {
        let _ = write!(
            requirements,
            "{} Ignite stacks on the target",
            context.ignite_max
        );
    }
    if definition.target.needs_target() {
        if !requirements.is_empty() {
            requirements.push('\n');
        }
        requirements.push_str("An enemy target");
        if definition.checks_line_of_sight() {
            requirements.push_str(" in line of sight");
        }
    }
    if !requirements.is_empty() {
        let section = model.section(SectionKind::Requirements);
        section.text.push_str(&requirements);
        section.color = color::TEXT_SECONDARY;
    }

    if ability == FireAbility::FireMastery {
        model.footer.push_str(match context.mastery {
            FireMastery::Haste => "Specialized in Haste",
            FireMastery::Crit => "Specialized in Critical Strike",
        });
    }
    if empowers && will_empower {
        model.border = color::alpha(color::AMBER, 0.55);
    }
}

/// Fills `model` with a trinket's button: its name, what it does, its cooldown and radius.
///
/// `key` is the label of the key that uses it. The numbers are the item's own: quality and
/// sharpening scale a trinket's stats and never these.
pub fn fill_trinket(model: &mut TooltipModel, view: &crate::snapshot::TrinketView, key: &str) {
    use std::fmt::Write as _;
    model.clear();
    model.title.push_str(view.name);
    model.title_color = color::TEXT_TITLE;
    model.tag.push_str(key);
    model.tag_color = color::TEXT_SECONDARY;
    let active = &view.active;
    model.border = color::alpha(color::BRONZE_BRIGHT, 0.7);
    let _ = write!(model.meta[0].0, "{}", active.effect.name());
    let _ = write!(model.meta[0].1, "{:.0} sec cooldown", active.cooldown);
    let _ = write!(model.meta[1].0, "{:.0} m radius", active.radius);
    if active.effect.has_duration() {
        let _ = write!(model.meta[1].1, "{:.0} sec", active.duration);
    }
    model
        .section(SectionKind::Description)
        .text
        .push_str(&format!("Use: {}", active.description()));
    let section = model.section(SectionKind::Details);
    section.color = color::TEXT_SECONDARY;
    if view.is_cooling() {
        let _ = write!(
            section.text,
            "{:.0} sec until ready",
            view.cooldown_remaining.ceil()
        );
    } else {
        section.text.push_str("Ready");
    }
    model.footer.push_str("Not on the global cooldown.");
}

/// Fills `model` with a buff or debuff.
pub fn fill_aura(model: &mut TooltipModel, aura: &crate::aura::Aura) {
    use std::fmt::Write as _;
    model.clear();
    model.title.push_str(aura.kind.name());
    model.title_color = if aura.is_harmful() {
        color::CRIMSON
    } else {
        color::TEXT_TITLE
    };
    let (kind_color, kind) = match aura.kind.dispel_school() {
        Some(school) => (
            super::aura_row::school_color(school),
            super::aura_row::school_name(school),
        ),
        None if aura.is_harmful() => (color::TEXT_MUTED, "Debuff"),
        None => (color::VERDANT, "Buff"),
    };
    model.tag.push_str(kind);
    model.tag_color = kind_color;
    if aura.is_dispellable() {
        model.border = color::alpha(super::aura_row::DISPEL_BORDER, 0.8);
    }
    model
        .section(SectionKind::Description)
        .text
        .push_str(&aura.description());

    let section = model.section(SectionKind::Details);
    section.color = color::TEXT_SECONDARY;
    let text = &mut section.text;
    if aura.stacks > 1 {
        let _ = write!(text, "{} stacks", aura.stacks);
    }
    if aura.indefinite {
        if !text.is_empty() {
            text.push_str("  ·  ");
        }
        text.push_str("Until removed");
    } else if aura.remaining > 0.0 {
        if !text.is_empty() {
            text.push_str("  ·  ");
        }
        if aura.remaining >= 60.0 {
            let _ = write!(text, "{} min remaining", (aura.remaining / 60.0).ceil());
        } else {
            let _ = write!(text, "{} sec remaining", aura.remaining.ceil());
        }
    }
    if aura.amount > 0.0 {
        if !text.is_empty() {
            text.push_str("  ·  ");
        }
        let _ = write!(text, "{:.0} absorb left", aura.amount);
    }
}

/// Where the item a tooltip describes is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemPlace {
    /// Worn, in this slot.
    Equipped(EquipSlot),
    /// In the bag.
    Bag,
    /// Not in a slot at all: a loot toast, a reward row.
    Loose,
    /// Waiting in the mailbox.
    Mailed,
}

/// What an item's tooltip needs to know beyond the item: who is reading it and what they wear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemContext {
    /// Where the item is.
    pub place: ItemPlace,
    /// The reader's stats as they are now.
    pub total: StatBlock,
    /// What putting the item on would take off, for an item that is not already worn.
    pub replaces: Option<ItemStack>,
    /// What the reader wears, for the set section: which pieces are on.
    pub worn: [Option<ItemStack>; EquipSlot::COUNT],
    /// What the reader carries, for the set section: which pieces they own.
    pub carried: [Option<ItemStack>; INVENTORY_SLOTS],
}

impl ItemContext {
    /// The item in `slot` on `character`, with its context.
    #[must_use]
    pub fn for_slot(character: &CharacterView, slot: ItemSlot) -> Option<(ItemStack, Self)> {
        let stack = character.at(slot)?;
        Some(match slot {
            ItemSlot::Equipment(worn) => (
                stack,
                Self {
                    place: ItemPlace::Equipped(worn),
                    total: character.total,
                    replaces: None,
                    worn: character.equipment,
                    carried: character.inventory,
                },
            ),
            ItemSlot::Inventory(_) => (stack, Self::loose(character, stack, ItemPlace::Bag)),
        })
    }

    /// The context of an item that is not worn, against what it would replace.
    #[must_use]
    pub fn loose(character: &CharacterView, stack: ItemStack, place: ItemPlace) -> Self {
        let replaces = super::character_menu::equip_target(character, stack)
            .and_then(|slot| character.equipment[slot.index()]);
        Self {
            place,
            total: character.total,
            replaces,
            worn: character.equipment,
            carried: character.inventory,
        }
    }
}

/// An item's stats with the placeholder main stat folded into the hero's own.
fn effective(stats: &StatBlock, hero: Hero) -> StatBlock {
    let mut folded = *stats;
    let main = folded.get(Stat::MainStat);
    if main != 0.0 {
        folded.add(hero.main_stat(), main);
        folded.set(Stat::MainStat, 0.0);
    }
    folded
}

/// `total` with `remove` taken out and `add` put in.
fn swapped(total: &StatBlock, remove: &StatBlock, add: &StatBlock) -> StatBlock {
    let mut out = *total;
    for stat in Stat::ALL {
        out.add(stat, add.get(stat) - remove.get(stat));
    }
    out
}

/// What a stat is worth as a fraction in `stats`: a rating's percentage, or armour's
/// reduction. `None` for a stat that is not a percentage.
fn worth(stat: Stat, stats: &StatBlock) -> Option<f32> {
    if stat == Stat::Armor {
        Some(armor_reduction(stats.get(stat)))
    } else if stat.is_rating() {
        Some(stats.percent(stat))
    } else {
        None
    }
}

/// Writes `value` whole when it is whole and to one decimal otherwise.
fn write_amount(out: &mut String, value: f32) {
    if (value - value.round()).abs() < 0.05 {
        write_integer(out, value.round() as i64);
    } else {
        write_tenths(out, value);
    }
}

/// Writes a signed amount: `+2`, `-3`.
fn write_signed(out: &mut String, value: f32) {
    out.push(if value < 0.0 { '-' } else { '+' });
    write_amount(out, value.abs());
}

/// Writes a fraction as a percentage to two decimals.
fn write_percent(out: &mut String, fraction: f32) {
    use std::fmt::Write as _;
    let _ = write!(out, "{:.2}%", fraction * 100.0);
}

/// Fills `model` with an item: its name in its quality's colour, where it goes, the stats it
/// grants, what they are worth to `hero` in numbers and - for an item not yet worn - how it
/// compares to what it would replace.
///
/// Built like [`fill_ability`], from the item table and the reader's own sheet: no sentence
/// here is written for one item.
pub fn fill_item(model: &mut TooltipModel, stack: ItemStack, hero: Hero, context: &ItemContext) {
    model.clear();
    let Some(def) = stack.def() else {
        return;
    };
    // What this copy grants: the table's stats scaled by its quality and sharpening.
    let granted = stack.stats();
    super::map_style::write_title_case(&mut model.title, def.name);
    let quality = super::character_menu::quality_color(def.quality);
    model.title_color = quality;
    model.border = color::alpha(quality, 0.6);
    super::map_style::write_title_case(&mut model.tag, def.family.name());
    model.tag_color = color::TEXT_SECONDARY;
    // A highly sharpened copy says so, in its quality colour: the only marker, no effects.
    if stack.props.is_highly_sharpened() {
        model.tag.push_str(" - Sharpened +");
        write_integer(&mut model.tag, i64::from(stack.props.sharpening));
        model.tag_color = quality;
    }

    // Meta: how good it is, then where it is.
    let (rank, level) = &mut model.meta[0];
    super::map_style::write_title_case(rank, def.quality.name());
    level.push_str("Item level ");
    write_integer(level, i64::from(def.item_level));
    let place = &mut model.meta[1].0;
    match context.place {
        ItemPlace::Equipped(slot) => {
            place.push_str("Equipped - ");
            super::map_style::push_title_case(place, slot.name());
        }
        ItemPlace::Bag => place.push_str("In your bag"),
        ItemPlace::Loose => place.push_str("New"),
        ItemPlace::Mailed => place.push_str("In your mail"),
    }

    // The stats, as the genre prints them: "+2 Haste".
    let stats = model.section(SectionKind::Stats);
    stats.color = color::VERDANT;
    if granted.is_empty() {
        stats.color = color::TEXT_MUTED;
        stats.text.push_str("Grants no combat stats");
    }
    for (stat, value) in granted.entries() {
        if !stats.text.is_empty() {
            stats.text.push('\n');
        }
        write_signed(&mut stats.text, value);
        stats.text.push(' ');
        super::map_style::push_title_case(&mut stats.text, stat.display_for(hero));
    }

    // Gem sockets: a hollow marker when empty, the gem and its stat when filled. A gem itself
    // is not equipment: it says where it goes instead.
    let is_gem = def.family == engine_game::character::ItemFamily::Gem;
    if is_gem {
        let note = model.section(SectionKind::Description);
        note.color = color::TEXT_SECONDARY;
        note.text
            .push_str("Socket it into equipment, or combine two into a better one, at the Workshop.");
    } else {
        write_socket_lines(model, def.sockets, stack.props, hero);
    }

    // This copy's quality: what it rolled, what sharpening added, what it is worth in all.
    if !granted.is_empty() && !is_gem {
        write_quality_lines(model, stack.props);
    }

    // What they do for this character, in the numbers combat uses.
    let mine = effective(&granted, hero);
    let (before, after) = match (context.place, context.replaces) {
        (ItemPlace::Equipped(_), _) => (
            swapped(&context.total, &mine, &StatBlock::zero()),
            context.total,
        ),
        (_, Some(worn)) => {
            let gone = effective(&worn.stats(), hero);
            (context.total, swapped(&context.total, &gone, &mine))
        }
        (_, None) => (
            context.total,
            swapped(&context.total, &StatBlock::zero(), &mine),
        ),
    };
    if !granted.is_empty() && !is_gem {
        let numbers = model.section(SectionKind::Numbers);
        numbers.color = color::TEXT_SECONDARY;
        for stat in Stat::ALL {
            let value = mine.get(stat);
            if value == 0.0 {
                continue;
            }
            if !numbers.text.is_empty() {
                numbers.text.push('\n');
            }
            super::map_style::push_title_case(&mut numbers.text, stat.name());
            numbers.text.push_str(": ");
            if let Some((now, was)) = worth(stat, &after).zip(worth(stat, &before)) {
                write_signed(&mut numbers.text, (now - was) * 100.0);
                numbers.text.push_str("%  (");
                write_percent(&mut numbers.text, was);
                numbers.text.push_str(" > ");
                write_percent(&mut numbers.text, now);
                numbers.text.push(')');
            } else if stat == Stat::Stamina {
                write_signed(&mut numbers.text, value * HEALTH_PER_STAMINA);
                numbers.text.push_str(" maximum health");
            } else {
                write_signed(&mut numbers.text, value);
                numbers.text.push_str(if stat == Stat::Strength {
                    " attack power"
                } else {
                    " spell power"
                });
            }
        }
    }

    // Against what it would replace.
    if !is_gem
        && !matches!(context.place, ItemPlace::Equipped(_))
        && let Some(worn_stack) = context.replaces
        && let Some(worn) = worn_stack.def()
    {
        let theirs = effective(&worn_stack.stats(), hero);
        let compare = model.section(SectionKind::Comparison);
        compare.color = color::TEXT_SECONDARY;
        compare.text.push_str("vs. ");
        super::map_style::push_title_case(&mut compare.text, worn.name);
        for stat in Stat::ALL {
            let delta = mine.get(stat) - theirs.get(stat);
            if delta.abs() < 0.001 {
                continue;
            }
            compare.text.push('\n');
            write_signed(&mut compare.text, delta);
            compare.text.push(' ');
            super::map_style::push_title_case(&mut compare.text, stat.name());
        }
    }

    if let Some(set) = def.set.and_then(set_by_id) {
        fill_set(model, set, hero, context);
    }

    model.footer.push_str(match context.place {
        ItemPlace::Equipped(_) => "Right-click to unequip - drag to move",
        ItemPlace::Bag if is_gem => "Drag to move - socket it at the Workshop",
        ItemPlace::Bag => "Right-click to equip - drag to move",
        ItemPlace::Loose => "Added to your bag",
        ItemPlace::Mailed => "Sent to your mail",
    });
}

/// Writes a set effect as a short phrase: `+12% Firebolt damage`, `Flame Lance cooldown -20%`,
/// `+15% resource generation`.
pub fn write_set_effect(out: &mut String, effect: SetEffect) {
    let percent = (effect.value() * 100.0).round() as i64;
    match effect {
        SetEffect::DamagePct { ability, .. } => {
            out.push('+');
            write_integer(out, percent);
            out.push_str("% ");
            super::map_style::push_title_case(out, ability.definition().name);
            out.push_str(" damage");
        }
        SetEffect::CooldownPct { ability, .. } => {
            super::map_style::push_title_case(out, ability.definition().name);
            out.push_str(" cooldown -");
            write_integer(out, percent);
            out.push('%');
        }
        SetEffect::ResourceGenPct { .. } => {
            out.push('+');
            write_integer(out, percent);
            out.push_str("% resource generation");
        }
    }
}

/// Writes one set bonus tier: its stats on one line, then one line per effect, the first
/// prefixed with its piece count: `(2) +6 Critical Strike, +4 Haste`.
pub fn write_set_bonus(out: &mut String, bonus: &SetBonus, hero: Hero) {
    out.push('(');
    write_integer(out, i64::from(bonus.count));
    out.push_str(") ");
    let mut wrote = false;
    for (stat, value) in &bonus.stats {
        if wrote {
            out.push_str(", ");
        }
        write_signed(out, *value);
        out.push(' ');
        super::map_style::push_title_case(out, stat.display_for(hero));
        wrote = true;
    }
    for effect in &bonus.effects {
        if wrote {
            out.push_str("\n     ");
        }
        write_set_effect(out, *effect);
        wrote = true;
    }
}

/// Writes the set sections of an item that belongs to `set`: the name and how many pieces are
/// worn, the pieces (worn ones highlighted, owned ones plain, missing ones muted) and every
/// bonus tier, greyed until the worn pieces earn it.
fn fill_set(model: &mut TooltipModel, set: &SetDef, hero: Hero, context: &ItemContext) {
    let worn = active_sets_of(&context.worn);
    let equipped = worn.get(&set.id).map_or(0, |active| active.equipped);

    let name = model.section(SectionKind::SetName);
    name.color = color::BRONZE_BRIGHT;
    name.text.push_str(&set.name);
    name.text.push_str(" (");
    write_integer(&mut name.text, i64::from(equipped));
    name.text.push('/');
    write_integer(&mut name.text, set.pieces.len() as i64);
    name.text.push(')');

    #[derive(PartialEq, Clone, Copy)]
    enum Have {
        Worn,
        Carried,
        Missing,
    }
    let has = |stacks: &[Option<ItemStack>], key: &str| {
        item_by_key(key).is_some_and(|def| stacks.iter().flatten().any(|s| s.item == def.id))
    };
    for (state, tint, highlight) in [
        (Have::Worn, color::VERDANT, true),
        (Have::Carried, color::TEXT_PRIMARY, false),
        (Have::Missing, color::TEXT_MUTED, false),
    ] {
        let section = model.section(SectionKind::SetPiece);
        section.color = tint;
        section.highlight = highlight;
        for key in &set.pieces {
            let have = if has(&context.worn, key) {
                Have::Worn
            } else if has(&context.carried, key) {
                Have::Carried
            } else {
                Have::Missing
            };
            if have != state {
                continue;
            }
            if !section.text.is_empty() {
                section.text.push('\n');
            }
            let piece = item_by_key(key).map_or(key.as_str(), |def| def.name);
            super::map_style::push_title_case(&mut section.text, piece);
            if state == Have::Carried {
                section.text.push_str(" (in bag)");
            }
        }
        if section.text.is_empty() {
            // Nothing in this state: give the section back.
            model.drop_last_section();
        }
    }

    for (active, tint) in [(true, color::VERDANT), (false, color::TEXT_MUTED)] {
        let section = model.section(SectionKind::SetBonus);
        section.color = tint;
        for bonus in &set.bonuses {
            if (bonus.count <= equipped) != active {
                continue;
            }
            if !section.text.is_empty() {
                section.text.push('\n');
            }
            write_set_bonus(&mut section.text, bonus, hero);
        }
        if section.text.is_empty() {
            model.drop_last_section();
        }
    }
}

/// Writes the quality sections of an item copy: `Base Quality: 106%`, `Sharpening: +7%`,
/// `Effective Quality: 113%`, and for a broken one a red line saying so.
/// The item's sockets, one line each: `( ) Empty socket`, or `(*) Perfect Ruby +6 Main Stat`.
/// Nothing for an item with none.
fn write_socket_lines(model: &mut TooltipModel, sockets: u8, props: ItemProps, hero: Hero) {
    if sockets == 0 {
        return;
    }
    let section = model.section(SectionKind::Sockets);
    section.color = color::TEXT_SECONDARY;
    for socket in 0..usize::from(sockets) {
        if socket > 0 {
            section.text.push('\n');
        }
        let gem = props
            .gems
            .get(socket)
            .filter(|id| **id != 0)
            .and_then(|id| engine_game::character::item(engine_game::character::ItemId(*id)))
            .filter(|def| def.family == engine_game::character::ItemFamily::Gem);
        match gem {
            Some(gem) => {
                section.text.push_str("(*) ");
                super::map_style::push_title_case(&mut section.text, gem.name);
                for (stat, value) in gem.stats.entries() {
                    section.text.push(' ');
                    write_signed(&mut section.text, value);
                    section.text.push(' ');
                    super::map_style::push_title_case(&mut section.text, stat.display_for(hero));
                }
            }
            None => section.text.push_str("( ) Empty socket"),
        }
    }
}

fn write_quality_lines(model: &mut TooltipModel, props: ItemProps) {
    let section = model.section(SectionKind::Quality);
    section.color = color::TEXT_SECONDARY;
    section.text.push_str("Base Quality: ");
    write_amount(&mut section.text, props.base_quality_pct());
    section.text.push('%');
    if props.broken {
        section.text.push_str("\nEffective Quality: ");
    } else {
        section.text.push_str("\nSharpening: +");
        write_amount(&mut section.text, props.sharpening_pct());
        section.text.push_str("%\nEffective Quality: ");
    }
    write_amount(&mut section.text, props.total_pct());
    section.text.push('%');
    if props.broken {
        let broken = model.section(SectionKind::Quality);
        broken.color = color::CRIMSON;
        broken.text.push_str("BROKEN (-");
        write_integer(&mut broken.text, (BROKEN_PENALTY * 100.0).round() as i64);
        broken.text.push_str("%) - cannot be sharpened");
    }
}

/// An item tooltip with its own storage, docked beside whatever shows the item.
///
/// The one way an item is explained anywhere in the interface - the character sheet's slots,
/// the loot toast, a reward row on the dungeon map - so they cannot disagree. Holds a model
/// and a panel and rewrites them in place, so hovering costs no allocation once warm.
#[derive(Debug, Default)]
pub struct ItemTooltip {
    model: TooltipModel,
    panel: TooltipPanel,
}

impl ItemTooltip {
    /// An empty tooltip with its storage reserved.
    #[must_use]
    pub fn new() -> Self {
        Self {
            model: TooltipModel::new(),
            panel: TooltipPanel::new(),
        }
    }

    /// Draws the tooltip of `stack` next to `anchor` (see [`slot_dock`]) and returns its
    /// rectangle. Nothing is drawn for a stack whose item the table does not know.
    #[allow(
        clippy::too_many_arguments,
        reason = "a tooltip is this many independent facts"
    )]
    pub fn push(
        &mut self,
        list: &mut UiDrawList,
        kit: &Kit,
        stack: ItemStack,
        hero: Hero,
        context: &ItemContext,
        anchor: UiRect,
        viewport: (u32, u32),
    ) -> Option<UiRect> {
        stack.def()?;
        fill_item(&mut self.model, stack, hero, context);
        let (dock, direction) = slot_dock(anchor, kit.px(DEFAULT_WIDTH), kit, viewport);
        Some(
            self.panel
                .push(list, &self.model, dock, direction, kit, viewport),
        )
    }
}

/// `8 sec`, `1.5 sec`, `2 min`.
fn write_seconds_phrase(out: &mut String, seconds: f32) {
    if seconds >= 60.0 && (seconds % 60.0).abs() < 0.05 {
        write_integer(out, (seconds / 60.0).round() as i64);
        out.push_str(" min");
    } else if (seconds - seconds.round()).abs() < 0.05 {
        write_integer(out, seconds.round() as i64);
        out.push_str(" sec");
    } else {
        write_tenths(out, seconds);
        out.push_str(" sec");
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_render_data::ui::{UiCommand, UiScale};

    const FHD: (u32, u32) = (1920, 1080);

    fn dock() -> UiRect {
        UiRect::new(1500.0, 950.0, DEFAULT_WIDTH, DOCK_HEIGHT)
    }

    fn bottom_right() -> FlowDirection {
        FlowDirection {
            right_to_left: true,
            bottom_to_top: true,
        }
    }

    fn model_with(text: &str) -> TooltipModel {
        let mut model = TooltipModel::new();
        model.clear();
        model.title.push_str("Colossus Strike");
        model.section(SectionKind::Description).text.push_str(text);
        model
    }

    #[test]
    fn a_long_description_wraps_and_the_panel_grows_instead_of_overflowing() {
        let kit = Kit::new(UiScale::default());
        let mut panel = TooltipPanel::new();
        let mut list = UiDrawList::new();
        let short = panel.push(
            &mut list,
            &model_with("Short."),
            dock(),
            bottom_right(),
            &kit,
            FHD,
        );
        let long_text = "Smash the ground, dealing Physical damage plus 1% of your maximum Health \
                         to all enemies within 8 m, and making them bleed for 8 sec. "
            .repeat(4);
        let mut list = UiDrawList::new();
        let long = panel.push(
            &mut list,
            &model_with(&long_text),
            dock(),
            bottom_right(),
            &kit,
            FHD,
        );
        assert!(panel.line_count() > 4, "it did not wrap");
        assert!(long.height > short.height * 2.0, "{short:?} -> {long:?}");
        assert!(
            (long.width - short.width).abs() < 0.5,
            "the width followed the text"
        );
        // Every line fits inside the panel.
        let pad = kit.px(16.0);
        for command in list.commands() {
            if let UiCommand::Label {
                origin,
                size,
                font,
                range,
                ..
            } = *command
            {
                let text = list.text_of(range);
                let width = font.width(text, size);
                assert!(
                    origin.0 >= long.x && origin.0 + width <= long.right() - pad + 1.0,
                    "{text:?} overflows: {origin:?} + {width} in {long:?}"
                );
            }
        }
    }

    #[test]
    fn the_panel_opens_from_the_dock_and_stays_on_screen() {
        let kit = Kit::new(UiScale::default());
        let mut panel = TooltipPanel::new();
        let mut list = UiDrawList::new();
        let rect = panel.push(
            &mut list,
            &model_with("Text."),
            dock(),
            bottom_right(),
            &kit,
            FHD,
        );
        assert!((rect.right() - dock().right()).abs() < 0.5);
        assert!((rect.bottom() - dock().bottom()).abs() < 0.5);
        // A dock against the screen's corner still yields a panel inside it.
        let corner = UiRect::new(1900.0, 1070.0, DEFAULT_WIDTH, DOCK_HEIGHT);
        let rect = panel.push(
            &mut list,
            &model_with("Text."),
            corner,
            FlowDirection::READING,
            &kit,
            FHD,
        );
        assert!(rect.x >= 0.0 && rect.right() <= 1920.0 && rect.bottom() <= 1080.0);
    }

    #[test]
    fn an_aura_tooltip_opens_beside_the_pointer_away_from_the_nearer_edges() {
        let kit = Kit::new(engine_render_data::ui::UiScale::new(1.0));
        let model = model_with("Burns for a while.");
        for pointer in [
            (100.0, 100.0),
            (1800.0, 100.0),
            (100.0, 1000.0),
            (1800.0, 1000.0),
        ] {
            let (dock, direction) = pointer_dock(pointer, DEFAULT_WIDTH, &kit, FHD);
            let rect = TooltipPanel::new().push(
                &mut UiDrawList::new(),
                &model,
                dock,
                direction,
                &kit,
                FHD,
            );
            // Beside the pointer, never under it, and on the screen's open side.
            assert!(
                !rect.contains(pointer.0, pointer.1),
                "{pointer:?} -> {rect:?}"
            );
            assert_eq!(
                rect.x > pointer.0,
                pointer.0 < 960.0,
                "{pointer:?} -> {rect:?}"
            );
            assert_eq!(
                rect.y > pointer.1,
                pointer.1 < 540.0,
                "{pointer:?} -> {rect:?}"
            );
            assert!((rect.width - DEFAULT_WIDTH).abs() < 0.5);
        }
    }

    #[test]
    fn holding_a_tooltip_open_wraps_once() {
        let kit = Kit::new(UiScale::default());
        let mut panel = TooltipPanel::new();
        let model = model_with("Some words that wrap across a few lines of the panel, perhaps.");
        let mut list = UiDrawList::new();
        panel.push(&mut list, &model, dock(), bottom_right(), &kit, FHD);
        let first = panel.lines.as_ptr();
        for _ in 0..10 {
            list.clear();
            panel.push(&mut list, &model, dock(), bottom_right(), &kit, FHD);
        }
        assert_eq!(panel.lines.as_ptr(), first, "the lines were rebuilt");
    }

    #[test]
    fn the_tooltip_font_is_considerably_larger_than_the_old_bitmap_one() {
        let kit = Kit::new(UiScale::default());
        assert!(kit.style.size(tokens::TOOLTIP_BODY) >= 15.0);
        assert!(kit.style.size(tokens::TOOLTIP_TITLE) >= 19.0);
    }

    fn loot_item() -> &'static engine_game::character::ItemDef {
        engine_game::character::loot_pool()
            .iter()
            .find(|def| def.stats.entries().count() == 2)
            .expect("a two-stat loot item")
    }

    fn character() -> CharacterView {
        let mut simulation = crate::snapshot::dressed_training_arena();
        simulation.step_once();
        crate::snapshot::UiSnapshot::build(&simulation).character
    }

    #[test]
    fn an_item_tooltip_names_the_item_in_its_quality_colour_and_prints_signed_stats() {
        let def = loot_item();
        let character = character();
        let context = ItemContext::loose(&character, ItemStack::one(def.id), ItemPlace::Bag);
        let mut model = TooltipModel::new();
        fill_item(&mut model, ItemStack::one(def.id), Hero::FireMage, &context);
        assert_eq!(
            model.title_color,
            super::super::character_menu::quality_color(def.quality)
        );
        assert_eq!(model.meta[0].0, "Uncommon");
        assert!(model.meta[0].1.starts_with("Item level"));
        let stats = &model.find(SectionKind::Stats).expect("stat lines").text;
        assert!(stats.lines().all(|line| line.starts_with('+')), "{stats}");
        assert!(!stats.to_lowercase().contains("main stat"), "{stats}");
        assert!(model.footer.contains("Right-click to equip"));
    }

    #[test]
    fn a_rating_is_explained_in_percent_with_the_before_and_after() {
        let def = engine_game::character::loot_pool()
            .iter()
            .find(|def| def.stats.get(Stat::Haste) > 0.0)
            .expect("a haste item");
        let character = character();
        let context = ItemContext::loose(&character, ItemStack::one(def.id), ItemPlace::Bag);
        let mut model = TooltipModel::new();
        fill_item(&mut model, ItemStack::one(def.id), Hero::FireMage, &context);
        let numbers = &model.find(SectionKind::Numbers).expect("numbers").text;
        assert!(numbers.contains("Haste: +"), "{numbers}");
        assert!(numbers.contains("0.00% >"), "{numbers}");
    }

    #[test]
    fn an_item_that_replaces_another_is_compared_with_it() {
        let character = character();
        let ring = engine_game::character::loot_pool()
            .iter()
            .find(|def| def.family == engine_game::character::ItemFamily::Feet)
            .expect("boots");
        let context = ItemContext::loose(&character, ItemStack::one(ring.id), ItemPlace::Bag);
        assert!(
            context.replaces.is_some(),
            "the starting boots are not replaced"
        );
        let mut model = TooltipModel::new();
        fill_item(
            &mut model,
            ItemStack::one(ring.id),
            Hero::FireMage,
            &context,
        );
        let compare = model.find(SectionKind::Comparison).expect("a comparison");
        assert!(compare.text.starts_with("vs. "), "{}", compare.text);
    }

    #[test]
    fn a_slot_dock_opens_to_the_right_and_flips_left_at_the_screen_edge() {
        let kit = Kit::new(UiScale::default());
        let width = kit.px(DEFAULT_WIDTH);
        let slot = UiRect::new(200.0, 300.0, 64.0, 64.0);
        let (dock, direction) = slot_dock(slot, width, &kit, FHD);
        assert!(dock.x >= slot.right() && !direction.right_to_left);

        let edge = UiRect::new(1800.0, 300.0, 64.0, 64.0);
        let (dock, direction) = slot_dock(edge, width, &kit, FHD);
        assert!(direction.right_to_left);
        let rect = TooltipPanel::new().push(
            &mut UiDrawList::new(),
            &model_with("Text."),
            dock,
            direction,
            &kit,
            FHD,
        );
        assert!(
            rect.right() <= edge.x,
            "{rect:?} covers the slot at {edge:?}"
        );
    }

    #[test]
    fn a_tall_item_tooltip_is_clamped_vertically_and_still_beside_the_slot() {
        let kit = Kit::new(UiScale::default());
        let bottom = UiRect::new(200.0, 1040.0, 64.0, 64.0);
        let (dock, direction) = slot_dock(bottom, kit.px(DEFAULT_WIDTH), &kit, FHD);
        let rect = TooltipPanel::new().push(
            &mut UiDrawList::new(),
            &model_with("A line."),
            dock,
            direction,
            &kit,
            FHD,
        );
        assert!(rect.bottom() <= 1080.0);
        assert!(rect.x >= bottom.right());
    }

    fn tooltip_of(stack: ItemStack) -> TooltipModel {
        let character = character();
        let context = ItemContext::loose(&character, stack, ItemPlace::Bag);
        let mut model = TooltipModel::new();
        fill_item(&mut model, stack, Hero::FireMage, &context);
        model
    }

    #[test]
    fn an_items_quality_sharpening_and_effective_quality_are_printed() {
        let def = loot_item();
        let stack = ItemStack::with_props(def.id, ItemProps::new(106, 7, false));
        let model = tooltip_of(stack);
        let lines = &model
            .find(SectionKind::Quality)
            .expect("quality lines")
            .text;
        assert_eq!(
            lines,
            "Base Quality: 106%\nSharpening: +7%\nEffective Quality: 113%"
        );
        // The stats shown are the props-scaled ones, not the table's.
        let base = tooltip_of(ItemStack::one(def.id));
        let (scaled, plain) = (
            &model.find(SectionKind::Stats).expect("stats").text,
            &base.find(SectionKind::Stats).expect("stats").text,
        );
        assert_ne!(scaled, plain, "113% is not 100%");
        assert_eq!(model.sections().count(), base.sections().count());
    }

    #[test]
    fn sharpening_is_printed_at_plus_zero_and_a_high_one_is_marked() {
        let def = loot_item();
        let text = |props| {
            let model = tooltip_of(ItemStack::with_props(def.id, props));
            (
                model
                    .find(SectionKind::Quality)
                    .expect("lines")
                    .text
                    .clone(),
                model.tag.clone(),
            )
        };
        let (lines, tag) = text(ItemProps::new(82, 0, false));
        assert_eq!(
            lines,
            "Base Quality: 82%
Sharpening: +0%
Effective Quality: 82%"
        );
        assert!(!tag.contains("Sharpened"), "{tag}");
        let (lines, tag) = text(ItemProps::new(82, 7, false));
        assert_eq!(
            lines,
            "Base Quality: 82%
Sharpening: +7%
Effective Quality: 89%"
        );
        assert!(tag.ends_with("Sharpened +7"), "{tag}");
        assert!(!text(ItemProps::new(82, 4, false)).1.contains("Sharpened"));
    }

    #[test]
    fn a_broken_item_says_so_in_red_and_loses_its_sharpening_line() {
        let def = loot_item();
        let model = tooltip_of(ItemStack::with_props(def.id, ItemProps::new(100, 0, true)));
        let mut quality = model
            .sections()
            .filter(|section| section.kind == Some(SectionKind::Quality));
        let lines = quality.next().expect("quality lines");
        assert_eq!(lines.text, "Base Quality: 100%\nEffective Quality: 85%");
        let broken = quality.next().expect("a broken line");
        assert!(broken.text.starts_with("BROKEN (-15%)"), "{}", broken.text);
        assert!(broken.text.contains("cannot be sharpened"));
        assert_eq!(broken.color, color::CRIMSON);
    }

    #[test]
    fn an_epic_is_titled_in_purple() {
        let def = engine_realm_epic();
        let model = tooltip_of(ItemStack::with_props(def.id, ItemProps::rolled(104)));
        assert_eq!(
            model.title_color,
            super::super::character_menu::quality_color(engine_game::character::ItemQuality::Epic)
        );
        assert_eq!(model.meta[0].0, "Epic");
    }

    #[test]
    fn a_set_piece_lists_its_set_and_greys_the_tiers_it_has_not_earned() {
        use engine_game::character::item_by_key;
        engine_realm::loot::ensure_items_registered();
        let piece = |key: &str| ItemStack::one(item_by_key(key).expect("a set piece").id);
        let mut character = character();
        character.equipment = [None; EquipSlot::COUNT];
        character.inventory = [None; engine_game::character::INVENTORY_SLOTS];
        character.equipment[EquipSlot::Head.index()] = Some(piece("emberwake_hood"));
        character.equipment[EquipSlot::Shoulder.index()] = Some(piece("emberwake_mantle"));
        character.inventory[0] = Some(piece("emberwake_shroud"));

        let stack = piece("emberwake_shroud");
        let context = ItemContext::loose(&character, stack, ItemPlace::Bag);
        let mut model = TooltipModel::new();
        fill_item(&mut model, stack, Hero::FireMage, &context);

        let name = &model.find(SectionKind::SetName).expect("a set name").text;
        assert_eq!(name, "Emberwake Regalia (2/5)");
        let pieces: Vec<_> = model
            .sections()
            .filter(|section| section.kind == Some(SectionKind::SetPiece))
            .collect();
        assert_eq!(pieces.len(), 3, "worn, owned and missing");
        assert!(pieces[0].highlight && pieces[0].text.contains("Emberwake Hood"));
        assert!(pieces[1].text.contains("Emberwake Shroud (in bag)"));
        assert!(pieces[2].text.contains("Emberwake Gauntlets"));
        let bonuses: Vec<_> = model
            .sections()
            .filter(|section| section.kind == Some(SectionKind::SetBonus))
            .collect();
        assert_eq!(bonuses.len(), 2, "earned, then still to earn");
        assert!(bonuses[0].text.starts_with("(2) +6 Critical Strike"));
        assert!(!bonuses[0].text.contains("(4)"));
        assert!(bonuses[1].text.contains("(4)") && bonuses[1].text.contains("(5)"));
        assert!(bonuses[1].text.contains("+12% Firebolt damage"));
        assert!(bonuses[1].text.contains("Flame Lance cooldown -20%"));
        assert_ne!(bonuses[0].color, bonuses[1].color);

        // An item that is part of no set says nothing about one.
        let loose = piece("gravewardens_cowl");
        let context = ItemContext::loose(&character, loose, ItemPlace::Bag);
        fill_item(&mut model, loose, Hero::FireMage, &context);
        assert!(model.find(SectionKind::SetName).is_none());
    }

    /// An epic from the registered catalog items, if the catalog registered any.
    #[test]
    fn sockets_show_a_hollow_marker_when_empty_and_the_gem_with_its_stat_when_filled() {
        use engine_game::character::{ItemProps, ItemStack};
        let epic = engine_realm_epic();
        assert!(epic.sockets > 0, "the sample epic has a socket");
        let gem = engine_game::character::item_by_key("gem_haste_3").expect("a gem");
        let text = |props: ItemProps| {
            let stack = ItemStack::with_props(epic.id, props);
            let model = tooltip_of(stack);
            model
                .find(SectionKind::Sockets)
                .map(|section| section.text.clone())
                .unwrap_or_default()
        };
        assert!(text(ItemProps::STANDARD).starts_with("( ) Empty socket"));
        let mut gems = [0; engine_game::character::GEM_SOCKETS_MAX];
        gems[0] = gem.id.0;
        let filled = text(ItemProps::STANDARD.with_gems(gems));
        assert!(filled.starts_with("(*) Cut Emerald"), "{filled}");
        assert!(filled.contains("+0.8 Haste"), "{filled}");
    }

    #[test]
    fn a_gem_item_says_where_it_goes_and_prints_no_quality_lines() {
        use engine_game::character::ItemStack;
        engine_realm::loot::ensure_items_registered();
        let gem = engine_game::character::item_by_key("gem_haste_3").expect("a gem");
        let model = tooltip_of(ItemStack::one(gem.id));
        assert!(model.find(SectionKind::Stats).is_some());
        assert!(model.find(SectionKind::Quality).is_none());
        assert!(model.find(SectionKind::Sockets).is_none());
        assert!(model.footer.contains("Workshop"));
    }

    fn engine_realm_epic() -> &'static engine_game::character::ItemDef {
        engine_realm::loot::ensure_items_registered();
        engine_game::character::catalog_items()
            .iter()
            .find(|def| def.quality == engine_game::character::ItemQuality::Epic)
            .expect("the catalog has an epic")
    }
}
