//! The playground's Effects mode: the global effect registry on the left, and on the right
//! whoever the effects are being tried on — their stats, their buffs and their debuffs.
//!
//! ```text
//! ┌ Effect Library ───────┐                         ┌ Playground ─────────────┐
//! │ [Monsters | Effects]  │                         │ PLAYER …                │
//! │ [ Search effects… ]   │                         │ EFFECT TARGET           │
//! │ BUFFS                 │                         │ ● You       ○ Skeleton  │
//! │ ▣ Fortitude      Buff │                         │ STATS                   │
//! │ DEBUFFS               │                         │ Stamina      70  (+20)  │
//! │ ▣ Poisoned     Poison │                         │ BUFFS · 2               │
//! ├───────────────────────┤                         │ ▣ Fortitude ×2   58s  × │
//! │ Fortitude             │                         │ DEBUFFS · 1             │
//! │ Buff · stacks to 3    │                         │ ▣ Poisoned       11s  × │
//! │ +20 Stamina per stack │                         │ [ Add test effects ]    │
//! ├───────────────────────┤                         │ [Clear buffs|Clear deb.]│
//! │ TIME - 30s +  STACKS -│                         │ [ RESET ENCOUNTER ]     │
//! │ [ APPLY TO YOU ]      │                         └─────────────────────────┘
//! └───────────────────────┘
//! ```
//!
//! Nothing here is an effect. The list is [`StatusKind::ALL`] and each row's words are its
//! registry row; what is on a target is the same [`AuraList`] its nameplate and frame read,
//! drawn with the same icon widget; the stats are [`Simulation::effect_stats`], the numbers
//! gameplay multiplies by. Applying, removing and clearing are actions the application
//! carries out through the playground, which carries them out through the simulation.

use std::fmt::Write as _;

use engine_game::hero::Hero;
use engine_game::playground::{EffectTarget, MonsterPlayground, TargetRole};
use engine_game::sim::Simulation;
use engine_game::status::{EffectClass, EffectModifier, StatusKind};
use engine_render_data::ui::{TextAlign, UiClip, UiDrawList, UiRect};

use super::super::map_style::{blend, color, radius, space, type_scale};
use super::{Control, Layout, PlaygroundAction, PlaygroundPanels, Style};
use crate::aura::{Aura, AuraKind, AuraList, fill_auras, status_aura};
use crate::ui::aura_row::{AuraRowStyle, DISPEL_BORDER, push_aura_icon};

/// One line of the effect library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectRow {
    /// "BUFFS" or "DEBUFFS".
    Heading(EffectClass),
    /// One registry entry.
    Effect(StatusKind),
}

/// One line of the target inspector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Item {
    /// A section title, with a count when it has one.
    Heading(&'static str, Option<usize>),
    /// Somebody effects can be pointed at.
    Target(EffectTarget),
    /// One stat, by its index in the stat lines.
    Stat(usize),
    /// One effect on the target.
    Effect(Aura),
    /// A quiet line: "No buffs".
    Note(&'static str),
}

/// Whether a stat is better, worse or the same as without effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Plain,
    Better,
    Worse,
}

/// One stat line, its value written into a reused buffer.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StatLine {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) tone: Tone,
}

/// The time-left stepper's stops, seconds.
const TIME_STOPS: [f32; 16] = [
    1.0, 2.0, 3.0, 5.0, 8.0, 10.0, 15.0, 20.0, 30.0, 45.0, 60.0, 90.0, 120.0, 180.0, 300.0, 600.0,
];

/// The longest time a tester may type, seconds.
pub(super) const TIME_MAX: f32 = 3600.0;

/// Every modifier of `kind`, as a line of words, prepared once.
pub(super) fn modifier_lines(kind: StatusKind) -> Vec<String> {
    let definition = kind.definition();
    let per = if definition.max_stacks > 1 {
        " per stack"
    } else {
        ""
    };
    let percent = |fraction: f32| {
        let value = fraction * 100.0;
        if (value - value.round()).abs() < 0.05 {
            format!("{:+.0}%", value)
        } else {
            format!("{value:+.1}%")
        }
    };
    definition
        .modifiers
        .iter()
        .map(|modifier| match *modifier {
            EffectModifier::Stat { stat, per_stack } => {
                let name = super::super::map_style::title_case(stat.name());
                if stat.is_rating() {
                    format!(
                        "{} {name}{per}",
                        percent(
                            engine_game::character::StatBlock::of(stat, per_stack).percent(stat)
                        )
                    )
                } else {
                    format!("{per_stack:+.0} {name}{per}")
                }
            }
            EffectModifier::MoveSpeed { per_stack } => {
                format!("{} movement speed{per}", percent(per_stack))
            }
            EffectModifier::Slow => format!(
                "Slower by its strength ({:.0}% here)",
                definition.generic.magnitude * 100.0
            ),
            EffectModifier::AttackSpeed { per_stack } => {
                format!("{} attack speed{per}", percent(per_stack))
            }
            EffectModifier::DamageTaken { schools, per_stack } => {
                format!(
                    "{} {} damage taken{per}",
                    percent(per_stack),
                    schools.describe()
                )
            }
            EffectModifier::DamageReduction { fraction } => {
                format!("{:.0}% less damage taken", fraction * 100.0)
            }
            EffectModifier::DamageReductionMagnitude => format!(
                "{:.0}% less damage taken (by its strength)",
                definition.generic.magnitude * 100.0
            ),
            EffectModifier::DamageDealt { per_stack } => {
                format!("{} damage dealt{per}", percent(per_stack))
            }
            EffectModifier::MaxHealth { per_stack } => {
                format!("{} maximum health{per}", percent(per_stack))
            }
            EffectModifier::HealingReduced => format!(
                "{:.0}% less healing received",
                definition.generic.magnitude * 100.0
            ),
            EffectModifier::Reflect { fraction } => {
                format!(
                    "{:.0}% of damage taken sent back at its author",
                    fraction * 100.0
                )
            }
        })
        .collect()
}

