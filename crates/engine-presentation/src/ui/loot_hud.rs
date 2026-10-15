//! Loot on the HUD: the pill over a waiting chest, and the toast that says what you got.
//!
//! ```text
//!            ┌──────────────────┐
//!            │ [E]  Open chest  │            world-anchored, above the chest
//!            └──────────────────┘
//!
//!      ┌───────────────────────────────────────────┐
//!      │ ┌────┐  YOU RECEIVED                       │   centre-bottom, five seconds,
//!      │ │icon│  Ember-Wrought Band                 │   hover it for the item tooltip
//!      │ └────┘  +2 Haste   +2 Intellect            │
//!      └───────────────────────────────────────────┘
//! ```
//!
//! # Events in, pixels out
//!
//! The toast is made from the `LootAwarded` and `LootMailed` events the simulation emits for
//! the *local* player - the same events offline and online, because online they are decoded
//! off the wire and injected into the client simulation. The module keeps no inventory and
//! decides nothing; it shows what happened, for as long as it takes to read.

use engine_game::Simulation;
use engine_game::character::{ItemId, ItemProps, ItemStack, MailReason, item};
use engine_game::event::CombatEvent;
use engine_render_data::ui::{TextAlign, UiDrawList, UiRect};

use super::character_menu::{icon_for_item, quality_color};
use super::kit::{self, Kit, tokens};
use super::map_style::{blend, color, push_title_case, radius, type_scale};
use super::tooltip::{ItemContext, ItemPlace, ItemTooltip};
use crate::game_ui::write_integer;
use crate::snapshot::CharacterView;

/// How long a toast stays up, in seconds.
pub const TOAST_SECONDS: f32 = 5.0;
/// How long it takes to slide in, and out again at the end.
const SLIDE_SECONDS: f32 = 0.3;
/// How long a toast stays when more are waiting behind it.
const QUEUED_TOAST_SECONDS: f32 = 2.5;
/// How long the currency line stays after the last drop.
const CURRENCY_SECONDS: f32 = 4.0;

/// What the toast reports.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Toast {
    item: ItemId,
    /// The copy's quality, enhancement and broken state.
    props: ItemProps,
    /// A monster's drop: the headline names its rarity (`RARE DROP`).
    drop: bool,
    /// `None` for an item put in the bag, the reason for one sent to the mail.
    mailed: Option<MailReason>,
    age: f32,
}

/// The loot toast, with the storage its tooltip needs.
#[derive(Debug, Default)]
pub struct LootHud {
    toast: Option<Toast>,
    /// Toasts waiting their turn: a chest gives several items at once.
    queue: std::collections::VecDeque<Toast>,
    /// Currency picked up recently: resource id and amount, merged.
    currency: Vec<(&'static str, u32)>,
    /// Seconds since the last currency drop.
    currency_age: f32,
    tooltip: ItemTooltip,
}

impl LootHud {
    /// No toast.
    #[must_use]
    pub fn new() -> Self {
        Self {
            toast: None,
            queue: std::collections::VecDeque::new(),
            currency: Vec::new(),
            currency_age: 0.0,
            tooltip: ItemTooltip::new(),
        }
    }

    /// Removes the toasts and the currency line.
    pub fn clear(&mut self) {
        self.toast = None;
        self.queue.clear();
        self.currency.clear();
    }

    /// Shows `toast` now, or after the ones ahead of it.
    fn show(&mut self, toast: Toast) {
        if self.toast.is_some() {
            self.queue.push_back(toast);
        } else {
            self.toast = Some(toast);
        }
    }

    /// Whether a toast is up.
    #[must_use]
    pub fn is_showing(&self) -> bool {
        self.toast.is_some()
    }

    /// Reads the frame's events and starts a toast for the local player's loot.
    ///
    /// Returns whether the character changed - an item arrived in the bag or went to the mail
    /// - which is when the application writes it to disk.
    pub fn observe(&mut self, simulation: &Simulation) -> bool {
        let local = simulation.player().entity;
        let mut changed = false;
        for event in simulation.events() {
            match *event {
                CombatEvent::LootAwarded {
                    player,
                    item,
                    props,
                    ..
                } if player == local => {
                    self.show(Toast {
                        item,
                        props,
                        drop: false,
                        mailed: None,
                        age: 0.0,
                    });
                    changed = true;
                }
                CombatEvent::LootMailed {
                    player,
                    item,
                    props,
                    reason,
                    ..
                } if player == local => {
                    self.show(Toast {
                        item,
                        props,
                        drop: false,
                        mailed: Some(reason),
                        age: 0.0,
                    });
                    changed = true;
                }
                // A monster's drop is mailed by the server: the character changes there.
                CombatEvent::MobLoot {
                    player,
                    item,
                    props,
                    ..
                } if player == local => self.show(Toast {
                    item,
                    props,
                    drop: true,
                    mailed: Some(MailReason::DungeonDrop),
                    age: 0.0,
                }),
                CombatEvent::MobCurrency {
                    player,
                    resource,
                    amount,
                    ..
                } if player == local => {
                    match self.currency.iter_mut().find(|(id, _)| *id == resource) {
                        Some((_, held)) => *held = held.saturating_add(amount),
                        None => self.currency.push((resource, amount)),
                    }
                    self.currency_age = 0.0;
                }
                _ => {}
            }
        }
        changed
    }

