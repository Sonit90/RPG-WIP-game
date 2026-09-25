//! The monster test bed's panel: pick a monster, spawn it in front of you, watch it fight.
//!
//! ```text
//! ┌ Monster Lab ─────────────────┐   click a row or Up/Down
//! │ GRAVEYARD CRYPT              │   Enter / Spawn      one in front of you
//! │ ● Skeleton Bonecaster  Ranged│   Spawn 3            three, in a fan
//! │ ● Grave Acolyte        Ranged│   P / Passive        new spawns ignore you until hit
//! │ …                            │   K / Kill all       every monster dies: its death plays
//! ├──────────────────────────────┤   Delete / Clear     every monster goes
//! │ Grave Brute                  │   L                  hide the panel
//! │ Melee · 2,400 health · ...   │
//! │ [Spawn] [Spawn 3] [Passive]  │
//! │ [Kill all]  [Clear]          │
//! │ 2 alive · 1 dead · CAST      │
//! └──────────────────────────────┘
//! ```
//!
//! Pure interface: it owns a selection, a scroll offset and two toggles, and answers input
//! with a [`LabAction`] the application carries out on the simulation. It never touches the
//! simulation itself, which is what lets it be tested without one.

use engine_game::enemy::{EnemyKind, MonsterRole, MonsterTier};
use engine_render_data::ui::{TextAlign, UiClip, UiColor, UiDrawList, UiRect, UiScale};

use super::input::UiInput;
use super::map_style::{MapStyle, TextStyle, color, radius, space, title_case, type_scale};

/// One line of the roster: a heading, or a monster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabRow {
    /// A group title.
    Heading(&'static str),
    /// A monster that can be spawned.
    Monster(EnemyKind),
}

/// Every monster in the game, grouped the way a tester looks for them.
pub const ROSTER: [LabRow; 24] = [
    LabRow::Heading("GRAVEYARD CRYPT"),
    LabRow::Monster(EnemyKind::SkeletonBonecaster),
    LabRow::Monster(EnemyKind::GraveAcolyte),
    LabRow::Monster(EnemyKind::CryptArcher),
    LabRow::Monster(EnemyKind::SkeletonWarrior),
    LabRow::Monster(EnemyKind::GraveBrute),
    LabRow::Monster(EnemyKind::CryptGhoul),
    LabRow::Heading("EMPOWERED - THE BOSS'S ADDS"),
    LabRow::Monster(EnemyKind::EmpoweredBonecaster),
    LabRow::Monster(EnemyKind::EmpoweredAcolyte),
    LabRow::Monster(EnemyKind::EmpoweredArcher),
    LabRow::Monster(EnemyKind::EmpoweredWarrior),
    LabRow::Monster(EnemyKind::EmpoweredBrute),
    LabRow::Monster(EnemyKind::EmpoweredGhoul),
    LabRow::Heading("BOSS"),
    LabRow::Monster(EnemyKind::CryptSteward),
    LabRow::Heading("ASHEN RELIQUARY"),
    LabRow::Monster(EnemyKind::Melee),
    LabRow::Monster(EnemyKind::Caster),
    LabRow::Monster(EnemyKind::Ritualist),
    LabRow::Monster(EnemyKind::Warden),
    LabRow::Monster(EnemyKind::Weaver),
    LabRow::Monster(EnemyKind::Revenant),
    LabRow::Monster(EnemyKind::Boss),
];

/// The maximum health the test bed gives the player.
pub const TEST_PLAYER_HEALTH: f32 = 1_000_000.0;

/// What the panel asks the application to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabAction {
    /// Nothing.
    None,
    /// Spawn `count` of `kind` in front of the player; passive ones do not notice the player
    /// until hit.
    Spawn {
        /// Which monster.
        kind: EnemyKind,
        /// How many.
        count: usize,
        /// Whether they wait to be attacked.
        passive: bool,
    },
    /// Kill every monster through the ordinary damage path, so each one's death plays.
    KillAll,
    /// Take every monster out of the world.
    Clear,
}