/// "1m 12s", "12s", "4.2s".
fn write_time(out: &mut String, seconds: f32) {
    if seconds >= 60.0 {
        let whole = seconds.floor() as u32;
        let _ = write!(out, "{}m {:02}s", whole / 60, whole % 60);
    } else if seconds >= 10.0 {
        let _ = write!(out, "{}s", seconds.floor() as u32);
    } else {
        let _ = write!(out, "{seconds:.1}s");
    }
}

/// The time-left value as the stepper prints it: "30s", "2m", "1m 30s".
pub(super) fn write_duration(out: &mut String, seconds: f32) {
    let whole = seconds.round() as u32;
    if whole >= 60 && whole % 60 == 0 {
        let _ = write!(out, "{}m", whole / 60);
    } else if whole >= 60 {
        let _ = write!(out, "{}m {}s", whole / 60, whole % 60);
    } else {
        let _ = write!(out, "{whole}s");
    }
}

/// The next stop up or down from `seconds`.
pub(super) fn step_time(seconds: f32, up: bool) -> f32 {
    if up {
        TIME_STOPS
            .into_iter()
            .find(|stop| *stop > seconds + 0.01)
            .unwrap_or(TIME_MAX.min(seconds.max(TIME_STOPS[TIME_STOPS.len() - 1])))
    } else {
        TIME_STOPS
            .into_iter()
            .rev()
            .find(|stop| *stop < seconds - 0.01)
            .unwrap_or(TIME_STOPS[0])
    }
}

/// The word under a hero's tab.
pub(super) fn hero_label(hero: Hero) -> &'static str {
    match hero {
        Hero::FireMage => "Fire Mage",
        Hero::Warrior => "Warrior",
        Hero::Druid => "Druid",
    }
}

impl PlaygroundPanels {
    /// Rebuilds the effect library from its search. Runs when the search changes.
    pub(super) fn rebuild_effect_rows(&mut self) {
        self.effect_rows.clear();
        for class in [EffectClass::Buff, EffectClass::Debuff] {
            let start = self.effect_rows.len();
            self.effect_rows.push(EffectRow::Heading(class));
            for kind in StatusKind::ALL {
                let definition = kind.definition();
                if definition.class == class && effect_matches(kind, &self.effect_search) {
                    self.effect_rows.push(EffectRow::Effect(kind));
                }
            }
            if self.effect_rows.len() == start + 1 {
                self.effect_rows.pop();
            }
        }
        self.list_scroll = 0.0;
    }

    /// How many effects the library shows.
    #[must_use]
    pub fn visible_effects(&self) -> usize {
        self.effect_rows
            .iter()
            .filter(|row| matches!(row, EffectRow::Effect(_)))
            .count()
    }

    pub(super) fn effect_row_height(&self, layout: &Layout, row: EffectRow) -> f32 {
        match row {
            EffectRow::Heading(_) => layout.heading_height,
            EffectRow::Effect(_) => layout.row_height,
        }
    }

    /// The effect the details card describes: the row under the pointer, else the chosen.
    pub(super) fn detail_effect(&self) -> Option<StatusKind> {
        if let Some(Control::Effect(row)) = self.hovered
            && let Some(EffectRow::Effect(kind)) = self.effect_rows.get(row)
        {
            return Some(*kind);
        }
        self.chosen_effect
    }

    /// Moves the keyboard cursor through the effect list, choosing as it goes.
    pub(super) fn step_effect(&mut self, delta: i32) {
        let kinds: Vec<StatusKind> = self
            .effect_rows
            .iter()
            .filter_map(|row| match row {
                EffectRow::Effect(kind) => Some(*kind),
                EffectRow::Heading(_) => None,
            })
            .collect();
        if kinds.is_empty() {
            return;
        }
        let at = self
            .chosen_effect
            .and_then(|chosen| kinds.iter().position(|kind| *kind == chosen));
        let next = match at {
            None if delta > 0 => 0,
            None => kinds.len() - 1,
            Some(at) => (at as i32 + delta).rem_euclid(kinds.len() as i32) as usize,
        };
        self.chosen_effect = Some(kinds[next]);
    }

