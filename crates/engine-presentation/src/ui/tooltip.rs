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
//! ## A fixed dock, not the pointer
//!
//! A tooltip that follows the pointer lands on top of the thing being read and jumps every
//! time the pointer moves between buttons. Every tooltip in the game — abilities, buffs and
//! debuffs — opens from the same dock (bottom right by default), which the player can move,
//! widen and point in another direction in Edit HUD like any other element.
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
use engine_game::firemage::{FireAbility, FireMastery, TargetMode};
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect};

use super::flow::FlowDirection;
use super::kit::{Kit, tokens};
use super::map_style::{TextStyle, color, type_scale};
use crate::game_ui::{write_integer, write_tenths};
use crate::snapshot::AbilitySlotView;

/// A tooltip's width when the player has not set one, logical pixels.
pub const DEFAULT_WIDTH: f32 = 380.0;

/// The dock's height in Edit HUD, logical pixels: a tooltip's opening edge, not its size.
pub const DOCK_HEIGHT: f32 = 64.0;

/// Sections a tooltip may hold.
const MAX_SECTIONS: usize = 8;

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
}

impl SectionKind {
    /// The heading the section is printed under, if it has one.
    #[must_use]
    pub fn heading(self) -> Option<&'static str> {
        match self {
            Self::Description | Self::Effect | Self::Details => None,
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
            _ => color::TEXT_MUTED,
        }
    }

    /// The body's type.
    #[must_use]
    pub fn style(self) -> TextStyle {
        match self {
            Self::Effect | Self::Details => tokens::TOOLTIP_META,
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
            kit.text(
                list,
                rect.x + pad,
                cursor + line_height * 0.5,
                &line.text,
                style,
                section.color,
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
    /// The Warrior's banked Deflection.
    pub deflection: f32,
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
                    ability_text::Resource::Deflection => view.spends_deflection,
                    ability_text::Resource::AstralPower => view.spends_astral,
                    _ => false,
                };
                will_empower |= ready;
                section.text.push('\n');
                match entry.resource {
                    ability_text::Resource::Deflection if ready => {
                        let _ = write!(
                            section.text,
                            "Ready: {} Deflection banked",
                            context.deflection.floor()
                        );
                    }
                    ability_text::Resource::Deflection => {
                        let _ = write!(
                            section.text,
                            "Not empowered: {} of {} Deflection",
                            context.deflection.floor(),
                            entry.threshold.max(entry.amount)
                        );
                    }
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
        .push_str(aura.kind.description());

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
}