/// The keys the panel answers, already read by the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LabKeys {
    /// Select the previous monster.
    pub previous: bool,
    /// Select the next monster.
    pub next: bool,
    /// Spawn the selected monster.
    pub spawn: bool,
    /// Toggle passive spawns.
    pub passive: bool,
    /// Kill every monster.
    pub kill: bool,
    /// Clear every monster.
    pub clear: bool,
    /// Show or hide the panel.
    pub hide: bool,
}

/// What is on the field, for the status line. Filled by the application each frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LabStatus {
    /// Monsters alive.
    pub alive: usize,
    /// Monsters dead and lying there.
    pub dead: usize,
    /// How many of them are drawn with their own animated model.
    pub animated: usize,
    /// The most recently spawned monster's name and the animation it is playing.
    pub latest: Option<(String, &'static str)>,
    /// The player's current and maximum health.
    pub player_health: (f32, f32),
}

/// The buttons, in the order they are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Spawn,
    SpawnThree,
    Passive,
    KillAll,
    Clear,
}

/// Where everything is, for one viewport and scale.
#[derive(Debug, Clone, PartialEq)]
struct Layout {
    panel: UiRect,
    title: (f32, f32),
    subtitle: (f32, f32),
    /// The scrolling roster's window.
    list: UiRect,
    /// Every roster row's rectangle before scrolling, from the list's top.
    rows: Vec<UiRect>,
    detail: UiRect,
    buttons: Vec<(Button, UiRect)>,
    status: (f32, f32),
    hint: (f32, f32),
    content_height: f32,
}

/// The monster test bed's panel.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterLab {
    /// Index into [`ROSTER`]; always a monster row.
    selected: usize,
    passive: bool,
    hidden: bool,
    scroll: f32,
    hovered_row: Option<usize>,
    hovered_button: Option<Button>,
    style: MapStyle,
}

impl Default for MonsterLab {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
}