    /// Rebuilds the inspector's lines for whoever the effects are being tried on. Runs once
    /// per update and once per draw; reuses every buffer.
    pub(super) fn rebuild_inspector(
        &mut self,
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) {
        playground.effect_targets(simulation, &mut self.targets);
        let target = playground.effect_target(simulation);
        self.items.clear();
        self.items
            .push(Item::Heading("EFFECT TARGET", Some(self.targets.len())));
        for entry in &self.targets {
            self.items.push(Item::Target(*entry));
        }

        // The stats, from the numbers gameplay multiplies by.
        self.items.push(Item::Heading("STATS", None));
        let mut used = 0;
        if let Some(stats) = simulation.effect_stats(target) {
            let bonus = simulation
                .world()
                .statuses(target)
                .map(engine_game::status::StatusSet::stat_bonus)
                .unwrap_or_default();
            let mut line = |label: &'static str, tone: Tone, write: &dyn Fn(&mut String)| {
                if self.stat_lines.len() <= used {
                    self.stat_lines.push(StatLine {
                        label,
                        value: String::with_capacity(24),
                        tone,
                    });
                }
                let entry = &mut self.stat_lines[used];
                entry.label = label;
                entry.tone = tone;
                entry.value.clear();
                write(&mut entry.value);
                used += 1;
            };
            line("Health", Tone::Plain, &|out| {
                super::push_thousands(out, stats.health);
                out.push_str(" / ");
                super::push_thousands(out, stats.max_health);
            });
            if let Some(sheet) = stats.sheet {
                use engine_game::character::Stat;
                for (label, stat) in [
                    ("Strength", Stat::Strength),
                    ("Intellect", Stat::Intellect),
                    ("Stamina", Stat::Stamina),
                    ("Critical strike", Stat::CriticalStrike),
                    ("Haste", Stat::Haste),
                    ("Versatility", Stat::Versatility),
                    ("Armor", Stat::Armor),
                ] {
                    let change = bonus.get(stat);
                    let tone = tone_of(change, true);
                    let value = sheet.get(stat);
                    let rating = stat.is_rating();
                    let percent = sheet.percent(stat) * 100.0;
                    // What the bonus is worth *on this sheet*: ratings are not linear, so the
                    // change is the difference between the sheet with and without it.
                    let mut without = sheet;
                    without.add(stat, -change);
                    let change_percent = (sheet.percent(stat) - without.percent(stat)) * 100.0;
                    line(label, tone, &move |out| {
                        if rating {
                            let _ = write!(out, "{percent:.1}%");
                            if change.abs() > 0.001 {
                                let _ = write!(out, "  ({change_percent:+.0}%)");
                            }
                        } else {
                            let _ = write!(out, "{value:.0}");
                            if change.abs() > 0.001 {
                                let _ = write!(out, "  ({change:+.0})");
                            }
                        }
                    });
                }
            }
            let speed = stats.move_speed;
            let speed_multiplier = stats.move_speed_multiplier;
            line(
                "Movement speed",
                tone_of(speed_multiplier - 1.0, true),
                &move |out| {
                    let _ = write!(out, "{speed:.1} m/s  (\u{d7}{speed_multiplier:.2})");
                },
            );
            let attack = stats.attack_speed;
            line("Attack speed", tone_of(attack - 1.0, true), &move |out| {
                let _ = write!(out, "\u{d7}{attack:.2}");
            });
            let dealt = stats.damage_dealt;
            line("Damage dealt", tone_of(dealt - 1.0, true), &move |out| {
                let _ = write!(out, "\u{d7}{dealt:.2}");
            });
            let taken = stats.damage_taken;
            let physical = taken[0].1;
            line(
                "Physical taken",
                tone_of(physical - 1.0, false),
                &move |out| {
                    let _ = write!(out, "\u{d7}{physical:.2}");
                },
            );
            let worst_magic = taken[1..]
                .iter()
                .map(|(_, multiplier)| *multiplier)
                .fold(f32::MIN, f32::max);
            let best_magic = taken[1..]
                .iter()
                .map(|(_, multiplier)| *multiplier)
                .fold(f32::MAX, f32::min);
            line(
                "Magic taken",
                tone_of(worst_magic.max(best_magic) - 1.0, false),
                &move |out| {
                    if (worst_magic - best_magic).abs() < 1e-4 {
                        let _ = write!(out, "\u{d7}{worst_magic:.2}");
                    } else {
                        let fire = taken[1].1;
                        let arcane = taken[2].1;
                        let nature = taken[3].1;
                        let _ = write!(
                            out,
                            "F\u{d7}{fire:.2} A\u{d7}{arcane:.2} N\u{d7}{nature:.2}"
                        );
                    }
                },
            );
            let reduction = stats.damage_reduction;
            line("Damage reduction", tone_of(reduction, true), &move |out| {
                let _ = write!(out, "{:.0}%", reduction * 100.0);
            });
            if stats.sheet.is_none() {
                self.items
                    .push(Item::Note("No character sheet: stats apply to players"));
            }
        }
        for index in 0..used {
            self.items.push(Item::Stat(index));
        }