    /// Ages the toast by `delta` real seconds, ending it when its time is up; the next queued
    /// one follows. While others wait, a toast is shown for a shorter time.
    pub fn advance(&mut self, delta: f32) {
        self.currency_age += delta;
        if self.currency_age >= CURRENCY_SECONDS {
            self.currency.clear();
        }
        let limit = if self.queue.is_empty() {
            TOAST_SECONDS
        } else {
            QUEUED_TOAST_SECONDS
        };
        if let Some(toast) = self.toast.as_mut() {
            toast.age += delta;
            if toast.age >= limit {
                self.toast = self.queue.pop_front();
            }
        }
    }

    /// The currency line (`+12 Gold  +1 Aether Shard`) while there is something to say.
    #[must_use]
    pub fn currency_line(&self) -> Option<String> {
        if self.currency.is_empty() {
            return None;
        }
        let mut line = String::with_capacity(48);
        for (resource, amount) in &self.currency {
            if !line.is_empty() {
                line.push_str("   ");
            }
            line.push('+');
            write_integer(&mut line, i64::from(*amount));
            line.push(' ');
            push_title_case(&mut line, &resource.replace('_', " "));
        }
        Some(line)
    }

    /// Where the toast rests, before its slide.
    #[must_use]
    pub fn toast_rect(kit: &Kit, viewport: (u32, u32)) -> UiRect {
        let width = kit.px(460.0);
        let height = kit.px(96.0);
        UiRect::new(
            ((viewport.0 as f32 - width) * 0.5).round(),
            viewport.1 as f32 - kit.px(200.0) - height,
            width,
            height,
        )
    }

    /// How far below its resting place the toast is `age` seconds in, in logical pixels.
    fn slide(age: f32) -> f32 {
        let enter = (age / SLIDE_SECONDS).clamp(0.0, 1.0);
        let leave = ((TOAST_SECONDS - age) / SLIDE_SECONDS).clamp(0.0, 1.0);
        let eased = |x: f32| 1.0 - (1.0 - x).powi(3);
        (1.0 - eased(enter.min(leave))) * 48.0
    }

    /// Draws the toast, and the item tooltip beside it while `pointer` is over it.
    pub fn push_toast(
        &mut self,
        list: &mut UiDrawList,
        kit: &Kit,
        character: &CharacterView,
        pointer: (f32, f32),
        viewport: (u32, u32),
    ) {
        let rest = Self::toast_rect(kit, viewport);
        if let Some(line) = self.currency_line() {
            kit.text(
                list,
                rest.x + rest.width * 0.5,
                rest.y - kit.px(18.0),
                &line,
                type_scale::HEADING,
                color::BRONZE_BRIGHT,
                TextAlign::Center,
            );
        }
        let Some(toast) = self.toast else {
            return;
        };
        let Some(def) = item(toast.item) else {
            return;
        };
        let stack = ItemStack::with_props(toast.item, toast.props);
        let rect = UiRect::new(
            rest.x,
            rest.y + kit.px(Self::slide(toast.age)),
            rest.width,
            rest.height,
        );
        let quality = quality_color(def.quality);
        kit::panel(list, kit, rect, color::alpha(quality, 0.55));

        // The item, in a slot like the sheet's.
        let edge = kit.px(64.0);
        let icon = UiRect::new(
            rect.x + kit.px(16.0),
            rect.y + (rect.height - edge) * 0.5,
            edge,
            edge,
        );
        let corner = kit.px(radius::CONTROL);
        list.panel(
            icon,
            corner,
            blend(tokens::CONTROL, quality, 0.22),
            blend(tokens::WELL, quality, 0.08),
        );
        list.icon(
            icon.inset(edge * 0.14),
            icon_for_item(def.icon),
            [1.0, 1.0, 1.0, 1.0],
        );
        list.panel_stroke(icon, corner, kit.px(2.0), quality);

        let text_x = icon.right() + kit.px(16.0);
        let mut line = String::with_capacity(64);
        match toast.mailed {
            _ if toast.drop => {
                line.push_str(def.quality.name());
                line.push_str(" DROP - SENT TO YOUR MAIL");
            }
            None => line.push_str("YOU RECEIVED"),
            Some(MailReason::BagFull) => line.push_str("YOUR BAGS ARE FULL - SENT TO YOUR MAIL"),
            Some(MailReason::Unclaimed | MailReason::SlotRemoved | MailReason::DungeonDrop) => {
                line.push_str("SENT TO YOUR MAIL");
            }
        }
        kit.text(
            list,
            text_x,
            rect.y + kit.px(20.0),
            &line,
            type_scale::LABEL,
            color::BRONZE_BRIGHT,
            TextAlign::Left,
        );
        line.clear();
        push_title_case(&mut line, def.name);
        line.push_str(" (");
        write_integer(&mut line, toast.props.total_pct().round() as i64);
        line.push_str("%)");
        kit.text(
            list,
            text_x,
            rect.y + kit.px(44.0),
            &line,
            type_scale::NAME,
            quality,
            TextAlign::Left,
        );
        // The stat lines, as the tooltip prints them.
        line.clear();
        for (stat, value) in stack.stats().entries() {
            if !line.is_empty() {
                line.push_str("   ");
            }
            line.push('+');
            write_integer(&mut line, value.round() as i64);
            line.push(' ');
            push_title_case(&mut line, stat.display_for(character.hero));
        }
        kit.text(
            list,
            text_x,
            rect.y + kit.px(70.0),
            &line,
            type_scale::BODY,
            color::VERDANT,
            TextAlign::Left,
        );

        if rect.contains(pointer.0, pointer.1) {
            let place = if toast.mailed.is_some() {
                ItemPlace::Mailed
            } else {
                ItemPlace::Loose
            };
            let context = ItemContext::loose(character, stack, place);
            self.tooltip
                .push(list, kit, stack, character.hero, &context, icon, viewport);
        }
    }