impl MonsterLab {
    /// The panel at interface scale `scale`, the first monster selected.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            selected: first_monster(),
            passive: false,
            hidden: false,
            scroll: 0.0,
            hovered_row: None,
            hovered_button: None,
            style: MapStyle::new(scale),
        }
    }

    /// Changes the interface scale.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.style = MapStyle::new(scale);
    }

    /// The monster that Spawn would spawn.
    #[must_use]
    pub fn selected(&self) -> EnemyKind {
        match ROSTER.get(self.selected) {
            Some(LabRow::Monster(kind)) => *kind,
            _ => EnemyKind::SkeletonBonecaster,
        }
    }

    /// Selects `kind`, if it is on the roster.
    pub fn select(&mut self, kind: EnemyKind) {
        if let Some(index) = ROSTER.iter().position(|row| *row == LabRow::Monster(kind)) {
            self.selected = index;
        }
    }

    /// Whether new spawns wait to be attacked.
    #[must_use]
    pub fn is_passive(&self) -> bool {
        self.passive
    }

    /// Whether the panel is hidden.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Whether the pointer at `pointer` is over the panel, so a click there is the panel's
    /// and not the world's.
    #[must_use]
    pub fn wants_pointer(&self, pointer: (f32, f32), viewport: (u32, u32)) -> bool {
        !self.hidden && self.layout(viewport).panel.contains(pointer.0, pointer.1)
    }

    /// Answers this frame's pointer, wheel and keys.
    pub fn update(
        &mut self,
        input: &UiInput,
        wheel: f32,
        keys: LabKeys,
        viewport: (u32, u32),
    ) -> LabAction {
        if keys.hide {
            self.hidden = !self.hidden;
        }
        if keys.previous {
            self.step(-1);
        }
        if keys.next {
            self.step(1);
        }
        if keys.passive {
            self.passive = !self.passive;
        }
        let mut action = if keys.spawn {
            self.spawn(1)
        } else if keys.kill {
            LabAction::KillAll
        } else if keys.clear {
            LabAction::Clear
        } else {
            LabAction::None
        };
        if self.hidden {
            self.hovered_row = None;
            self.hovered_button = None;
            return action;
        }

        let layout = self.layout(viewport);
        let (x, y) = input.pointer;
        let max_scroll = (layout.content_height - layout.list.height).max(0.0);
        if wheel != 0.0 && layout.list.contains(x, y) {
            self.scroll = (self.scroll - wheel * self.style.px(40.0)).clamp(0.0, max_scroll);
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        if keys.previous || keys.next {
            self.reveal(&layout);
        }

        self.hovered_row = None;
        if layout.list.contains(x, y) {
            let local = y - layout.list.y + self.scroll;
            self.hovered_row = layout.rows.iter().position(|row| {
                local >= row.y - layout.list.y && local < row.bottom() - layout.list.y
            });
            if let Some(index) = self.hovered_row
                && !matches!(ROSTER.get(index), Some(LabRow::Monster(_)))
            {
                self.hovered_row = None;
            }
        }
        self.hovered_button = layout
            .buttons
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(button, _)| *button);

        if input.pointer_pressed {
            if let Some(index) = self.hovered_row {
                self.selected = index;
            } else if let Some(button) = self.hovered_button {
                action = match button {
                    Button::Spawn => self.spawn(1),
                    Button::SpawnThree => self.spawn(3),
                    Button::Passive => {
                        self.passive = !self.passive;
                        LabAction::None
                    }
                    Button::KillAll => LabAction::KillAll,
                    Button::Clear => LabAction::Clear,
                };
            }
        }
        action
    }

    fn spawn(&self, count: usize) -> LabAction {
        LabAction::Spawn {
            kind: self.selected(),
            count,
            passive: self.passive,
        }
    }

    /// Moves the selection by `delta` monsters, skipping headings, wrapping at the ends.
    fn step(&mut self, delta: i32) {
        let len = ROSTER.len() as i32;
        let mut index = self.selected as i32;
        for _ in 0..len {
            index = (index + delta).rem_euclid(len);
            if matches!(ROSTER[index as usize], LabRow::Monster(_)) {
                self.selected = index as usize;
                return;
            }
        }
    }

    /// Scrolls the roster so the selected row is in view.
    fn reveal(&mut self, layout: &Layout) {
        let Some(row) = layout.rows.get(self.selected) else {
            return;
        };
        let top = row.y - layout.list.y;
        let bottom = row.bottom() - layout.list.y;
        if top < self.scroll {
            self.scroll = top;
        } else if bottom > self.scroll + layout.list.height {
            self.scroll = bottom - layout.list.height;
        }
    }

    fn layout(&self, viewport: (u32, u32)) -> Layout {
        let s = &self.style;
        let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
        let margin = s.px(space::LG);
        let pad = s.px(space::LG);
        let width = s.px(320.0).min(vw - margin * 2.0).max(s.px(200.0));
        let x = vw - width - margin;
        let top = margin;
        let inner = UiRect::new(x + pad, top, width - pad * 2.0, 0.0);

        let title = (inner.x, top + pad);
        let subtitle = (inner.x, title.1 + s.line(type_scale::ENCOUNTER));
        let list_top = subtitle.1 + s.line(type_scale::META) + s.px(space::MD);

        let row_height = s.px(26.0);
        let heading_height = s.px(28.0);
        let mut rows = Vec::with_capacity(ROSTER.len());
        let mut cursor = list_top;
        for row in ROSTER {
            let height = match row {
                LabRow::Heading(_) => heading_height,
                LabRow::Monster(_) => row_height,
            };
            rows.push(UiRect::new(
                inner.x - s.px(space::SM),
                cursor,
                inner.width + s.px(space::SM) * 2.0,
                height,
            ));
            cursor += height;
        }
        let content_height = cursor - list_top;

        // Everything below the roster has a fixed height; the roster takes what is left.
        let detail_height =
            s.line(type_scale::NAME) + s.line(type_scale::META) * 3.0 + s.px(space::MD) * 2.0;
        let button_height = s.px(32.0);
        let below = s.px(space::MD)
            + detail_height
            + s.px(space::MD)
            + button_height * 2.0
            + s.px(space::SM)
            + s.px(space::MD)
            + s.line(type_scale::META) * 4.0
            + pad;
        let available = (vh - margin - list_top - below).max(row_height * 3.0);
        let list = UiRect::new(
            inner.x - s.px(space::SM),
            list_top,
            inner.width + s.px(space::SM) * 2.0,
            content_height.min(available),
        );

        let detail = UiRect::new(
            inner.x,
            list.bottom() + s.px(space::MD),
            inner.width,
            detail_height,
        );
        let gap = s.px(space::SM);
        let first = detail.bottom() + s.px(space::MD);
        let third = (inner.width - gap * 2.0) / 3.0;
        let half = (inner.width - gap) / 2.0;
        let buttons = vec![
            (
                Button::Spawn,
                UiRect::new(inner.x, first, third, button_height),
            ),
            (
                Button::SpawnThree,
                UiRect::new(inner.x + third + gap, first, third, button_height),
            ),
            (
                Button::Passive,
                UiRect::new(inner.x + (third + gap) * 2.0, first, third, button_height),
            ),
            (
                Button::KillAll,
                UiRect::new(inner.x, first + button_height + gap, half, button_height),
            ),
            (
                Button::Clear,
                UiRect::new(
                    inner.x + half + gap,
                    first + button_height + gap,
                    half,
                    button_height,
                ),
            ),
        ];
        let status = (inner.x, first + button_height * 2.0 + gap + s.px(space::MD));
        let hint = (inner.x, status.1 + s.line(type_scale::META) * 2.0);
        let bottom = hint.1 + s.line(type_scale::META) * 2.0 + pad;
        let panel = UiRect::new(x, top, width, bottom - top);

        Layout {
            panel,
            title,
            subtitle,
            list,
            rows,
            detail,
            buttons,
            status,
            hint,
            content_height,
        }
    }

    /// Draws the panel.
    pub fn push(&self, list: &mut UiDrawList, viewport: (u32, u32), status: &LabStatus) {
        if self.hidden {
            let s = &self.style;
            let text = "L  Monster Lab";
            let width = s.width(type_scale::META, text) + s.px(space::LG) * 2.0;
            let rect = UiRect::new(
                viewport.0 as f32 - width - s.px(space::LG),
                s.px(space::LG),
                width,
                s.px(28.0),
            );
            list.panel(
                rect,
                s.px(radius::CONTROL),
                color::SURFACE_2,
                color::SURFACE_1,
            );
            list.panel_stroke(rect, s.px(radius::CONTROL), s.px(1.0), color::BORDER_STRONG);
            self.text(
                list,
                (
                    rect.x + s.px(space::LG),
                    s.centred_top(type_scale::META, rect.y + rect.height * 0.5),
                ),
                text,
                type_scale::META,
                color::TEXT_SECONDARY,
                TextAlign::Left,
            );
            return;
        }
        let s = &self.style;
        let layout = self.layout(viewport);
        let panel = layout.panel;

        list.shadow(panel, s.px(radius::WINDOW), s.px(24.0), color::SHADOW);
        list.panel(
            panel,
            s.px(radius::WINDOW),
            color::SURFACE_0_TOP,
            color::SURFACE_0_BOTTOM,
        );
        list.panel_stroke(panel, s.px(radius::WINDOW), s.px(1.0), color::BORDER_STRONG);

        self.text(
            list,
            layout.title,
            "Monster Lab",
            type_scale::ENCOUNTER,
            color::TEXT_TITLE,
            TextAlign::Left,
        );
        let (current, max) = status.player_health;
        let subtitle = format!(
            "Test scene \u{b7} your health {} / {}",
            thousands(current),
            thousands(max)
        );
        let subtitle = s.fit(type_scale::META, &subtitle, layout.list.width);
        self.text(
            list,
            layout.subtitle,
            &subtitle,
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );

        // The roster.
        list.panel(
            layout.list,
            s.px(radius::CARD),
            color::SURFACE_1,
            color::SURFACE_1,
        );
        list.clip(UiClip::rect(layout.list));
        for (index, (row, rect)) in ROSTER.iter().zip(&layout.rows).enumerate() {
            let rect = UiRect::new(rect.x, rect.y - self.scroll, rect.width, rect.height);
            if rect.bottom() < layout.list.y || rect.y > layout.list.bottom() {
                continue;
            }
            match row {
                LabRow::Heading(title) => {
                    let top = s.centred_top(type_scale::LABEL, rect.y + rect.height * 0.6);
                    self.text(
                        list,
                        (rect.x + s.px(space::MD), top),
                        title,
                        type_scale::LABEL,
                        color::BRONZE,
                        TextAlign::Left,
                    );
                }
                LabRow::Monster(kind) => self.push_row(list, *kind, rect, index),
            }
        }
        list.clip(UiClip::NONE);
        list.panel_stroke(layout.list, s.px(radius::CARD), s.px(1.0), color::BORDER);
        let max_scroll = (layout.content_height - layout.list.height).max(0.0);
        if max_scroll > 0.0 {
            let track = layout.list.inset(s.px(space::XS));
            let thumb_height =
                (track.height * layout.list.height / layout.content_height).max(s.px(20.0));
            let thumb_y = track.y + (track.height - thumb_height) * (self.scroll / max_scroll);
            let thumb = UiRect::new(track.right() - s.px(3.0), thumb_y, s.px(3.0), thumb_height);
            list.panel(thumb, s.px(1.5), color::BRONZE_DIM, color::BRONZE_DIM);
        }

        self.push_detail(list, layout.detail);

        for (button, rect) in &layout.buttons {
            let label = match button {
                Button::Spawn => "Spawn",
                Button::SpawnThree => "Spawn 3",
                Button::Passive if self.passive => "Passive: on",
                Button::Passive => "Passive: off",
                Button::KillAll => "Kill all",
                Button::Clear => "Clear",
            };
            let primary = matches!(button, Button::Spawn);
            let active = matches!(button, Button::Passive) && self.passive;
            let hovered = self.hovered_button == Some(*button);
            let (top, bottom) = if primary {
                (color::BRONZE, color::BRONZE_DIM)
            } else if hovered || active {
                (color::SURFACE_3, color::SURFACE_2)
            } else {
                (color::SURFACE_2, color::SURFACE_1)
            };
            list.panel(*rect, s.px(radius::CONTROL), top, bottom);
            let stroke = if hovered {
                color::BRONZE_BRIGHT
            } else if active {
                color::TEAL
            } else {
                color::BORDER_STRONG
            };
            list.panel_stroke(*rect, s.px(radius::CONTROL), s.px(1.0), stroke);
            let text_color = if primary {
                color::SURFACE_0_BOTTOM
            } else {
                color::TEXT_PRIMARY
            };
            let top = s.centred_top(type_scale::UI, rect.y + rect.height * 0.5);
            self.text(
                list,
                (rect.x + rect.width * 0.5, top),
                label,
                type_scale::UI,
                text_color,
                TextAlign::Center,
            );
        }

        let field = format!(
            "On the field: {} alive \u{b7} {} dead \u{b7} {} animated",
            status.alive, status.dead, status.animated
        );
        self.text(
            list,
            layout.status,
            &s.fit(type_scale::META, &field, layout.detail.width),
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        if let Some((name, animation)) = &status.latest {
            let latest = format!("Latest: {} \u{2014} {}", title_case(name), animation);
            let at = (layout.status.0, layout.status.1 + s.line(type_scale::META));
            self.text(
                list,
                at,
                &s.fit(type_scale::META, &latest, layout.detail.width),
                type_scale::META,
                color::TEAL,
                TextAlign::Left,
            );
        }
        let hints = [
            "Up/Down choose \u{b7} Enter spawn \u{b7} P passive",
            "K kill all \u{b7} Del clear \u{b7} H interrupt \u{b7} L hide",
        ];
        for (line, hint) in hints.into_iter().enumerate() {
            let at = (
                layout.hint.0,
                layout.hint.1 + s.line(type_scale::META) * line as f32,
            );
            self.text(
                list,
                at,
                &s.fit(type_scale::META, hint, layout.detail.width),
                type_scale::META,
                color::TEXT_MUTED,
                TextAlign::Left,
            );
        }
    }

    fn push_row(&self, list: &mut UiDrawList, kind: EnemyKind, rect: UiRect, index: usize) {
        let s = &self.style;
        let selected = index == self.selected;
        let hovered = self.hovered_row == Some(index);
        let inset = UiRect::new(
            rect.x + s.px(space::XS),
            rect.y + s.px(1.0),
            rect.width - s.px(space::XS) * 2.0,
            rect.height - s.px(2.0),
        );
        if selected {
            list.panel(
                inset,
                s.px(radius::CONTROL),
                color::SURFACE_3,
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
        let archetype = kind.archetype();
        let centre = rect.y + rect.height * 0.5;
        let dot = s.px(4.5);
        list.disc((inset.x + s.px(space::MD), centre), dot, tint_of(kind));
        if archetype.look.glow > 0.0 {
            list.glow(
                UiRect::new(
                    inset.x + s.px(space::MD) - dot,
                    centre - dot,
                    dot * 2.0,
                    dot * 2.0,
                ),
                dot,
                s.px(5.0),
                color::alpha(tint_of(kind), 0.5),
            );
        }
        let role = role_name(archetype.role, archetype.tier);
        let role_width = s.width(type_scale::META, role);
        let name_x = inset.x + s.px(space::MD) + dot + s.px(space::SM);
        let name_width = inset.right() - s.px(space::MD) - role_width - s.px(space::SM) - name_x;
        let name = s.fit(type_scale::UI, &title_case(archetype.name), name_width);
        let name_color = if selected {
            color::TEXT_PRIMARY
        } else {
            color::TEXT_SECONDARY
        };
        self.text(
            list,
            (name_x, s.centred_top(type_scale::UI, centre)),
            &name,
            type_scale::UI,
            name_color,
            TextAlign::Left,
        );
        self.text(
            list,
            (
                inset.right() - s.px(space::MD),
                s.centred_top(type_scale::META, centre),
            ),
            role,
            type_scale::META,
            color::TEXT_MUTED,
            TextAlign::Right,
        );
    }

    fn push_detail(&self, list: &mut UiDrawList, rect: UiRect) {
        let s = &self.style;
        let kind = self.selected();
        let archetype = kind.archetype();
        list.panel(rect, s.px(radius::CARD), color::SURFACE_2, color::SURFACE_1);
        list.panel(
            UiRect::new(
                rect.x,
                rect.y + s.px(space::MD),
                s.px(3.0),
                rect.height - s.px(space::MD) * 2.0,
            ),
            s.px(1.5),
            tint_of(kind),
            tint_of(kind),
        );
        let x = rect.x + s.px(space::MD);
        let width = rect.width - s.px(space::MD) * 2.0;
        let mut y = rect.y + s.px(space::MD);
        let name = s.fit(type_scale::NAME, &title_case(archetype.name), width);
        self.text(
            list,
            (x, y),
            &name,
            type_scale::NAME,
            color::TEXT_PRIMARY,
            TextAlign::Left,
        );
        y += s.line(type_scale::NAME);
        let abilities = archetype.abilities().count();
        let phases = archetype.phase_count();
        let mut line = format!(
            "{} \u{b7} {} health \u{b7} {} abilit{}",
            role_name(archetype.role, archetype.tier),
            thousands(archetype.max_health),
            abilities,
            if abilities == 1 { "y" } else { "ies" }
        );
        if phases > 1 {
            line.push_str(&format!(" \u{b7} {phases} phases"));
        }
        self.text(
            list,
            (x, y),
            &s.fit(type_scale::META, &line, width),
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        y += s.line(type_scale::META);
        let summary = s.wrap(type_scale::META, archetype.summary, width);
        for text in summary.iter().take(1) {
            self.text(
                list,
                (x, y),
                &s.fit(type_scale::META, text, width),
                type_scale::META,
                color::TEXT_MUTED,
                TextAlign::Left,
            );
        }
        y += s.line(type_scale::META);
        let model = format!("Model: {}", archetype.look.model);
        self.text(
            list,
            (x, y),
            &s.fit(type_scale::META, &model, width),
            type_scale::META,
            color::TEXT_MUTED,
            TextAlign::Left,
        );
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one run of text, fully described"
    )]
    fn text(
        &self,
        list: &mut UiDrawList,
        at: (f32, f32),
        text: &str,
        style: TextStyle,
        tint: UiColor,
        align: TextAlign,
    ) {
        list.label(at, text, tint, style.font, self.style.size(style), align);
    }
}

fn first_monster() -> usize {
    ROSTER
        .iter()
        .position(|row| matches!(row, LabRow::Monster(_)))
        .unwrap_or(0)
}

fn tint_of(kind: EnemyKind) -> UiColor {
    let tint = kind.archetype().look.tint;
    [tint[0], tint[1], tint[2], 1.0]
}

fn role_name(role: MonsterRole, tier: MonsterTier) -> &'static str {
    match (tier, role) {
        (MonsterTier::Boss, _) | (_, MonsterRole::Boss) => "Boss",
        (MonsterTier::Empowered, MonsterRole::Melee) => "Empowered melee",
        (MonsterTier::Empowered, MonsterRole::Ranged) => "Empowered ranged",
        (MonsterTier::Empowered, _) => "Empowered",
        (_, MonsterRole::Melee) => "Melee",
        (_, MonsterRole::Ranged) => "Ranged",
        (_, MonsterRole::Support) => "Support",
        (_, MonsterRole::Elite) => "Elite",
    }
}

/// `1234567.8` as `"1,234,568"`.
fn thousands(value: f32) -> String {
    let whole = value.max(0.0).round() as u64;
    let digits = whole.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn click(at: (f32, f32)) -> UiInput {
        UiInput {
            pointer: at,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::default()
        }
    }

    #[test]
    fn every_enemy_kind_is_on_the_roster_once() {
        for kind in EnemyKind::ALL {
            let count = ROSTER
                .iter()
                .filter(|row| **row == LabRow::Monster(kind))
                .count();
            assert_eq!(count, 1, "{kind:?}");
        }
    }

    #[test]
    fn the_arrows_step_over_headings_and_wrap() {
        let mut lab = MonsterLab::default();
        assert_eq!(lab.selected(), EnemyKind::SkeletonBonecaster);
        let keys = LabKeys {
            previous: true,
            ..LabKeys::default()
        };
        lab.update(&UiInput::default(), 0.0, keys, VIEWPORT);
        assert_eq!(lab.selected(), EnemyKind::Boss, "wraps to the last monster");
        lab.select(EnemyKind::CryptGhoul);
        let keys = LabKeys {
            next: true,
            ..LabKeys::default()
        };
        lab.update(&UiInput::default(), 0.0, keys, VIEWPORT);
        assert_eq!(
            lab.selected(),
            EnemyKind::EmpoweredBonecaster,
            "skips the heading"
        );
    }

    #[test]
    fn clicking_a_row_selects_it_and_the_spawn_button_spawns_it() {
        let mut lab = MonsterLab::default();
        let layout = lab.layout(VIEWPORT);
        let index = ROSTER
            .iter()
            .position(|row| *row == LabRow::Monster(EnemyKind::GraveBrute))
            .unwrap_or(0);
        let row = layout.rows[index];
        let action = lab.update(
            &click((row.x + 20.0, row.y + row.height * 0.5)),
            0.0,
            LabKeys::default(),
            VIEWPORT,
        );
        assert_eq!(action, LabAction::None);
        assert_eq!(lab.selected(), EnemyKind::GraveBrute);

        let spawn = layout.buttons[0].1;
        let action = lab.update(
            &click((spawn.x + 4.0, spawn.y + 4.0)),
            0.0,
            LabKeys::default(),
            VIEWPORT,
        );
        assert_eq!(
            action,
            LabAction::Spawn {
                kind: EnemyKind::GraveBrute,
                count: 1,
                passive: false
            }
        );
        let three = layout.buttons[1].1;
        let action = lab.update(
            &click((three.x + 4.0, three.y + 4.0)),
            0.0,
            LabKeys::default(),
            VIEWPORT,
        );
        assert!(matches!(action, LabAction::Spawn { count: 3, .. }));
    }

    #[test]
    fn the_keys_spawn_kill_clear_and_toggle_passive() {
        let mut lab = MonsterLab::default();
        let passive = LabKeys {
            passive: true,
            ..LabKeys::default()
        };
        lab.update(&UiInput::default(), 0.0, passive, VIEWPORT);
        assert!(lab.is_passive());
        let spawn = LabKeys {
            spawn: true,
            ..LabKeys::default()
        };
        assert!(matches!(
            lab.update(&UiInput::default(), 0.0, spawn, VIEWPORT),
            LabAction::Spawn {
                passive: true,
                count: 1,
                ..
            }
        ));
        let kill = LabKeys {
            kill: true,
            ..LabKeys::default()
        };
        assert_eq!(
            lab.update(&UiInput::default(), 0.0, kill, VIEWPORT),
            LabAction::KillAll
        );
        let clear = LabKeys {
            clear: true,
            ..LabKeys::default()
        };
        assert_eq!(
            lab.update(&UiInput::default(), 0.0, clear, VIEWPORT),
            LabAction::Clear
        );
    }

    #[test]
    fn the_panel_fits_the_screen_and_owns_the_pointer_over_it() {
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160)] {
            for factor in [1.0, 1.5] {
                let lab = MonsterLab::new(UiScale::new(factor));
                let layout = lab.layout(viewport);
                assert!(
                    layout.panel.right() <= viewport.0 as f32,
                    "{viewport:?} x{factor}"
                );
                assert!(
                    layout.panel.bottom() <= viewport.1 as f32 + 1.0,
                    "{viewport:?} x{factor}: {}",
                    layout.panel.bottom()
                );
                assert!(lab.wants_pointer((layout.panel.x + 5.0, layout.panel.y + 5.0), viewport));
                assert!(!lab.wants_pointer((5.0, 5.0), viewport));
            }
        }
    }

    #[test]
    fn hiding_it_gives_the_pointer_back() {
        let mut lab = MonsterLab::default();
        let hide = LabKeys {
            hide: true,
            ..LabKeys::default()
        };
        lab.update(&UiInput::default(), 0.0, hide, VIEWPORT);
        assert!(lab.is_hidden());
        let layout = lab.layout(VIEWPORT);
        assert!(!lab.wants_pointer((layout.panel.x + 5.0, layout.panel.y + 5.0), VIEWPORT));
    }

    #[test]
    fn the_panel_draws_text_in_the_interface_font() {
        let lab = MonsterLab::default();
        let mut list = UiDrawList::new();
        lab.push(&mut list, VIEWPORT, &LabStatus::default());
        assert!(list.text().contains("Monster Lab"));
        assert!(list.text().contains("Skeleton Bonecaster"));
        assert!(
            list.text().contains("Lord")
                || list.text().contains("Steward")
                || list.text().contains("Ashgrave")
                || list.text().contains("Crypt")
        );
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(1_000_000.0), "1,000,000");
        assert_eq!(thousands(950.0), "950");
        assert_eq!(thousands(16_000.0), "16,000");
    }
}