        // What is on them: every status, uncapped — this is where the interface's own limit
        // is tested, so it must not be the limit — and the class effects the frames show,
        // each converted exactly as the nameplate and the frames convert it.
        fill_auras(simulation, target, &mut self.auras);
        self.effect_auras.clear();
        if let Some(statuses) = simulation.world().statuses(target) {
            self.effect_auras.extend(statuses.iter().map(status_aura));
        }
        self.effect_auras.extend(
            self.auras
                .iter()
                .filter(|aura| !matches!(aura.kind, AuraKind::Status(_)))
                .copied(),
        );
        let total = self.effect_auras.len();
        let buffs = self
            .effect_auras
            .iter()
            .filter(|aura| !aura.is_harmful())
            .count();
        let debuffs = total - buffs;
        self.items.push(Item::Heading("BUFFS", Some(buffs)));
        if buffs == 0 {
            self.items.push(Item::Note("No buffs"));
        }
        for aura in self.effect_auras.iter().filter(|aura| !aura.is_harmful()) {
            self.items.push(Item::Effect(*aura));
        }
        self.items.push(Item::Heading("DEBUFFS", Some(debuffs)));
        if debuffs == 0 {
            self.items.push(Item::Note("No debuffs"));
        }
        for aura in self.effect_auras.iter().filter(|aura| aura.is_harmful()) {
            self.items.push(Item::Effect(*aura));
        }
        if total > AuraList::CAPACITY {
            self.items.push(Item::Note(
                "Over the HUD's limit: plates and frames show 20",
            ));
        }
    }

    pub(super) fn item_height(&self, layout: &Layout, item: &Item) -> f32 {
        let s = &self.style;
        match item {
            Item::Heading(..) => s.px(28.0),
            Item::Target(_) => s.px(26.0),
            Item::Stat(_) => s.px(20.0),
            Item::Effect(_) => layout.row_height,
            Item::Note(_) => s.px(22.0),
        }
    }

    pub(super) fn inspector_height(&self, layout: &Layout) -> f32 {
        self.items
            .iter()
            .map(|item| self.item_height(layout, item))
            .sum()
    }

    /// The remove button on an effect line.
    pub(super) fn remove_rect(&self, line: UiRect) -> UiRect {
        let size = self.style.px(20.0);
        UiRect::new(
            line.right() - size - self.style.px(space::SM),
            line.y + (line.height - size) * 0.5,
            size,
            size,
        )
    }

    /// Which effects-mode control is under `point`, in the list or the inspector.
    pub(super) fn effect_control_at(&self, layout: &Layout, point: (f32, f32)) -> Option<Control> {
        let (x, y) = point;
        if layout.list.contains(x, y) {
            let local = y - layout.list.y + self.list_scroll;
            let mut top = 0.0;
            for (index, row) in self.effect_rows.iter().enumerate() {
                let height = self.effect_row_height(layout, *row);
                if local >= top && local < top + height {
                    return matches!(row, EffectRow::Effect(_)).then_some(Control::Effect(index));
                }
                top += height;
            }
            return None;
        }
        if layout.encounter.contains(x, y) {
            let mut top = layout.encounter.y - self.encounter_scroll;
            for (index, item) in self.items.iter().enumerate() {
                let height = self.item_height(layout, item);
                let line = UiRect::new(layout.encounter.x, top, layout.encounter.width, height);
                top += height;
                if !line.contains(x, y) {
                    continue;
                }
                return match item {
                    Item::Target(_) => Some(Control::Inspector(index)),
                    Item::Effect(aura) if removable(aura) => {
                        if self.remove_rect(line).contains(x, y) {
                            Some(Control::RemoveEffect(index))
                        } else {
                            Some(Control::Inspector(index))
                        }
                    }
                    Item::Effect(_) => Some(Control::Inspector(index)),
                    _ => None,
                };
            }
        }
        None
    }

    /// What a click on an effects-mode control asks for.
    pub(super) fn click_effects(
        &mut self,
        control: Control,
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) -> PlaygroundAction {
        match control {
            Control::Effect(row) => {
                if let Some(EffectRow::Effect(kind)) = self.effect_rows.get(row) {
                    self.chosen_effect = Some(*kind);
                }
                PlaygroundAction::None
            }
            Control::TimeDown => {
                self.duration = step_time(self.duration, false);
                PlaygroundAction::None
            }
            Control::TimeUp => {
                self.duration = step_time(self.duration, true);
                PlaygroundAction::None
            }
            Control::TimeValue => {
                self.typing = true;
                self.field = super::Field::Time;
                self.time_text.clear();
                PlaygroundAction::None
            }
            Control::StacksDown => {
                self.stacks = self.stacks.saturating_sub(1).max(1);
                PlaygroundAction::None
            }
            Control::StacksUp => {
                let ceiling = self.chosen_effect.map_or(u8::MAX, StatusKind::max_stacks);
                self.stacks = (self.stacks + 1).min(ceiling.max(1));
                PlaygroundAction::None
            }
            Control::Spawn => match self.chosen_effect {
                Some(kind) if kind.definition().applicable => PlaygroundAction::ApplyEffect {
                    kind,
                    duration: self.duration,
                    stacks: self.stacks.min(kind.max_stacks()),
                },
                _ => PlaygroundAction::None,
            },
            Control::TestEffects => PlaygroundAction::AddTestEffects,
            Control::ClearBuffs => PlaygroundAction::ClearEffects { buffs: true },
            Control::ClearDebuffs => PlaygroundAction::ClearEffects { buffs: false },
            Control::RemoveEffect(index) => match self.items.get(index) {
                Some(Item::Effect(Aura {
                    kind: AuraKind::Status(kind),
                    ..
                })) => PlaygroundAction::RemoveEffect(*kind),
                _ => PlaygroundAction::None,
            },
            Control::Inspector(index) => match self.items.get(index) {
                Some(Item::Target(entry)) => PlaygroundAction::SetEffectTarget(entry.entity),
                Some(Item::Effect(Aura {
                    kind: AuraKind::Status(kind),
                    ..
                })) => {
                    // Clicking a line picks that effect in the library, for applying again.
                    self.chosen_effect = Some(*kind);
                    PlaygroundAction::None
                }
                _ => PlaygroundAction::None,
            },
            _ => {
                let _ = (playground, simulation);
                PlaygroundAction::None
            }
        }
    }

    /// The name an effect target is listed under.
    fn target_name<'a>(&'a self, entry: &EffectTarget, simulation: &Simulation) -> &'a str {
        match entry.role {
            TargetRole::Player => match simulation.hero() {
                Hero::FireMage => "You (Fire Mage)",
                Hero::Warrior => "You (Warrior)",
                Hero::Druid => "You (Druid)",
            },
            TargetRole::Friendly => "Healing Dummy",
            TargetRole::Dummy => "Training Dummy",
            TargetRole::Monster(kind) => &self.entries[self.by_kind[kind.index()]].name,
        }
    }

    /// The left panel's list, card and footer in Effects mode.
    pub(super) fn push_effect_library(
        &mut self,
        list: &mut UiDrawList,
        layout: &Layout,
        playground: &MonsterPlayground,
        simulation: &Simulation,
        scratch: &mut String,
    ) {
        let s = self.style;
        list.panel(
            layout.list,
            s.px(radius::CARD),
            color::SURFACE_1,
            color::SURFACE_1,
        );
        list.clip(UiClip::rect(layout.list));
        let mut top = layout.list.y - self.list_scroll;
        for (index, row) in self.effect_rows.iter().enumerate() {
            let height = self.effect_row_height(layout, *row);
            let rect = UiRect::new(layout.list.x, top, layout.list.width, height);
            top += height;
            if rect.bottom() < layout.list.y || rect.y > layout.list.bottom() {
                continue;
            }
            match row {
                EffectRow::Heading(class) => {
                    let (title, tint) = match class {
                        EffectClass::Buff => ("BUFFS", color::VERDANT),
                        EffectClass::Debuff => ("DEBUFFS", color::CRIMSON),
                    };
                    self.text(
                        list,
                        (
                            rect.x + s.px(space::MD),
                            s.centred_top(type_scale::LABEL, rect.y + rect.height * 0.62),
                        ),
                        title,
                        type_scale::LABEL,
                        tint,
                        TextAlign::Left,
                    );
                }
                EffectRow::Effect(kind) => {
                    let chosen = self.chosen_effect == Some(*kind);
                    let hovered = self.hovered == Some(Control::Effect(index));
                    let inset = UiRect::new(
                        rect.x + s.px(space::XS),
                        rect.y + s.px(1.0),
                        rect.width - s.px(space::XS) * 2.0,
                        rect.height - s.px(2.0),
                    );
                    if chosen {
                        list.panel(
                            inset,
                            s.px(radius::CONTROL),
                            blend(color::SURFACE_2, color::BRONZE, 0.12),
                            color::SURFACE_2,
                        );
                        list.panel_stroke(inset, s.px(radius::CONTROL), s.px(1.0), color::BRONZE);
                    } else if hovered {
                        list.panel(
                            inset,
                            s.px(radius::CONTROL),
                            color::SURFACE_2,
                            color::SURFACE_2,
                        );
                    }
                    let definition = kind.definition();
                    let edge = s.px(20.0);
                    let icon = UiRect::new(
                        inset.x + s.px(space::SM),
                        inset.y + (inset.height - edge) * 0.5,
                        edge,
                        edge,
                    );
                    let aura = Aura::timed(AuraKind::Status(*kind), 0.0, 0.0);
                    push_aura_icon(
                        list,
                        icon,
                        &aura,
                        AuraRowStyle::new(edge, 1, 1).with_time(false),
                        scratch,
                    );
                    let tag = tag_of(*kind);
                    let tag_width = s.width(type_scale::META, tag.0);
                    let centre = rect.y + rect.height * 0.5;
                    let name_x = icon.right() + s.px(space::SM);
                    self.fitted(
                        list,
                        (name_x, s.centred_top(type_scale::UI, centre)),
                        definition.name,
                        type_scale::UI,
                        inset.right() - s.px(space::SM) - tag_width - s.px(space::SM) - name_x,
                        if !definition.applicable {
                            color::TEXT_MUTED
                        } else if chosen {
                            color::TEXT_PRIMARY
                        } else {
                            color::TEXT_SECONDARY
                        },
                    );
                    self.text(
                        list,
                        (
                            inset.right() - s.px(space::SM),
                            s.centred_top(type_scale::META, centre),
                        ),
                        tag.0,
                        type_scale::META,
                        tag.1,
                        TextAlign::Right,
                    );
                }
            }
        }
        if self.effect_rows.is_empty() {
            self.text(
                list,
                (
                    layout.list.x + layout.list.width * 0.5,
                    layout.list.y + s.px(space::LG),
                ),
                "No effect matches",
                type_scale::UI,
                color::TEXT_MUTED,
                TextAlign::Center,
            );
        }
        list.clip(UiClip::NONE);
        list.panel_stroke(layout.list, s.px(radius::CARD), s.px(1.0), color::BORDER);
        let content: f32 = self
            .effect_rows
            .iter()
            .map(|row| self.effect_row_height(layout, *row))
            .sum();
        self.scrollbar(list, layout.list, self.list_scroll, content);

        if let Some(rect) = layout.detail {
            match self.detail_effect() {
                Some(kind) => self.push_effect_detail(list, rect, kind, scratch),
                None => self.push_hint_card(
                    list,
                    rect,
                    "Choose an effect",
                    "to see what it does and apply it",
                ),
            }
        }

        // The footer: how long, how many, and the one action.
        let centre = layout.time_value.y + layout.time_value.height * 0.5;
        self.text(
            list,
            (
                layout.left.x + s.px(space::LG),
                s.centred_top(type_scale::LABEL, centre),
            ),
            "TIME",
            type_scale::LABEL,
            color::BRONZE,
            TextAlign::Left,
        );
        self.button(
            list,
            layout.time_down,
            "-",
            Style::Secondary,
            self.hovered == Some(Control::TimeDown),
            true,
        );
        let typing_time = self.typing && self.field == super::Field::Time;
        scratch.clear();
        if typing_time {
            scratch.push_str(&self.time_text);
            scratch.push('|');
        } else {
            write_duration(scratch, self.duration);
        }
        let hovered = self.hovered == Some(Control::TimeValue);
        if typing_time || hovered {
            list.panel_stroke(
                layout.time_value,
                s.px(radius::CHIP),
                s.px(1.0),
                if typing_time {
                    color::BRONZE
                } else {
                    color::BORDER_STRONG
                },
            );
        }
        self.text(
            list,
            (
                layout.time_value.x + layout.time_value.width * 0.5,
                s.centred_top(type_scale::UI, centre),
            ),
            scratch,
            type_scale::UI,
            color::TEXT_PRIMARY,
            TextAlign::Center,
        );
        self.button(
            list,
            layout.time_up,
            "+",
            Style::Secondary,
            self.hovered == Some(Control::TimeUp),
            true,
        );
        // The word only where it fits; the value says what it is either way.
        let label_width = s.width(type_scale::LABEL, "STACKS");
        if layout.time_up.right() + label_width + s.px(space::SM) <= layout.stacks_down.x {
            self.text(
                list,
                (
                    layout.stacks_down.x - s.px(space::XS),
                    s.centred_top(type_scale::LABEL, centre),
                ),
                "STACKS",
                type_scale::LABEL,
                color::BRONZE,
                TextAlign::Right,
            );
        }
        let ceiling = self.chosen_effect.map_or(1, StatusKind::max_stacks);
        let stacks = self.stacks.min(ceiling);
        self.button(
            list,
            layout.stacks_down,
            "-",
            Style::Secondary,
            self.hovered == Some(Control::StacksDown),
            stacks > 1,
        );
        scratch.clear();
        let _ = write!(scratch, "\u{d7}{stacks}");
        self.text(
            list,
            (
                (layout.stacks_down.right() + layout.stacks_up.x) * 0.5,
                s.centred_top(type_scale::UI, centre),
            ),
            scratch,
            type_scale::UI,
            color::TEXT_PRIMARY,
            TextAlign::Center,
        );
        self.button(
            list,
            layout.stacks_up,
            "+",
            Style::Secondary,
            self.hovered == Some(Control::StacksUp),
            stacks < ceiling,
        );

        let target = playground.effect_target(simulation);
        let entry = self
            .targets
            .iter()
            .find(|entry| entry.entity == target)
            .copied()
            .unwrap_or(EffectTarget {
                entity: target,
                role: TargetRole::Player,
            });
        scratch.clear();
        scratch.push_str("APPLY TO ");
        let name = self.target_name(&entry, simulation);
        let short = name.split(" (").next().unwrap_or(name);
        for character in short.chars() {
            scratch.extend(character.to_uppercase());
        }
        let enabled = self
            .chosen_effect
            .is_some_and(|kind| kind.definition().applicable);
        let label = self.style.fit(
            type_scale::HEADING,
            scratch,
            layout.spawn.width - s.px(space::LG),
        );
        self.button(
            list,
            layout.spawn,
            &label,
            Style::Primary,
            self.hovered == Some(Control::Spawn),
            enabled,
        );
    }

    fn push_effect_detail(
        &self,
        list: &mut UiDrawList,
        rect: UiRect,
        kind: StatusKind,
        scratch: &mut String,
    ) {
        let s = &self.style;
        let definition = kind.definition();
        let accent = match definition.class {
            EffectClass::Buff => color::VERDANT,
            EffectClass::Debuff => color::CRIMSON,
        };
        list.panel(rect, s.px(radius::CARD), color::SURFACE_2, color::SURFACE_1);
        list.panel(
            UiRect::new(
                rect.x,
                rect.y + s.px(space::MD),
                s.px(3.0),
                rect.height - s.px(space::MD) * 2.0,
            ),
            s.px(1.5),
            accent,
            accent,
        );
        let x = rect.x + s.px(space::MD);
        let width = rect.width - s.px(space::MD) * 2.0;
        let mut y = rect.y + s.px(space::MD);
        let edge = s.px(28.0);
        let aura = Aura::timed(AuraKind::Status(kind), 0.0, 0.0);
        push_aura_icon(
            list,
            UiRect::new(x, y, edge, edge),
            &aura,
            AuraRowStyle::new(edge, 1, 1).with_time(false),
            scratch,
        );
        let name_x = x + edge + s.px(space::SM);
        self.fitted(
            list,
            (name_x, y),
            definition.name,
            type_scale::NAME,
            x + width - name_x,
            color::TEXT_PRIMARY,
        );
        y += s.line(type_scale::NAME);
        scratch.clear();
        scratch.push_str(match definition.class {
            EffectClass::Buff => "Buff",
            EffectClass::Debuff => "Debuff",
        });
        match (kind.is_dispellable(), definition.dispel) {
            (true, Some(school)) => {
                let _ = write!(scratch, " \u{b7} {} (dispellable)", school_name(school));
            }
            _ if definition.class == EffectClass::Debuff => {
                scratch.push_str(" \u{b7} not dispellable")
            }
            _ => {}
        }
        if definition.max_stacks > 1 {
            let _ = write!(scratch, " \u{b7} stacks to {}", definition.max_stacks);
        }
        self.fitted(
            list,
            (name_x, y),
            scratch,
            type_scale::META,
            x + width - name_x,
            if kind.is_dispellable() {
                DISPEL_BORDER
            } else {
                color::TEXT_SECONDARY
            },
        );
        y = (y + s.line(type_scale::META)).max(rect.y + s.px(space::MD) + edge + s.px(space::XS));
        for line in s
            .wrap(type_scale::META, definition.description, width)
            .iter()
            .take(2)
        {
            self.text(
                list,
                (x, y),
                line,
                type_scale::META,
                color::TEXT_MUTED,
                TextAlign::Left,
            );
            y += s.line(type_scale::META);
        }
        y += s.px(space::XS);
        self.text(
            list,
            (x, y),
            "WHAT IT CHANGES",
            type_scale::LABEL,
            color::BRONZE,
            TextAlign::Left,
        );
        y += s.line(type_scale::LABEL);
        let lines = &self.effect_lines[kind.index()];
        if lines.is_empty() {
            let note = if definition.heals {
                "Heals every second; no stats"
            } else if definition.generic.per_tick > 0.0 {
                "Damage every second; no stats"
            } else if !definition.applicable {
                "Owned by the respawn system; not applied here"
            } else {
                "A mechanic, not a stat"
            };
            self.text(
                list,
                (x, y),
                note,
                type_scale::META,
                color::TEXT_SECONDARY,
                TextAlign::Left,
            );
            y += s.line(type_scale::META);
        }
        for line in lines.iter().take(4) {
            self.fitted(list, (x, y), line, type_scale::META, width, color::TEAL);
            y += s.line(type_scale::META);
        }
        scratch.clear();
        scratch.push_str("Lasts ");
        write_duration(scratch, definition.generic.duration);
        scratch.push_str(" by default");
        if definition.generic.per_tick > 0.0 {
            let _ = write!(
                scratch,
                " \u{b7} {:.0} {} a second",
                definition.generic.per_tick,
                if definition.heals {
                    "healing"
                } else {
                    "damage"
                }
            );
        }
        let bottom = rect.bottom() - s.px(space::MD) - s.line(type_scale::META);
        self.fitted(
            list,
            (x, y.max(bottom)),
            scratch,
            type_scale::META,
            width,
            color::TEXT_MUTED,
        );
    }

    /// The right panel's lower half in Effects mode: target, stats, buffs, debuffs.
    pub(super) fn push_inspector(
        &mut self,
        list: &mut UiDrawList,
        layout: &Layout,
        playground: &MonsterPlayground,
        simulation: &Simulation,
        scratch: &mut String,
    ) {
        let s = self.style;
        let window = layout.encounter;
        list.panel(
            window,
            s.px(radius::CARD),
            color::SURFACE_1,
            color::SURFACE_1,
        );
        list.clip(UiClip::rect(window));
        let target = playground.effect_target(simulation);
        let mut top = window.y - self.encounter_scroll;
        let x = window.x + s.px(space::MD);
        let right = window.right() - s.px(space::MD);
        for (index, item) in self.items.iter().enumerate() {
            let height = self.item_height(layout, item);
            let line = UiRect::new(window.x, top, window.width, height);
            top += height;
            if line.bottom() < window.y || line.y > window.bottom() {
                continue;
            }
            let centre = line.y + line.height * 0.5;
            match item {
                Item::Heading(title, count) => {
                    let tint = match *title {
                        "BUFFS" => color::VERDANT,
                        "DEBUFFS" => color::CRIMSON,
                        _ => color::BRONZE,
                    };
                    let top_y = s.centred_top(type_scale::LABEL, line.y + line.height * 0.62);
                    self.text(
                        list,
                        (x, top_y),
                        title,
                        type_scale::LABEL,
                        tint,
                        TextAlign::Left,
                    );
                    if let Some(count) = count {
                        scratch.clear();
                        let _ = write!(scratch, "{count}");
                        self.text(
                            list,
                            (right, top_y),
                            scratch,
                            type_scale::LABEL,
                            color::TEXT_MUTED,
                            TextAlign::Right,
                        );
                    }
                    list.rect(
                        UiRect::new(x, line.bottom() - s.px(2.0), right - x, s.px(1.0)),
                        color::BORDER,
                    );
                }
                Item::Target(entry) => {
                    let chosen = entry.entity == target;
                    let hovered = self.hovered == Some(Control::Inspector(index));
                    if chosen || hovered {
                        let inset = UiRect::new(
                            line.x + s.px(space::XS),
                            line.y + s.px(1.0),
                            line.width - s.px(space::XS) * 2.0,
                            line.height - s.px(2.0),
                        );
                        list.panel(
                            inset,
                            s.px(radius::CONTROL),
                            if chosen {
                                blend(color::SURFACE_2, color::BRONZE, 0.12)
                            } else {
                                color::SURFACE_2
                            },
                            color::SURFACE_2,
                        );
                    }
                    // A radio dot: filled for the one chosen.
                    let dot = s.px(5.0);
                    let at = (x + dot, centre);
                    list.disc(at, dot, color::BORDER_STRONG);
                    list.disc(
                        at,
                        dot - s.px(1.5),
                        if chosen {
                            color::BRONZE_BRIGHT
                        } else {
                            color::SURFACE_0_BOTTOM
                        },
                    );
                    let (tag, tint) = match entry.role {
                        TargetRole::Player => ("you", color::TEAL),
                        TargetRole::Friendly => ("friendly", color::VERDANT),
                        TargetRole::Dummy => ("dummy", color::TEXT_MUTED),
                        TargetRole::Monster(_) => ("hostile", color::CRIMSON),
                    };
                    let tag_width = s.width(type_scale::META, tag);
                    let name_x = x + dot * 2.0 + s.px(space::SM);
                    let name = self.target_name(entry, simulation);
                    self.fitted(
                        list,
                        (name_x, s.centred_top(type_scale::UI, centre)),
                        name,
                        type_scale::UI,
                        right - tag_width - s.px(space::SM) - name_x,
                        if chosen {
                            color::TEXT_PRIMARY
                        } else {
                            color::TEXT_SECONDARY
                        },
                    );
                    self.text(
                        list,
                        (right, s.centred_top(type_scale::META, centre)),
                        tag,
                        type_scale::META,
                        tint,
                        TextAlign::Right,
                    );
                }
                Item::Stat(stat) => {
                    let entry = &self.stat_lines[*stat];
                    let top_y = s.centred_top(type_scale::META, centre);
                    self.text(
                        list,
                        (x, top_y),
                        entry.label,
                        type_scale::META,
                        color::TEXT_SECONDARY,
                        TextAlign::Left,
                    );
                    self.text(
                        list,
                        (right, top_y),
                        &entry.value,
                        type_scale::META,
                        match entry.tone {
                            Tone::Plain => color::TEXT_PRIMARY,
                            Tone::Better => color::TEAL,
                            Tone::Worse => color::CRIMSON,
                        },
                        TextAlign::Right,
                    );
                }
                Item::Note(note) => {
                    self.fitted(
                        list,
                        (x, s.centred_top(type_scale::META, centre)),
                        note,
                        type_scale::META,
                        right - x,
                        color::TEXT_MUTED,
                    );
                }
                Item::Effect(aura) => {
                    self.push_effect_line(list, line, index, aura, scratch);
                }
            }
        }
        list.clip(UiClip::NONE);
        list.panel_stroke(window, s.px(radius::CARD), s.px(1.0), color::BORDER);
        let content = self.inspector_height(layout);
        self.scrollbar(list, window, self.encounter_scroll, content);
    }

    /// One effect on the target, on its own line: icon, name, stacks, time left, remove.
    fn push_effect_line(
        &self,
        list: &mut UiDrawList,
        line: UiRect,
        index: usize,
        aura: &Aura,
        scratch: &mut String,
    ) {
        let s = &self.style;
        let hovered = matches!(
            self.hovered,
            Some(Control::Inspector(at) | Control::RemoveEffect(at)) if at == index
        );
        let inset = UiRect::new(
            line.x + s.px(space::XS),
            line.y + s.px(1.0),
            line.width - s.px(space::XS) * 2.0,
            line.height - s.px(2.0),
        );
        if hovered {
            list.panel(
                inset,
                s.px(radius::CONTROL),
                color::SURFACE_2,
                color::SURFACE_2,
            );
        }
        let edge = s.px(22.0);
        let icon = UiRect::new(
            inset.x + s.px(space::SM),
            inset.y + (inset.height - edge) * 0.5,
            edge,
            edge,
        );
        // The same widget the nameplate and the frames draw an effect with: art, wipe,
        // stacks, and the blue border a dispellable debuff wears.
        push_aura_icon(
            list,
            icon,
            aura,
            AuraRowStyle::new(edge, 1, 1).with_time(false),
            scratch,
        );
        let centre = line.y + line.height * 0.5;
        let remove = self.remove_rect(line);
        let removable = removable(aura);
        let mut right = if removable {
            remove.x - s.px(space::SM)
        } else {
            inset.right() - s.px(space::SM)
        };
        scratch.clear();
        if aura.indefinite {
            scratch.push_str("until removed");
        } else if aura.remaining > 0.0 {
            write_time(scratch, aura.remaining);
        }
        let urgent = !aura.indefinite && aura.remaining > 0.0 && aura.remaining < 5.0;
        self.text(
            list,
            (right, s.centred_top(type_scale::META, centre)),
            scratch,
            type_scale::META,
            if urgent {
                color::AMBER
            } else {
                color::TEXT_SECONDARY
            },
            TextAlign::Right,
        );
        right -= s.width(type_scale::META, scratch) + s.px(space::SM);
        if aura.stacks > 1 {
            scratch.clear();
            let _ = write!(scratch, "\u{d7}{}", aura.stacks);
            self.text(
                list,
                (right, s.centred_top(type_scale::META, centre)),
                scratch,
                type_scale::META,
                color::BRONZE_BRIGHT,
                TextAlign::Right,
            );
            right -= s.width(type_scale::META, scratch) + s.px(space::SM);
        }
        let name_x = icon.right() + s.px(space::SM);
        self.fitted(
            list,
            (name_x, s.centred_top(type_scale::UI, centre)),
            aura.kind.name(),
            type_scale::UI,
            right - name_x,
            if aura.is_dispellable() {
                blend(color::TEXT_PRIMARY, DISPEL_BORDER, 0.35)
            } else {
                color::TEXT_PRIMARY
            },
        );
        if removable {
            let over = self.hovered == Some(Control::RemoveEffect(index));
            if over {
                list.panel(
                    remove,
                    s.px(radius::CHIP),
                    color::SURFACE_3,
                    color::SURFACE_3,
                );
            }
            self.text(
                list,
                (
                    remove.x + remove.width * 0.5,
                    s.centred_top(type_scale::UI, remove.y + remove.height * 0.5),
                ),
                "\u{d7}",
                type_scale::UI,
                if over {
                    color::CRIMSON
                } else {
                    color::TEXT_MUTED
                },
                TextAlign::Center,
            );
        }
    }
}