    /// Draws the pill over a chest at screen position `at`: `[E]  Open chest`.
    pub fn push_prompt(list: &mut UiDrawList, kit: &Kit, at: (f32, f32), key: &str, opened: bool) {
        let label = if opened { "Loot" } else { "Open chest" };
        let style = type_scale::HEADING;
        let chip = kit.px(26.0);
        let pad = kit.px(10.0);
        let gap = kit.px(10.0);
        let width = pad + chip + gap + kit.width(style, label) + pad + kit.px(2.0);
        let height = chip + kit.px(10.0);
        let rect = UiRect::new(
            (at.0 - width * 0.5).round(),
            (at.1 - height * 0.5).round(),
            width,
            height,
        );
        kit::panel(list, kit, rect, color::alpha(color::BRONZE_BRIGHT, 0.5));
        let chip_rect = UiRect::new(rect.x + pad, rect.y + (height - chip) * 0.5, chip, chip);
        list.panel(
            chip_rect,
            kit.px(radius::CONTROL),
            tokens::ACCENT,
            tokens::ACCENT_DIM,
        );
        kit.text(
            list,
            chip_rect.x + chip * 0.5,
            chip_rect.y + chip * 0.5,
            key,
            type_scale::HEADING,
            tokens::ON_ACCENT,
            TextAlign::Center,
        );
        kit.text(
            list,
            chip_rect.right() + gap,
            rect.y + height * 0.5,
            label,
            style,
            color::TEXT_PRIMARY,
            TextAlign::Left,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::UiSnapshot;
    use engine_game::arena::{self, ArenaConfig};
    use engine_render_data::ui::UiCommand;
    use engine_render_data::ui::UiScale;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Label { range, .. } => Some(list.text_of(*range).to_owned()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_toast_names_the_item_lists_its_stats_and_expires() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let snapshot = UiSnapshot::build(&simulation);
        let item = engine_game::character::loot_pool()[0].id;
        let mut hud = LootHud::new();
        hud.toast = Some(Toast {
            props: ItemProps::STANDARD,
            drop: false,
            item,
            mailed: None,
            age: 1.0,
        });
        let kit = Kit::new(UiScale::default());
        let mut list = UiDrawList::new();
        hud.push_toast(&mut list, &kit, &snapshot.character, (0.0, 0.0), VIEWPORT);
        let drawn = texts(&list);
        assert!(drawn.contains(&"YOU RECEIVED".to_owned()), "{drawn:?}");
        assert!(drawn.iter().any(|text| text.starts_with('+')), "{drawn:?}");

        hud.advance(TOAST_SECONDS);
        assert!(!hud.is_showing());
    }

    #[test]
    fn hovering_the_toast_shows_the_item_tooltip_and_a_mailed_one_says_so() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let snapshot = UiSnapshot::build(&simulation);
        let item = engine_game::character::loot_pool()[0].id;
        let mut hud = LootHud::new();
        hud.toast = Some(Toast {
            props: ItemProps::STANDARD,
            drop: false,
            item,
            mailed: Some(MailReason::BagFull),
            age: 1.0,
        });
        let kit = Kit::new(UiScale::default());
        let rect = LootHud::toast_rect(&kit, VIEWPORT);
        let mut list = UiDrawList::new();
        hud.push_toast(
            &mut list,
            &kit,
            &snapshot.character,
            (rect.x + 30.0, rect.y + 40.0),
            VIEWPORT,
        );
        let drawn = texts(&list);
        assert!(
            drawn.iter().any(|text| text.contains("YOUR BAGS ARE FULL")),
            "{drawn:?}"
        );
        assert!(
            drawn.contains(&"Sent to your mail".to_owned()),
            "the tooltip is missing: {drawn:?}"
        );
    }

    #[test]
    fn the_prompt_reads_open_before_and_loot_after() {
        let kit = Kit::new(UiScale::default());
        for (opened, expected) in [(false, "Open chest"), (true, "Loot")] {
            let mut list = UiDrawList::new();
            LootHud::push_prompt(&mut list, &kit, (400.0, 300.0), "E", opened);
            let drawn = texts(&list);
            assert!(drawn.contains(&"E".to_owned()));
            assert!(drawn.contains(&expected.to_owned()), "{drawn:?}");
        }
    }

    #[test]
    fn the_toast_slides_in_and_out_and_rests_between() {
        assert!(LootHud::slide(0.0) > 40.0);
        assert!(LootHud::slide(2.0).abs() < 0.001);
        assert!(LootHud::slide(TOAST_SECONDS - 0.01) > 40.0);
    }

    #[test]
    fn the_toast_sits_inside_the_screen_above_the_bars() {
        let kit = Kit::new(UiScale::default());
        let rect = LootHud::toast_rect(&kit, (1280, 720));
        assert!(rect.x >= 0.0 && rect.right() <= 1280.0 && rect.y >= 0.0);
        assert!(rect.bottom() < 720.0 - 100.0);
    }

    #[test]
    fn a_monster_drop_toasts_its_rarity_and_quality_and_a_chest_queues_both_items() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let snapshot = UiSnapshot::build(&simulation);
        let local = simulation.player().entity;
        let items = engine_game::character::loot_pool();
        let tick = simulation.tick_count();
        simulation.inject_event(CombatEvent::MobLoot {
            player: local,
            item: items[0].id,
            props: ItemProps::rolled(104),
            tick,
        });
        simulation.inject_event(CombatEvent::LootAwarded {
            player: local,
            item: items[1].id,
            props: ItemProps::rolled(77),
            index: 0,
            tick,
        });
        simulation.inject_event(CombatEvent::LootAwarded {
            player: local,
            item: items[2].id,
            props: ItemProps::rolled(88),
            index: 1,
            tick,
        });
        let mut hud = LootHud::new();
        assert!(
            hud.observe(&simulation),
            "a chest item changes the character"
        );
        let kit = Kit::new(UiScale::default());
        let mut list = UiDrawList::new();
        hud.push_toast(&mut list, &kit, &snapshot.character, (0.0, 0.0), VIEWPORT);
        let drawn = texts(&list);
        let headline = format!("{} DROP - SENT TO YOUR MAIL", items[0].quality.name());
        assert!(drawn.contains(&headline), "{drawn:?}");
        assert!(drawn.iter().any(|t| t.ends_with("(104%)")), "{drawn:?}");
        // The first toast ends early because two more wait; then the chest items follow.
        hud.advance(QUEUED_TOAST_SECONDS + 0.1);
        let mut list = UiDrawList::new();
        hud.push_toast(&mut list, &kit, &snapshot.character, (0.0, 0.0), VIEWPORT);
        assert!(texts(&list).iter().any(|t| t.ends_with("(77%)")));
        hud.advance(QUEUED_TOAST_SECONDS + 0.1);
        let mut list = UiDrawList::new();
        hud.push_toast(&mut list, &kit, &snapshot.character, (0.0, 0.0), VIEWPORT);
        assert!(texts(&list).iter().any(|t| t.ends_with("(88%)")));
    }

    #[test]
    fn currency_drops_merge_into_one_line_that_fades() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let local = simulation.player().entity;
        let tick = simulation.tick_count();
        for (resource, amount) in [("gold", 5), ("aether_shard", 1), ("gold", 7)] {
            simulation.inject_event(CombatEvent::MobCurrency {
                player: local,
                resource,
                amount,
                tick,
            });
        }
        let mut hud = LootHud::new();
        assert!(
            !hud.observe(&simulation),
            "currency is not a character change here"
        );
        assert_eq!(
            hud.currency_line().as_deref(),
            Some("+12 Gold   +1 Aether Shard")
        );
        hud.advance(CURRENCY_SECONDS);
        assert_eq!(hud.currency_line(), None);
    }
}