/// Whether the inspector may take this effect off by hand: a status the registry lets
/// anything apply. Class timers and a monster's mind are not statuses.
fn removable(aura: &Aura) -> bool {
    matches!(aura.kind, AuraKind::Status(kind) if kind.definition().applicable)
}

/// Whether every word of `query` is in the effect's name, id or school.
fn effect_matches(kind: StatusKind, query: &str) -> bool {
    let definition = kind.definition();
    query.split_whitespace().all(|word| {
        let word = word.to_ascii_lowercase();
        definition.name.to_ascii_lowercase().contains(&word)
            || definition.id.contains(&word)
            || definition
                .dispel
                .is_some_and(|school| school_name(school).to_ascii_lowercase().contains(&word))
            || match definition.class {
                EffectClass::Buff => "buff".contains(&word),
                EffectClass::Debuff => "debuff".contains(&word),
            }
    })
}

/// The tag a library row shows: the dispel school in its colour, or what it is.
fn tag_of(kind: StatusKind) -> (&'static str, engine_render_data::ui::UiColor) {
    let definition = kind.definition();
    if !definition.applicable {
        return ("system", color::TEXT_MUTED);
    }
    match (definition.class, definition.dispel) {
        (EffectClass::Debuff, Some(school)) => (school_name(school), school_color(school)),
        (EffectClass::Debuff, None) => ("Debuff", color::TEXT_MUTED),
        (EffectClass::Buff, _) => ("Buff", color::TEXT_MUTED),
    }
}

fn school_name(school: engine_game::status::DispelSchool) -> &'static str {
    use engine_game::status::DispelSchool;
    match school {
        DispelSchool::Magic => "Magic",
        DispelSchool::Poison => "Poison",
        DispelSchool::Curse => "Curse",
        DispelSchool::Disease => "Disease",
    }
}

/// The school colours the aura tooltips already use.
fn school_color(school: engine_game::status::DispelSchool) -> engine_render_data::ui::UiColor {
    use engine_game::status::DispelSchool;
    match school {
        DispelSchool::Magic => DISPEL_BORDER,
        DispelSchool::Poison => color::VERDANT,
        DispelSchool::Curse => color::ARCANE,
        DispelSchool::Disease => color::AMBER,
    }
}

/// Better, worse or the same, for a change: `higher_is_better` says which way is good.
fn tone_of(change: f32, higher_is_better: bool) -> Tone {
    if change.abs() < 1e-4 {
        Tone::Plain
    } else if (change > 0.0) == higher_is_better {
        Tone::Better
    } else {
        Tone::Worse
    }
}
