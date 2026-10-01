//! The Monster Combat Playground's two panels: the library on the left, the test controls and
//! the current encounter on the right, the fight in the middle.
//!
//! ```text
//! ┌ Monster Library ──────┐                              ┌ Playground ────────────┐
//! │ [ Search monsters… ]  │                              │ PLAYER   1,000,000 / … │
//! │ MONSTERS              │                              │ [ Test HP | Normal HP ]│
//! │ ☑ Skeleton Bonecaster │                              │ [ Reset player ]       │
//! │ ☐ Grave Acolyte       │          the fight           │ SPAWNING               │
//! │ ☑ Crypt Archer        │                              │ ☐ Passive   Level - 1 +│
//! │ …                     │                              │ CURRENT ENCOUNTER  2   │
//! ├───────────────────────┤                              │ ● Crypt Archer    100% │
//! │ Crypt Archer          │                              │ ● Grave Brute      74% │
//! │ Level 1 · Ranged ·    │                              │                        │
//! │ • Bone Arrow  Instant │                              │ [ Kill all ]           │
//! ├───────────────────────┤                              │ [ RESET ENCOUNTER ]    │
//! │ 2 SELECTED     Clear  │                              │ Enter spawn · Del reset│
//! │ [ SPAWN SELECTED ]    │                              └────────────────────────┘
//! └───────────────────────┘
//! ```
//!
//! Pure interface. It owns how the panels look and where the pointer is — scroll offsets, the
//! search text, the keyboard cursor — and answers input with a [`PlaygroundAction`] for the
//! application to carry out on [`MonsterPlayground`]. What is chosen, what is standing in the
//! world and how the player is set up are the playground's, read here and never written.
//!
//! Everything the panels print that does not change — names, roles, ability lines — is
//! title-cased and formatted once, when the panels are built; the rest is written into one
//! reused buffer. A frame draws them without allocating.

use std::fmt::Write as _;

use engine_core::input::KeyCode;
use engine_game::enemy::brain::AiState;
use engine_game::enemy::{ENEMY_ABILITY_COUNT, ENEMY_KIND_COUNT, EnemyAbility, EnemyKind};
use engine_game::enemy::{MonsterRole, MonsterTier};
use engine_game::entity::EntityId;
use engine_game::playground::{HealthMode, MonsterPlayground, tier_name};
use engine_game::sim::Simulation;
use engine_render_data::ui::{TextAlign, UiClip, UiColor, UiDrawList, UiRect, UiScale};

use super::input::UiInput;
use super::map_style::{MapStyle, TextStyle, blend, color, radius, space, title_case, type_scale};

/// Effects mode: the effect registry, the target inspector, stats, buffs and debuffs.
mod effects;

use effects::{EffectRow, Item, StatLine};
use engine_game::hero::Hero;
use engine_game::status::StatusKind;

/// What the panels ask the application to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaygroundAction {
    /// Nothing.
    None,
    /// Choose this monster if it was not chosen, and un-choose it if it was.
    Toggle(EnemyKind),
    /// Choose nothing.
    ClearSelection,
    /// Replace the encounter with everything chosen.
    SpawnSelected,
    /// Take the encounter away.
    ResetEncounter,
    /// Kill the encounter through the damage path, so every death plays.
    KillEncounter,
    /// Give the player test health or their own.
    SetHealth(HealthMode),
    /// Put the player back to a fresh start.
    ResetPlayer,
    /// Make new spawns wait to be attacked, or not.
    SetPassive(bool),
    /// The level new spawns are.
    SetLevel(u8),
    /// Select this monster as the player's target.
    Target(EntityId),
    /// Become this hero: the healer, to test a dispel.
    SetHero(Hero),
    /// Point the effect controls at this entity, and target it.
    SetEffectTarget(EntityId),
    /// Apply this effect to the effect target.
    ApplyEffect {
        /// Which effect, from the registry.
        kind: StatusKind,
        /// Seconds it starts with.
        duration: f32,
        /// Stacks it starts with.
        stacks: u8,
    },
    /// Take this effect off the effect target.
    RemoveEffect(StatusKind),
    /// Take every buff (`true`) or every debuff off the effect target.
    ClearEffects {
        /// Buffs rather than debuffs.
        buffs: bool,
    },
    /// Put every applicable effect on the effect target at once.
    AddTestEffects,
}

/// The keys the panels answer, already read by the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlaygroundKeys {
    /// Move the keyboard cursor up the library.
    pub up: bool,
    /// Move it down.
    pub down: bool,
    /// Choose or un-choose the monster under the cursor.
    pub toggle: bool,
    /// Spawn what is chosen.
    pub spawn: bool,
    /// Reset the encounter.
    pub reset: bool,
    /// Kill the encounter.
    pub kill: bool,
    /// Toggle passive spawns.
    pub passive: bool,
    /// Show or hide the panels.
    pub hide: bool,
    /// Start typing in the search field.
    pub search: bool,
}

/// One line of the library list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// A tier's heading.
    Heading(MonsterTier),
    /// A monster, by its index in the library.
    Monster(usize),
}

/// Everything clickable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Search,
    SearchClear,
    /// A library row, by its index in the visible rows.
    Row(usize),
    ClearSelection,
    Spawn,
    HealthTest,
    HealthNormal,
    ResetPlayer,
    Passive,
    LevelDown,
    LevelUp,
    /// An encounter row, by member index.
    Member(usize),
    KillAll,
    Reset,
    Show,
    TabMonsters,
    TabEffects,
    /// A hero's tab, by its index in `Hero::ALL`.
    Hero(usize),
    /// An effect library row, by its index in the visible rows.
    Effect(usize),
    TimeDown,
    TimeValue,
    TimeUp,
    StacksDown,
    StacksUp,
    /// An inspector line, by its index in the items.
    Inspector(usize),
    /// An inspector effect line's remove button.
    RemoveEffect(usize),
    TestEffects,
    ClearBuffs,
    ClearDebuffs,
}

/// Which library the left panel shows, and so what the right panel's lower half is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaygroundMode {
    /// Monsters to spawn, and the encounter.
    #[default]
    Monsters,
    /// Effects to apply, and whoever they are being tried on.
    Effects,
}

/// Which text field has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The library's search.
    Search,
    /// The time-left value.
    Time,
}

/// What is printed for one library entry, prepared once.
#[derive(Debug, Clone, PartialEq)]
struct EntryText {
    kind: EnemyKind,
    name: String,
    /// Its role alone, for the list, whose headings already say the tier.
    role: &'static str,
    /// Its role with its tier, for the details card.
    kind_name: &'static str,
    /// Each ability's printed name and its "cast · cooldown" line.
    abilities: Vec<(String, String)>,
}

/// The most ability lines the details card shows before "+N more".
const DETAIL_ABILITY_LINES: usize = 6;

/// The most a search field holds.
const SEARCH_LIMIT: usize = 32;

/// The level stepper's bounds.
const LEVEL_RANGE: (u8, u8) = (1, 60);

/// Where everything is, for one viewport and scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    left: UiRect,
    right: UiRect,
    show: UiRect,
    // Left panel.
    left_title: (f32, f32),
    left_subtitle: (f32, f32),
    tab_monsters: UiRect,
    tab_effects: UiRect,
    search: UiRect,
    search_clear: UiRect,
    list: UiRect,
    detail: Option<UiRect>,
    selection_y: f32,
    clear: UiRect,
    spawn: UiRect,
    time_down: UiRect,
    time_value: UiRect,
    time_up: UiRect,
    stacks_down: UiRect,
    stacks_up: UiRect,
    // Right panel.
    right_title: (f32, f32),
    right_subtitle: (f32, f32),
    player_label_y: f32,
    health_test: UiRect,
    health_normal: UiRect,
    reset_player: UiRect,
    heroes: [UiRect; 3],
    spawning_label_y: f32,
    passive: UiRect,
    level_down: UiRect,
    level_up: UiRect,
    encounter_label_y: f32,
    encounter: UiRect,
    kill: UiRect,
    reset: UiRect,
    test_effects: UiRect,
    clear_buffs: UiRect,
    clear_debuffs: UiRect,
    hint_y: f32,
    /// Whether there is room for the key hints.
    hints: bool,
    // Row heights, physical.
    heading_height: f32,
    row_height: f32,
    member_height: f32,
}

/// The playground's two panels.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaygroundPanels {
    style: MapStyle,
    hidden: bool,
    search: String,
    typing: bool,
    /// The library as shown: headings and the monsters the search finds.
    rows: Vec<Row>,
    /// Per library entry, in library order.
    entries: Vec<EntryText>,
    /// `EnemyKind::index` to library index.
    by_kind: [usize; ENEMY_KIND_COUNT],
    /// Every enemy ability's printed name, by `EnemyAbility::index`.
    ability_names: Vec<String>,
    /// The keyboard cursor: a library index.
    focus: Option<usize>,
    /// Whether the cursor is drawn: only once the keyboard has moved it.
    focus_visible: bool,
    /// The last monster a click chose or un-chose, for the details card.
    last_touched: Option<usize>,
    hovered: Option<Control>,
    list_scroll: f32,
    encounter_scroll: f32,
    /// Reused for every number and sentence the panels print.
    scratch: String,
    /// The details card's summary, wrapped, for one entry at one width.
    summary: (Option<(usize, u32)>, Vec<String>),
    /// Monsters or effects.
    mode: PlaygroundMode,
    /// Which field types while `typing`.
    field: Field,
    /// The effect library's search.
    effect_search: String,
    /// The effect library as shown.
    effect_rows: Vec<EffectRow>,
    /// The effect APPLY applies.
    chosen_effect: Option<StatusKind>,
    /// Seconds a manual application starts with.
    duration: f32,
    /// Stacks a manual application starts with.
    stacks: u8,
    /// The time-left value being typed.
    time_text: String,
    /// What each effect's modifiers say, by `StatusKind::index`, prepared once.
    effect_lines: Vec<Vec<String>>,
    /// Who effects can be pointed at, refreshed each frame.
    targets: Vec<engine_game::playground::EffectTarget>,
    /// The inspector's lines, refreshed each frame.
    items: Vec<Item>,
    /// The inspector's stat values, refreshed each frame into reused buffers.
    stat_lines: Vec<StatLine>,
    /// The effect target's effects, read as its nameplate reads them.
    auras: crate::aura::AuraList,
    /// Every effect on the target, uncapped, refreshed each frame.
    effect_auras: Vec<crate::aura::Aura>,
}

impl PlaygroundPanels {
    /// The panels for `playground`'s library at interface scale `scale`.
    #[must_use]
    pub fn new(scale: UiScale, playground: &MonsterPlayground) -> Self {
        let mut ability_names = vec![String::new(); ENEMY_ABILITY_COUNT];
        for ability in EnemyAbility::ALL {
            ability_names[ability.index()] = title_case(ability.name());
        }
        let mut by_kind = [0; ENEMY_KIND_COUNT];
        let entries: Vec<EntryText> = playground
            .library()
            .entries()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                by_kind[entry.kind.index()] = index;
                EntryText {
                    kind: entry.kind,
                    name: title_case(entry.name()),
                    role: short_role(entry.role()),
                    kind_name: role_name(entry.role(), entry.tier()),
                    abilities: entry
                        .abilities
                        .iter()
                        .map(|ability| {
                            (
                                ability_names[ability.index()].clone(),
                                ability_meta(*ability),
                            )
                        })
                        .collect(),
                }
            })
            .collect();
        let mut panels = Self {
            style: MapStyle::new(scale),
            hidden: false,
            search: String::with_capacity(SEARCH_LIMIT),
            typing: false,
            rows: Vec::with_capacity(entries.len() + 3),
            entries,
            by_kind,
            ability_names,
            focus: None,
            focus_visible: false,
            last_touched: None,
            hovered: None,
            list_scroll: 0.0,
            encounter_scroll: 0.0,
            scratch: String::with_capacity(64),
            summary: (None, Vec::new()),
            mode: PlaygroundMode::Monsters,
            field: Field::Search,
            effect_search: String::with_capacity(SEARCH_LIMIT),
            effect_rows: Vec::with_capacity(StatusKind::ALL.len() + 2),
            chosen_effect: None,
            duration: 30.0,
            stacks: 1,
            time_text: String::with_capacity(8),
            effect_lines: StatusKind::ALL
                .into_iter()
                .map(effects::modifier_lines)
                .collect(),
            targets: Vec::with_capacity(16),
            items: Vec::with_capacity(64),
            stat_lines: Vec::with_capacity(16),
            auras: crate::aura::AuraList::default(),
            effect_auras: Vec::with_capacity(StatusKind::ALL.len() + 16),
        };
        panels.rebuild_rows(playground);
        panels.rebuild_effect_rows();
        panels
    }

    /// Changes the interface scale.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.style = MapStyle::new(scale);
        self.summary.0 = None;
    }

    /// Whether the panels are hidden.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Whether the search field has the keyboard: the application must not read this frame's
    /// keys as play while it does.
    #[must_use]
    pub fn is_typing(&self) -> bool {
        self.typing && !self.hidden
    }

    /// What the search field of the current library holds.
    #[must_use]
    pub fn search(&self) -> &str {
        match self.mode {
            PlaygroundMode::Monsters => &self.search,
            PlaygroundMode::Effects => &self.effect_search,
        }
    }

    /// Which library is showing.
    #[must_use]
    pub fn mode(&self) -> PlaygroundMode {
        self.mode
    }

    /// Shows the monster library or the effect library.
    pub fn set_mode(&mut self, mode: PlaygroundMode) {
        if self.mode != mode {
            self.mode = mode;
            self.typing = false;
            self.list_scroll = 0.0;
            self.encounter_scroll = 0.0;
        }
    }

    /// The effect APPLY would apply.
    #[must_use]
    pub fn chosen_effect(&self) -> Option<StatusKind> {
        self.chosen_effect
    }

    /// Chooses the effect APPLY applies.
    pub fn choose_effect(&mut self, kind: StatusKind) {
        self.chosen_effect = Some(kind);
    }

    /// Whether a click at `pointer` is the panels' rather than the world's.
    #[must_use]
    pub fn wants_pointer(&self, pointer: (f32, f32), viewport: (u32, u32)) -> bool {
        let layout = self.layout(viewport);
        let (x, y) = pointer;
        if self.hidden {
            return layout.show.contains(x, y);
        }
        layout.left.contains(x, y) || layout.right.contains(x, y)
    }

    /// How many physical pixels the panels take from the left and the right edge, for the
    /// HUD to be laid out beside them. Nothing while they are hidden.
    #[must_use]
    pub fn reserved(&self, viewport: (u32, u32)) -> (f32, f32) {
        if self.hidden {
            return (0.0, 0.0);
        }
        let layout = self.layout(viewport);
        (layout.left.right(), viewport.0 as f32 - layout.right.x)
    }

    /// Answers this frame's pointer, wheel and keys.
    ///
    /// `input` carries the frame's captured key while the search field is typing: see
    /// [`UiInput::with_captured_key`].
    pub fn update(
        &mut self,
        input: &UiInput,
        wheel: f32,
        keys: PlaygroundKeys,
        viewport: (u32, u32),
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) -> PlaygroundAction {
        let mut keys = keys;
        if self.typing {
            if !self.update_typing(input, viewport, playground) {
                self.hovered = None;
                return PlaygroundAction::None;
            }
            // Typing ended on a click elsewhere: the click is answered below, and this
            // frame's keys were letters, not commands.
            keys = PlaygroundKeys::default();
        }
        if keys.hide {
            self.hidden = !self.hidden;
            self.hovered = None;
        }
        let mut action = PlaygroundAction::None;
        if self.mode == PlaygroundMode::Effects {
            self.rebuild_inspector(playground, simulation);
        }
        if !self.hidden && self.mode == PlaygroundMode::Effects {
            if keys.search {
                self.typing = true;
                self.field = Field::Search;
                return action;
            }
            if keys.up {
                self.step_effect(-1);
            }
            if keys.down {
                self.step_effect(1);
            }
            action = if keys.spawn || keys.toggle {
                self.click_effects(Control::Spawn, playground, simulation)
            } else if keys.reset {
                PlaygroundAction::ResetEncounter
            } else {
                PlaygroundAction::None
            };
        } else if !self.hidden {
            if keys.search {
                self.typing = true;
                self.field = Field::Search;
                return action;
            }
            if keys.up {
                self.step_focus(-1);
            }
            if keys.down {
                self.step_focus(1);
            }
            if keys.up || keys.down {
                self.reveal_focus(viewport);
            }
            action = if keys.toggle {
                self.focus.map_or(PlaygroundAction::None, |index| {
                    PlaygroundAction::Toggle(self.entries[index].kind)
                })
            } else if keys.spawn && !playground.selection().is_empty() {
                PlaygroundAction::SpawnSelected
            } else if keys.reset {
                PlaygroundAction::ResetEncounter
            } else if keys.kill {
                PlaygroundAction::KillEncounter
            } else if keys.passive {
                PlaygroundAction::SetPassive(!playground.is_passive())
            } else {
                PlaygroundAction::None
            };
        }

        let layout = self.layout(viewport);
        let (x, y) = input.pointer;
        let members = playground.encounter().len();
        if !self.hidden && wheel != 0.0 {
            let step = wheel * self.style.px(40.0);
            if layout.list.contains(x, y) {
                self.list_scroll -= step;
            } else if layout.encounter.contains(x, y) {
                self.encounter_scroll -= step;
            }
        }
        self.clamp_scroll(&layout, members);
        self.hovered = self.control_at(&layout, (x, y), members);

        if input.pointer_pressed
            && let Some(control) = self.hovered
        {
            action = self.click(control, playground, simulation);
            // A click can open Effects mode or change what it shows: the inspector is read
            // again so the next hit test and draw see it.
            if self.mode == PlaygroundMode::Effects {
                self.rebuild_inspector(playground, simulation);
            }
        }
        action
    }

    /// Answers a frame while the search field types. Returns whether this frame's click
    /// was somewhere else, ending the typing, and is still to be answered.
    fn update_typing(
        &mut self,
        input: &UiInput,
        viewport: (u32, u32),
        playground: &MonsterPlayground,
    ) -> bool {
        if self.field == Field::Time {
            return self.update_time_typing(input, viewport);
        }
        let effects = self.mode == PlaygroundMode::Effects;
        let mut search = std::mem::take(if effects {
            &mut self.effect_search
        } else {
            &mut self.search
        });
        let before = search.len();
        if input.cancel {
            self.typing = false;
        } else if let Some(chord) = input.captured_key {
            match chord.key {
                KeyCode::Enter | KeyCode::Tab => self.typing = false,
                KeyCode::Backspace => {
                    search.pop();
                }
                KeyCode::Delete => search.clear(),
                key => {
                    if let Some(character) = typed(key)
                        && search.len() < SEARCH_LIMIT
                    {
                        search.push(character);
                    }
                }
            }
        }
        let mut elsewhere = false;
        if input.pointer_pressed {
            let layout = self.layout(viewport);
            let (x, y) = input.pointer;
            if !search.is_empty() && layout.search_clear.contains(x, y) {
                search.clear();
            } else if !layout.search.contains(x, y) {
                self.typing = false;
                elsewhere = true;
            }
        }
        let changed = search.len() != before;
        if effects {
            self.effect_search = search;
            if changed {
                self.rebuild_effect_rows();
            }
        } else {
            self.search = search;
            if changed {
                self.rebuild_rows(playground);
            }
        }
        elsewhere
    }

    /// Typing a time-left value: digits, Backspace, Enter to keep it, Escape to drop it.
    fn update_time_typing(&mut self, input: &UiInput, viewport: (u32, u32)) -> bool {
        let commit = |panels: &mut Self| {
            if let Ok(seconds) = panels.time_text.parse::<f32>()
                && seconds >= 1.0
            {
                panels.duration = seconds.min(effects::TIME_MAX);
            }
            panels.typing = false;
            panels.field = Field::Search;
        };
        if input.cancel {
            self.typing = false;
            self.field = Field::Search;
            return false;
        }
        if let Some(chord) = input.captured_key {
            match chord.key {
                KeyCode::Enter | KeyCode::Tab => {
                    commit(self);
                    return false;
                }
                KeyCode::Backspace => {
                    self.time_text.pop();
                }
                key => {
                    if let Some(character) = typed(key)
                        && character.is_ascii_digit()
                        && self.time_text.len() < 4
                    {
                        self.time_text.push(character);
                    }
                }
            }
        }
        if input.pointer_pressed {
            let layout = self.layout(viewport);
            if !layout.time_value.contains(input.pointer.0, input.pointer.1) {
                commit(self);
                return true;
            }
        }
        false
    }

    fn click(
        &mut self,
        control: Control,
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) -> PlaygroundAction {
        let effects = self.mode == PlaygroundMode::Effects;
        match control {
            Control::Show => {
                self.hidden = false;
                PlaygroundAction::None
            }
            Control::TabMonsters => {
                self.set_mode(PlaygroundMode::Monsters);
                PlaygroundAction::None
            }
            Control::TabEffects => {
                self.set_mode(PlaygroundMode::Effects);
                PlaygroundAction::None
            }
            Control::Hero(index) => {
                Hero::from_index(index).map_or(PlaygroundAction::None, PlaygroundAction::SetHero)
            }
            Control::Search => {
                self.typing = true;
                self.field = Field::Search;
                PlaygroundAction::None
            }
            Control::SearchClear if effects => {
                self.effect_search.clear();
                self.rebuild_effect_rows();
                PlaygroundAction::None
            }
            Control::SearchClear => {
                self.search.clear();
                self.rebuild_rows(playground);
                PlaygroundAction::None
            }
            Control::Effect(_)
            | Control::TimeDown
            | Control::TimeValue
            | Control::TimeUp
            | Control::StacksDown
            | Control::StacksUp
            | Control::Inspector(_)
            | Control::RemoveEffect(_)
            | Control::TestEffects
            | Control::ClearBuffs
            | Control::ClearDebuffs => self.click_effects(control, playground, simulation),
            Control::Spawn if effects => self.click_effects(control, playground, simulation),
            Control::Reset if effects => PlaygroundAction::ResetEncounter,
            Control::Row(row) => match self.rows.get(row) {
                Some(Row::Monster(index)) => {
                    self.last_touched = Some(*index);
                    self.focus = Some(*index);
                    self.focus_visible = false;
                    PlaygroundAction::Toggle(self.entries[*index].kind)
                }
                _ => PlaygroundAction::None,
            },
            Control::ClearSelection if !playground.selection().is_empty() => {
                PlaygroundAction::ClearSelection
            }
            Control::Spawn if !playground.selection().is_empty() => PlaygroundAction::SpawnSelected,
            Control::HealthTest => PlaygroundAction::SetHealth(HealthMode::Test),
            Control::HealthNormal => PlaygroundAction::SetHealth(HealthMode::Normal),
            Control::ResetPlayer => PlaygroundAction::ResetPlayer,
            Control::Passive => PlaygroundAction::SetPassive(!playground.is_passive()),
            Control::LevelDown if playground.monster_level() > LEVEL_RANGE.0 => {
                PlaygroundAction::SetLevel(playground.monster_level() - 1)
            }
            Control::LevelUp if playground.monster_level() < LEVEL_RANGE.1 => {
                PlaygroundAction::SetLevel(playground.monster_level() + 1)
            }
            Control::Member(index) => match playground.encounter().members().get(index) {
                Some(member) if simulation.world().is_alive(member.entity) => {
                    PlaygroundAction::Target(member.entity)
                }
                _ => PlaygroundAction::None,
            },
            Control::KillAll if !playground.encounter().is_empty() => {
                PlaygroundAction::KillEncounter
            }
            Control::Reset if !playground.encounter().is_empty() => {
                PlaygroundAction::ResetEncounter
            }
            _ => PlaygroundAction::None,
        }
    }

    /// Rebuilds the visible rows from the search. Runs when the search changes, never per
    /// frame.
    fn rebuild_rows(&mut self, playground: &MonsterPlayground) {
        self.rows.clear();
        let mut tier = None;
        for (index, entry) in playground.library().entries().iter().enumerate() {
            if !entry.matches(&self.search) {
                continue;
            }
            if tier != Some(entry.tier()) {
                tier = Some(entry.tier());
                self.rows.push(Row::Heading(entry.tier()));
            }
            self.rows.push(Row::Monster(index));
        }
        self.list_scroll = 0.0;
        if self
            .focus
            .is_some_and(|focus| !self.rows.contains(&Row::Monster(focus)))
        {
            self.focus = None;
        }
    }

    /// How many library entries the search shows.
    #[must_use]
    pub fn visible_monsters(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, Row::Monster(_)))
            .count()
    }

    fn step_focus(&mut self, delta: i32) {
        let positions = self.rows.iter().filter_map(|row| match row {
            Row::Monster(index) => Some(*index),
            Row::Heading(_) => None,
        });
        let count = positions.clone().count();
        if count == 0 {
            self.focus = None;
            return;
        }
        let current = self
            .focus
            .and_then(|focus| positions.clone().position(|index| index == focus));
        let next = match current {
            None if delta > 0 => 0,
            None => count - 1,
            Some(at) => (at as i32 + delta).rem_euclid(count as i32) as usize,
        };
        self.focus = positions.clone().nth(next);
        self.focus_visible = true;
    }

    /// Scrolls the library so the keyboard cursor is in view.
    fn reveal_focus(&mut self, viewport: (u32, u32)) {
        let Some(focus) = self.focus else {
            return;
        };
        let layout = self.layout(viewport);
        let mut top = 0.0;
        for row in &self.rows {
            let height = row_height(&layout, *row);
            if *row == Row::Monster(focus) {
                if top < self.list_scroll {
                    self.list_scroll = top;
                } else if top + height > self.list_scroll + layout.list.height {
                    self.list_scroll = top + height - layout.list.height;
                }
                return;
            }
            top += height;
        }
    }

    fn content_height(&self, layout: &Layout) -> f32 {
        self.rows.iter().map(|row| row_height(layout, *row)).sum()
    }

    fn clamp_scroll(&mut self, layout: &Layout, members: usize) {
        let (list_content, lower_content) = match self.mode {
            PlaygroundMode::Monsters => (
                self.content_height(layout),
                members as f32 * layout.member_height,
            ),
            PlaygroundMode::Effects => (
                self.effect_rows
                    .iter()
                    .map(|row| self.effect_row_height(layout, *row))
                    .sum(),
                self.inspector_height(layout),
            ),
        };
        let list_max = (list_content - layout.list.height).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, list_max);
        let lower_max = (lower_content - layout.encounter.height).max(0.0);
        self.encounter_scroll = self.encounter_scroll.clamp(0.0, lower_max);
    }

    fn control_at(&self, layout: &Layout, point: (f32, f32), members: usize) -> Option<Control> {
        let (x, y) = point;
        if self.hidden {
            return layout.show.contains(x, y).then_some(Control::Show);
        }
        let shared = [
            (layout.tab_monsters, Control::TabMonsters),
            (layout.tab_effects, Control::TabEffects),
            (layout.search_clear, Control::SearchClear),
            (layout.search, Control::Search),
            (layout.spawn, Control::Spawn),
            (layout.health_test, Control::HealthTest),
            (layout.health_normal, Control::HealthNormal),
            (layout.reset_player, Control::ResetPlayer),
            (layout.heroes[0], Control::Hero(0)),
            (layout.heroes[1], Control::Hero(1)),
            (layout.heroes[2], Control::Hero(2)),
            (layout.passive, Control::Passive),
            (layout.level_down, Control::LevelDown),
            (layout.level_up, Control::LevelUp),
            (layout.reset, Control::Reset),
        ];
        let monsters = [
            (layout.clear, Control::ClearSelection),
            (layout.kill, Control::KillAll),
        ];
        let effects = [
            (layout.time_down, Control::TimeDown),
            (layout.time_value, Control::TimeValue),
            (layout.time_up, Control::TimeUp),
            (layout.stacks_down, Control::StacksDown),
            (layout.stacks_up, Control::StacksUp),
            (layout.test_effects, Control::TestEffects),
            (layout.clear_buffs, Control::ClearBuffs),
            (layout.clear_debuffs, Control::ClearDebuffs),
        ];
        let by_mode: &[(UiRect, Control)] = match self.mode {
            PlaygroundMode::Monsters => &monsters,
            PlaygroundMode::Effects => &effects,
        };
        for (rect, control) in shared.iter().chain(by_mode) {
            if *control == Control::SearchClear && self.search().is_empty() {
                continue;
            }
            if rect.contains(x, y) {
                return Some(*control);
            }
        }
        if self.mode == PlaygroundMode::Effects {
            return self.effect_control_at(layout, point);
        }
        if layout.list.contains(x, y) {
            let local = y - layout.list.y + self.list_scroll;
            let mut top = 0.0;
            for (index, row) in self.rows.iter().enumerate() {
                let height = row_height(layout, *row);
                if local >= top && local < top + height {
                    return matches!(row, Row::Monster(_)).then_some(Control::Row(index));
                }
                top += height;
            }
        }
        if layout.encounter.contains(x, y) {
            let local = y - layout.encounter.y + self.encounter_scroll;
            let index = (local / layout.member_height).floor();
            if index >= 0.0 && (index as usize) < members {
                return Some(Control::Member(index as usize));
            }
        }
        None
    }

    fn layout(&self, viewport: (u32, u32)) -> Layout {
        let s = &self.style;
        let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
        let margin = s.px(space::LG);
        let pad = s.px(space::LG);
        let width = s.px(296.0).min(vw * 0.25).max(s.px(200.0).min(vw * 0.4));
        let top = margin;
        let bottom = (vh - margin).max(top + s.px(200.0));
        let left = UiRect::new(margin, top, width, bottom - top);
        let right = UiRect::new(vw - margin - width, top, width, bottom - top);

        let show_text = "L  Show playground";
        let show_width = s.width(type_scale::UI, show_text) + s.px(space::LG) * 2.0;
        let show = UiRect::new(vw - show_width - margin, margin, show_width, s.px(30.0));

        // The left panel, top down to the list, and bottom up to it.
        let inner_x = left.x + pad;
        let inner_w = left.width - pad * 2.0;
        let left_title = (inner_x, top + pad);
        let left_subtitle = (inner_x, left_title.1 + s.line(type_scale::ENCOUNTER));
        let tabs_top = left_subtitle.1 + s.line(type_scale::META) + s.px(space::MD);
        let tab_height = s.px(30.0);
        let tab_monsters = UiRect::new(inner_x, tabs_top, inner_w * 0.5, tab_height);
        let tab_effects = UiRect::new(inner_x + inner_w * 0.5, tabs_top, inner_w * 0.5, tab_height);
        let search_top = tab_monsters.bottom() + s.px(space::SM);
        let search = UiRect::new(inner_x, search_top, inner_w, s.px(32.0));
        let clear_size = s.px(22.0);
        let search_clear = UiRect::new(
            search.right() - clear_size - s.px(space::XS),
            search.y + (search.height - clear_size) * 0.5,
            clear_size,
            clear_size,
        );
        let list_top = search.bottom() + s.px(space::SM);

        let spawn = UiRect::new(inner_x, bottom - pad - s.px(42.0), inner_w, s.px(42.0));
        let selection_height = s.px(28.0);
        let selection_y = spawn.y - s.px(space::SM) - selection_height;
        let clear_width = s.width(type_scale::UI, "Clear") + s.px(space::MD) * 2.0;
        let clear = UiRect::new(
            inner_x + inner_w - clear_width,
            selection_y,
            clear_width,
            selection_height,
        );
        // Effects mode's footer row: TIME - 30s +   STACKS - 1 +
        // Sized for the normal scale and squeezed, all together, where the panel is narrower
        // than that: a 720p screen at a large interface size.
        let time_label = s.width(type_scale::LABEL, "TIME") + s.px(space::SM);
        let natural = time_label + s.px(24.0 * 4.0 + 46.0 + 24.0 + space::SM);
        let fit = (inner_w / natural).min(1.0);
        let step = s.px(24.0) * fit;
        let time_down = UiRect::new(
            inner_x + time_label * fit,
            selection_y,
            step,
            selection_height,
        );
        let time_value = UiRect::new(
            time_down.right(),
            selection_y,
            s.px(46.0) * fit,
            selection_height,
        );
        let time_up = UiRect::new(time_value.right(), selection_y, step, selection_height);
        let stacks_up = UiRect::new(
            inner_x + inner_w - step,
            selection_y,
            step,
            selection_height,
        );
        let stacks_down = UiRect::new(
            stacks_up.x - s.px(24.0) * fit - step,
            selection_y,
            step,
            selection_height,
        );
        let detail_height = s.px(space::MD) * 2.0
            + s.line(type_scale::NAME)
            + s.line(type_scale::META) * 3.0
            + s.px(space::SM)
            + s.line(type_scale::LABEL)
            + s.line(type_scale::META) * DETAIL_ABILITY_LINES as f32;
        let row_height = s.px(30.0);
        let detail_top = selection_y - s.px(space::MD) - detail_height;
        // The card goes when it would leave the list fewer than four rows.
        let (detail, list_bottom) = if detail_top - s.px(space::MD) - list_top >= row_height * 4.0 {
            (
                Some(UiRect::new(inner_x, detail_top, inner_w, detail_height)),
                detail_top - s.px(space::MD),
            )
        } else {
            (None, selection_y - s.px(space::SM))
        };
        let list = UiRect::new(
            left.x + s.px(space::SM),
            list_top,
            left.width - s.px(space::SM) * 2.0,
            (list_bottom - list_top).max(row_height),
        );

        // The right panel.
        let rx = right.x + pad;
        let rw = right.width - pad * 2.0;
        let right_title = (rx, top + pad);
        let right_subtitle = (rx, right_title.1 + s.line(type_scale::ENCOUNTER));
        let player_label_y = right_subtitle.1 + s.line(type_scale::META) + s.px(space::LG);
        let segment_top = player_label_y + s.line(type_scale::LABEL) + s.px(space::SM);
        let segment_height = s.px(32.0);
        let health_test = UiRect::new(rx, segment_top, rw * 0.5, segment_height);
        let health_normal = UiRect::new(rx + rw * 0.5, segment_top, rw * 0.5, segment_height);
        let reset_player = UiRect::new(rx, health_test.bottom() + s.px(space::SM), rw, s.px(32.0));
        let hero_top = reset_player.bottom() + s.px(space::SM);
        let third = rw / 3.0;
        let heroes = [0, 1, 2]
            .map(|index| UiRect::new(rx + third * index as f32, hero_top, third, s.px(28.0)));
        let spawning_label_y = heroes[0].bottom() + s.px(space::LG);
        let control_top = spawning_label_y + s.line(type_scale::LABEL) + s.px(space::SM);
        let control_height = s.px(28.0);
        let stepper = s.px(28.0);
        let passive = UiRect::new(rx, control_top, rw * 0.5, control_height);
        let level_up = UiRect::new(rx + rw - stepper, control_top, stepper, control_height);
        let level_down = UiRect::new(
            level_up.x - s.px(34.0) - stepper,
            control_top,
            stepper,
            control_height,
        );
        let encounter_label_y = passive.bottom() + s.px(space::LG);
        let encounter_top = encounter_label_y + s.line(type_scale::LABEL) + s.px(space::SM);
        let member_height = s.px(48.0);
        let hint_y = bottom - pad - s.line(type_scale::META) * 2.0;
        let mut reset = UiRect::new(rx, hint_y - s.px(space::MD) - s.px(42.0), rw, s.px(42.0));
        let mut kill = UiRect::new(rx, reset.y - s.px(space::SM) - s.px(30.0), rw, s.px(30.0));
        // On a short screen the hints go and the two buttons share a row, so the encounter
        // always has room for two monsters.
        let hints = kill.y - s.px(space::MD) - encounter_top >= member_height * 2.0;
        if !hints {
            let gap = s.px(space::SM);
            let height = s.px(40.0);
            let y = bottom - pad - height;
            kill = UiRect::new(rx, y, (rw - gap) * 0.38, height);
            reset = UiRect::new(kill.right() + gap, y, rw - kill.width - gap, height);
        }
        // Effects mode: the clears share the kill row; the test set sits above it.
        let gap = s.px(space::SM);
        let clear_buffs = UiRect::new(kill.x, kill.y, (kill.width - gap) * 0.5, kill.height);
        let clear_debuffs = UiRect::new(
            clear_buffs.right() + gap,
            kill.y,
            kill.width - clear_buffs.width - gap,
            kill.height,
        );
        let test_effects = UiRect::new(rx, kill.y - gap - s.px(30.0), rw, s.px(30.0));
        let lower_bottom = match self.mode {
            PlaygroundMode::Monsters => kill.y,
            PlaygroundMode::Effects => test_effects.y,
        };
        let encounter = UiRect::new(
            right.x + s.px(space::SM),
            encounter_top,
            right.width - s.px(space::SM) * 2.0,
            (lower_bottom - s.px(space::MD) - encounter_top).max(member_height),
        );

        Layout {
            left,
            right,
            show,
            left_title,
            left_subtitle,
            tab_monsters,
            tab_effects,
            search,
            search_clear,
            list,
            detail,
            selection_y,
            clear,
            spawn,
            time_down,
            time_value,
            time_up,
            stacks_down,
            stacks_up,
            right_title,
            right_subtitle,
            player_label_y,
            health_test,
            health_normal,
            reset_player,
            heroes,
            spawning_label_y,
            passive,
            level_down,
            level_up,
            encounter_label_y,
            encounter,
            kill,
            reset,
            test_effects,
            clear_buffs,
            clear_debuffs,
            hint_y,
            hints,
            heading_height: s.px(28.0),
            row_height,
            member_height,
        }
    }

    /// The library entry the details card describes: the row under the pointer, else the
    /// keyboard cursor, else the last one clicked, else the first chosen.
    fn detail_entry(&self, playground: &MonsterPlayground) -> Option<usize> {
        if let Some(Control::Row(row)) = self.hovered
            && let Some(Row::Monster(index)) = self.rows.get(row)
        {
            return Some(*index);
        }
        self.focus.or(self.last_touched).or_else(|| {
            playground
                .selection()
                .kinds()
                .first()
                .map(|kind| self.by_kind[kind.index()])
        })
    }

    /// Draws both panels, or the button that brings them back.
    pub fn push(
        &mut self,
        list: &mut UiDrawList,
        viewport: (u32, u32),
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) {
        let layout = self.layout(viewport);
        if self.hidden {
            let hovered = self.hovered == Some(Control::Show);
            self.button(
                list,
                layout.show,
                "L  Show playground",
                Style::Secondary,
                hovered,
                true,
            );
            return;
        }
        if self.mode == PlaygroundMode::Effects {
            self.rebuild_inspector(playground, simulation);
        }
        self.push_library(list, &layout, playground, simulation);
        self.push_controls(list, &layout, playground, simulation);
    }

    fn panel(&self, list: &mut UiDrawList, rect: UiRect) {
        let s = &self.style;
        list.shadow(rect, s.px(radius::WINDOW), s.px(24.0), color::SHADOW);
        list.panel(
            rect,
            s.px(radius::WINDOW),
            color::SURFACE_0_TOP,
            color::SURFACE_0_BOTTOM,
        );
        list.panel_stroke(rect, s.px(radius::WINDOW), s.px(1.0), color::BORDER_STRONG);
    }

    fn push_library(
        &mut self,
        list: &mut UiDrawList,
        layout: &Layout,
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) {
        let s = self.style;
        let effects = self.mode == PlaygroundMode::Effects;
        self.panel(list, layout.left);
        self.text(
            list,
            layout.left_title,
            if effects {
                "Effect Library"
            } else {
                "Monster Library"
            },
            type_scale::ENCOUNTER,
            color::TEXT_TITLE,
            TextAlign::Left,
        );
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        if effects {
            let shown = self.visible_effects();
            let total = StatusKind::ALL.len();
            if shown == total {
                let _ = write!(scratch, "{total} effects \u{b7} the global registry");
            } else {
                let _ = write!(scratch, "{shown} of {total} effects");
            }
        } else {
            let shown = self.visible_monsters();
            if shown == self.entries.len() {
                let _ = write!(scratch, "{} monsters", self.entries.len());
            } else {
                let _ = write!(scratch, "{shown} of {} monsters", self.entries.len());
            }
            let _ = write!(scratch, " \u{b7} level {}", playground.monster_level());
        }
        self.text(
            list,
            layout.left_subtitle,
            &scratch,
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );

        // The two libraries.
        self.segment(
            list,
            layout.tab_monsters,
            "Monsters",
            !effects,
            self.hovered == Some(Control::TabMonsters),
            true,
        );
        self.segment(
            list,
            layout.tab_effects,
            "Effects",
            effects,
            self.hovered == Some(Control::TabEffects),
            false,
        );

        // The search field.
        let field = layout.search;
        let focused = self.typing && self.field == Field::Search;
        let hovered = matches!(self.hovered, Some(Control::Search));
        list.panel(
            field,
            s.px(radius::CONTROL),
            color::SURFACE_1,
            color::SURFACE_1,
        );
        list.panel_stroke(
            field,
            s.px(radius::CONTROL),
            s.px(1.0),
            if focused {
                color::BRONZE
            } else if hovered {
                color::BORDER_STRONG
            } else {
                color::BORDER
            },
        );
        let text_x = field.x + s.px(space::MD);
        let text_top = s.centred_top(type_scale::UI, field.y + field.height * 0.5);
        if self.search().is_empty() && !focused {
            self.text(
                list,
                (text_x, text_top),
                if effects {
                    "Search effects  (Ctrl+F)"
                } else {
                    "Search monsters  (Ctrl+F)"
                },
                type_scale::UI,
                color::TEXT_MUTED,
                TextAlign::Left,
            );
        } else {
            scratch.clear();
            scratch.push_str(self.search());
            if focused {
                scratch.push('|');
            }
            self.text(
                list,
                (text_x, text_top),
                &scratch,
                type_scale::UI,
                color::TEXT_PRIMARY,
                TextAlign::Left,
            );
        }
        if !self.search().is_empty() {
            let clear = layout.search_clear;
            if self.hovered == Some(Control::SearchClear) {
                list.panel(
                    clear,
                    s.px(radius::CHIP),
                    color::SURFACE_3,
                    color::SURFACE_3,
                );
            }
            self.text(
                list,
                (
                    clear.x + clear.width * 0.5,
                    s.centred_top(type_scale::UI, clear.y + clear.height * 0.5),
                ),
                "\u{d7}",
                type_scale::UI,
                color::TEXT_SECONDARY,
                TextAlign::Center,
            );
        }

        if effects {
            self.push_effect_library(list, layout, playground, simulation, &mut scratch);
            self.scratch = scratch;
            return;
        }

        // The list.
        list.panel(
            layout.list,
            s.px(radius::CARD),
            color::SURFACE_1,
            color::SURFACE_1,
        );
        list.clip(UiClip::rect(layout.list));
        let mut top = layout.list.y - self.list_scroll;
        for (index, row) in self.rows.iter().enumerate() {
            let height = row_height(layout, *row);
            let rect = UiRect::new(layout.list.x, top, layout.list.width, height);
            top += height;
            if rect.bottom() < layout.list.y || rect.y > layout.list.bottom() {
                continue;
            }
            match row {
                Row::Heading(tier) => {
                    self.text(
                        list,
                        (
                            rect.x + s.px(space::MD),
                            s.centred_top(type_scale::LABEL, rect.y + rect.height * 0.62),
                        ),
                        tier_name(*tier),
                        type_scale::LABEL,
                        color::BRONZE,
                        TextAlign::Left,
                    );
                }
                Row::Monster(entry) => self.push_row(list, rect, index, *entry, playground),
            }
        }
        if self.rows.is_empty() {
            self.text(
                list,
                (
                    layout.list.x + layout.list.width * 0.5,
                    layout.list.y + s.px(space::LG),
                ),
                "No monster matches",
                type_scale::UI,
                color::TEXT_MUTED,
                TextAlign::Center,
            );
        }
        list.clip(UiClip::NONE);
        list.panel_stroke(layout.list, s.px(radius::CARD), s.px(1.0), color::BORDER);
        let content = self.content_height(layout);
        self.scrollbar(list, layout.list, self.list_scroll, content);

        if let Some(rect) = layout.detail {
            match self.detail_entry(playground) {
                Some(entry) => self.push_detail(list, rect, entry, playground, &mut scratch),
                None => self.push_hint_card(
                    list,
                    rect,
                    "Point at a monster",
                    "to see its level, health and abilities",
                ),
            }
        }

        // The selection and the primary action.
        let count = playground.selection().len();
        scratch.clear();
        match count {
            0 => scratch.push_str("NOTHING SELECTED"),
            1 => scratch.push_str("1 MONSTER SELECTED"),
            n => {
                let _ = write!(scratch, "{n} MONSTERS SELECTED");
            }
        }
        self.text(
            list,
            (
                layout.left.x + s.px(space::LG),
                s.centred_top(
                    type_scale::LABEL,
                    layout.clear.y + layout.clear.height * 0.5,
                ),
            ),
            &scratch,
            type_scale::LABEL,
            if count > 0 {
                color::BRONZE_BRIGHT
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Left,
        );
        if count > 0 {
            let hovered = self.hovered == Some(Control::ClearSelection);
            self.button(list, layout.clear, "Clear", Style::Quiet, hovered, true);
        }
        let hovered = self.hovered == Some(Control::Spawn);
        self.button(
            list,
            layout.spawn,
            "SPAWN SELECTED",
            Style::Primary,
            hovered,
            count > 0,
        );
        self.scratch = scratch;
    }

    fn push_row(
        &self,
        list: &mut UiDrawList,
        rect: UiRect,
        row: usize,
        entry: usize,
        playground: &MonsterPlayground,
    ) {
        let s = &self.style;
        let text = &self.entries[entry];
        let chosen = playground.selection().contains(text.kind);
        let hovered = self.hovered == Some(Control::Row(row));
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
                blend(color::SURFACE_2, color::BRONZE, 0.10),
                color::SURFACE_2,
            );
        } else if hovered {
            list.panel(
                inset,
                s.px(radius::CONTROL),
                color::SURFACE_2,
                color::SURFACE_2,
            );
        }
        if hovered && chosen {
            list.panel_stroke(
                inset,
                s.px(radius::CONTROL),
                s.px(1.0),
                color::BORDER_STRONG,
            );
        }
        if self.focus_visible && self.focus == Some(entry) {
            list.panel_stroke(inset, s.px(radius::CONTROL), s.px(1.0), color::BRONZE);
        }
        let centre = rect.y + rect.height * 0.5;
        let box_size = s.px(16.0);
        let check = UiRect::new(
            inset.x + s.px(space::SM),
            centre - box_size * 0.5,
            box_size,
            box_size,
        );
        checkbox(list, s, check, chosen, hovered);
        let role_width = s.width(type_scale::META, text.role);
        let name_x = check.right() + s.px(space::SM);
        let name_width = inset.right() - s.px(space::SM) - role_width - s.px(space::SM) - name_x;
        self.fitted(
            list,
            (name_x, s.centred_top(type_scale::UI, centre)),
            &text.name,
            type_scale::UI,
            name_width,
            if chosen {
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
            text.role,
            type_scale::META,
            color::TEXT_MUTED,
            TextAlign::Right,
        );
    }

    fn push_detail(
        &mut self,
        list: &mut UiDrawList,
        rect: UiRect,
        entry: usize,
        playground: &MonsterPlayground,
        scratch: &mut String,
    ) {
        let s = self.style;
        let text = &self.entries[entry];
        let archetype = text.kind.archetype();
        let tint = tint_of(text.kind);
        list.panel(rect, s.px(radius::CARD), color::SURFACE_2, color::SURFACE_1);
        list.panel(
            UiRect::new(
                rect.x,
                rect.y + s.px(space::MD),
                s.px(3.0),
                rect.height - s.px(space::MD) * 2.0,
            ),
            s.px(1.5),
            tint,
            tint,
        );
        let x = rect.x + s.px(space::MD);
        let width = rect.width - s.px(space::MD) * 2.0;
        let mut y = rect.y + s.px(space::MD);
        self.fitted(
            list,
            (x, y),
            &text.name,
            type_scale::NAME,
            width,
            color::TEXT_PRIMARY,
        );
        y += s.line(type_scale::NAME);
        let level = playground.monster_level();
        let (health, _) = engine_game::enemy::level_scale(level);
        scratch.clear();
        let _ = write!(scratch, "Level {level} \u{b7} {} \u{b7} ", text.kind_name);
        push_thousands(scratch, archetype.max_health * health);
        scratch.push_str(" health");
        if archetype.phase_count() > 1 {
            let _ = write!(scratch, " \u{b7} {} phases", archetype.phase_count());
        }
        self.fitted(
            list,
            (x, y),
            scratch,
            type_scale::META,
            width,
            color::TEXT_SECONDARY,
        );
        y += s.line(type_scale::META);

        let key = (entry, width as u32);
        if self.summary.0 != Some(key) {
            self.summary = (
                Some(key),
                s.wrap(type_scale::META, archetype.summary, width),
            );
        }
        for line in self.summary.1.iter().take(2) {
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
        if self.summary.1.len() < 2 {
            y += s.line(type_scale::META) * (2 - self.summary.1.len()) as f32;
        }
        y += s.px(space::SM);
        let text = &self.entries[entry];
        scratch.clear();
        let _ = write!(scratch, "ABILITIES \u{b7} {}", text.abilities.len());
        self.text(
            list,
            (x, y),
            scratch,
            type_scale::LABEL,
            color::BRONZE,
            TextAlign::Left,
        );
        y += s.line(type_scale::LABEL);
        let overflow = text.abilities.len() > DETAIL_ABILITY_LINES;
        let shown = if overflow {
            DETAIL_ABILITY_LINES - 1
        } else {
            text.abilities.len()
        };
        for (name, meta) in text.abilities.iter().take(shown) {
            let meta_width = s.width(type_scale::META, meta);
            self.text(
                list,
                (x, y),
                "\u{2022}",
                type_scale::META,
                tint,
                TextAlign::Left,
            );
            let name_x = x + s.px(10.0);
            self.fitted(
                list,
                (name_x, y),
                name,
                type_scale::META,
                x + width - meta_width - s.px(space::SM) - name_x,
                color::TEXT_PRIMARY,
            );
            self.text(
                list,
                (x + width, y),
                meta,
                type_scale::META,
                color::TEXT_MUTED,
                TextAlign::Right,
            );
            y += s.line(type_scale::META);
        }
        if overflow {
            scratch.clear();
            let _ = write!(scratch, "+{} more", text.abilities.len() - shown);
            self.text(
                list,
                (x + s.px(10.0), y),
                scratch,
                type_scale::META,
                color::TEXT_MUTED,
                TextAlign::Left,
            );
        }
    }

    /// The details card before anything has been pointed at.
    fn push_hint_card(&self, list: &mut UiDrawList, rect: UiRect, title: &str, line: &str) {
        let s = &self.style;
        list.panel(rect, s.px(radius::CARD), color::SURFACE_1, color::SURFACE_1);
        list.panel_stroke(rect, s.px(radius::CARD), s.px(1.0), color::BORDER);
        let centre_x = rect.x + rect.width * 0.5;
        let centre_y = rect.y + rect.height * 0.5;
        self.text(
            list,
            (centre_x, centre_y - s.line(type_scale::UI)),
            title,
            type_scale::UI,
            color::TEXT_SECONDARY,
            TextAlign::Center,
        );
        self.text(
            list,
            (centre_x, centre_y),
            line,
            type_scale::META,
            color::TEXT_MUTED,
            TextAlign::Center,
        );
    }

    fn push_controls(
        &mut self,
        list: &mut UiDrawList,
        layout: &Layout,
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) {
        let s = self.style;
        self.panel(list, layout.right);
        let rx = layout.right.x + s.px(space::LG);
        let rw = layout.right.width - s.px(space::LG) * 2.0;
        self.text(
            list,
            layout.right_title,
            "Playground",
            type_scale::ENCOUNTER,
            color::TEXT_TITLE,
            TextAlign::Left,
        );
        self.text(
            list,
            layout.right_subtitle,
            "Monster combat test controls",
            type_scale::META,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );

        // The player.
        let mut scratch = std::mem::take(&mut self.scratch);
        self.label(list, (rx, layout.player_label_y), "PLAYER");
        let world = simulation.world();
        let (current, max) = world
            .health(simulation.player().entity)
            .map_or((0.0, 0.0), |health| (health.current(), health.max()));
        scratch.clear();
        push_thousands(&mut scratch, current);
        scratch.push_str(" / ");
        push_thousands(&mut scratch, max);
        self.text(
            list,
            (rx + rw, layout.player_label_y),
            &scratch,
            type_scale::META,
            if simulation.player_alive() {
                color::VERDANT
            } else {
                color::CRIMSON
            },
            TextAlign::Right,
        );
        let mode = playground.health_mode();
        self.segment(
            list,
            layout.health_test,
            "Test HP",
            mode == HealthMode::Test,
            self.hovered == Some(Control::HealthTest),
            true,
        );
        self.segment(
            list,
            layout.health_normal,
            "Normal HP",
            mode == HealthMode::Normal,
            self.hovered == Some(Control::HealthNormal),
            false,
        );
        self.button(
            list,
            layout.reset_player,
            "Reset player",
            Style::Secondary,
            self.hovered == Some(Control::ResetPlayer),
            true,
        );
        // The hero: the Druid is the healer a dispel is tested with.
        for (index, hero) in Hero::ALL.into_iter().enumerate() {
            let chosen = simulation.hero() == hero;
            let hovered = self.hovered == Some(Control::Hero(index));
            let rect = layout.heroes[index];
            let corner = s.px(radius::CONTROL);
            list.panel(
                rect,
                corner,
                if chosen {
                    blend(color::SURFACE_2, color::BRONZE, 0.25)
                } else if hovered {
                    color::SURFACE_3
                } else {
                    color::SURFACE_1
                },
                color::SURFACE_1,
            );
            list.panel_stroke(
                rect,
                corner,
                s.px(1.0),
                if chosen { color::BRONZE } else { color::BORDER },
            );
            self.text(
                list,
                (
                    rect.x + rect.width * 0.5,
                    s.centred_top(type_scale::META, rect.y + rect.height * 0.5),
                ),
                effects::hero_label(hero),
                type_scale::META,
                if chosen {
                    color::TEXT_PRIMARY
                } else {
                    color::TEXT_SECONDARY
                },
                TextAlign::Center,
            );
        }

        // Spawning.
        self.label(list, (rx, layout.spawning_label_y), "SPAWNING");
        let passive = layout.passive;
        let hovered = self.hovered == Some(Control::Passive);
        let box_size = s.px(16.0);
        let centre = passive.y + passive.height * 0.5;
        checkbox(
            list,
            &s,
            UiRect::new(passive.x, centre - box_size * 0.5, box_size, box_size),
            playground.is_passive(),
            hovered,
        );
        self.text(
            list,
            (
                passive.x + box_size + s.px(space::SM),
                s.centred_top(type_scale::UI, centre),
            ),
            "Passive",
            type_scale::UI,
            if hovered {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_SECONDARY
            },
            TextAlign::Left,
        );
        let level = playground.monster_level();
        self.text(
            list,
            (
                layout.level_down.x - s.px(space::SM),
                s.centred_top(type_scale::UI, centre),
            ),
            "Level",
            type_scale::UI,
            color::TEXT_SECONDARY,
            TextAlign::Right,
        );
        self.button(
            list,
            layout.level_down,
            "-",
            Style::Secondary,
            self.hovered == Some(Control::LevelDown),
            level > LEVEL_RANGE.0,
        );
        scratch.clear();
        let _ = write!(scratch, "{level}");
        self.text(
            list,
            (
                (layout.level_down.right() + layout.level_up.x) * 0.5,
                s.centred_top(type_scale::UI, centre),
            ),
            &scratch,
            type_scale::UI,
            color::TEXT_PRIMARY,
            TextAlign::Center,
        );
        self.button(
            list,
            layout.level_up,
            "+",
            Style::Secondary,
            self.hovered == Some(Control::LevelUp),
            level < LEVEL_RANGE.1,
        );

        if self.mode == PlaygroundMode::Effects {
            self.push_effect_controls(list, layout, playground, simulation, &mut scratch);
            self.scratch = scratch;
            return;
        }

        // The encounter.
        self.label(list, (rx, layout.encounter_label_y), "CURRENT ENCOUNTER");
        let encounter = playground.encounter();
        let living = encounter.living(simulation);
        scratch.clear();
        if encounter.is_empty() {
            scratch.push_str("empty");
        } else if living == encounter.len() {
            let _ = write!(scratch, "{living} active");
        } else {
            let _ = write!(scratch, "{living} of {} alive", encounter.len());
        }
        self.text(
            list,
            (rx + rw, layout.encounter_label_y),
            &scratch,
            type_scale::META,
            if living > 0 {
                color::TEXT_SECONDARY
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Right,
        );
        let window = layout.encounter;
        list.panel(
            window,
            s.px(radius::CARD),
            color::SURFACE_1,
            color::SURFACE_1,
        );
        if encounter.is_empty() {
            let centre_x = window.x + window.width * 0.5;
            let y = window.y + s.px(space::XL);
            self.text(
                list,
                (centre_x, y),
                "No active monsters",
                type_scale::UI,
                color::TEXT_SECONDARY,
                TextAlign::Center,
            );
            self.text(
                list,
                (centre_x, y + s.line(type_scale::UI)),
                "Select monsters, then Spawn Selected",
                type_scale::META,
                color::TEXT_MUTED,
                TextAlign::Center,
            );
        } else {
            list.clip(UiClip::rect(window));
            let target = simulation.player().target;
            for (index, member) in encounter.members().iter().enumerate() {
                let rect = UiRect::new(
                    window.x,
                    window.y + index as f32 * layout.member_height - self.encounter_scroll,
                    window.width,
                    layout.member_height,
                );
                if rect.bottom() < window.y || rect.y > window.bottom() {
                    continue;
                }
                self.push_member(
                    list,
                    rect,
                    index,
                    member.entity,
                    member.kind,
                    target == Some(member.entity),
                    simulation,
                    &mut scratch,
                );
            }
            list.clip(UiClip::NONE);
        }
        list.panel_stroke(window, s.px(radius::CARD), s.px(1.0), color::BORDER);
        self.scrollbar(
            list,
            window,
            self.encounter_scroll,
            encounter.len() as f32 * layout.member_height,
        );

        let any = !encounter.is_empty();
        self.button(
            list,
            layout.kill,
            "Kill all",
            Style::Secondary,
            self.hovered == Some(Control::KillAll),
            any && living > 0,
        );
        self.button(
            list,
            layout.reset,
            "RESET ENCOUNTER",
            Style::Danger,
            self.hovered == Some(Control::Reset),
            any,
        );
        let hints = [
            "Enter spawn \u{b7} Del reset \u{b7} K kill all \u{b7} P passive",
            "Up/Down \u{b7} Right select \u{b7} H interrupt \u{b7} L hide",
        ];
        let hints = if layout.hints { &hints[..] } else { &[] };
        for (line, hint) in hints.iter().enumerate() {
            self.fitted(
                list,
                (rx, layout.hint_y + s.line(type_scale::META) * line as f32),
                hint,
                type_scale::META,
                rw,
                color::TEXT_MUTED,
            );
        }
        self.scratch = scratch;
    }

    /// The right panel's lower half in Effects mode, and its buttons.
    fn push_effect_controls(
        &mut self,
        list: &mut UiDrawList,
        layout: &Layout,
        playground: &MonsterPlayground,
        simulation: &Simulation,
        scratch: &mut String,
    ) {
        let s = self.style;
        let rx = layout.right.x + s.px(space::LG);
        let rw = layout.right.width - s.px(space::LG) * 2.0;
        self.label(list, (rx, layout.encounter_label_y), "EFFECT TESTING");
        self.text(
            list,
            (rx + rw, layout.encounter_label_y),
            "click a name to test on it",
            type_scale::META,
            color::TEXT_MUTED,
            TextAlign::Right,
        );
        self.push_inspector(list, layout, playground, simulation, scratch);
        self.button(
            list,
            layout.test_effects,
            "Add test effects (every effect)",
            Style::Secondary,
            self.hovered == Some(Control::TestEffects),
            true,
        );
        self.button(
            list,
            layout.clear_buffs,
            "Clear buffs",
            Style::Secondary,
            self.hovered == Some(Control::ClearBuffs),
            true,
        );
        self.button(
            list,
            layout.clear_debuffs,
            "Clear debuffs",
            Style::Secondary,
            self.hovered == Some(Control::ClearDebuffs),
            true,
        );
        self.button(
            list,
            layout.reset,
            "RESET ENCOUNTER",
            Style::Danger,
            self.hovered == Some(Control::Reset),
            !playground.encounter().is_empty(),
        );
        let hints = [
            "Enter apply \u{b7} Up/Down choose \u{b7} Del reset",
            "Drop a debuff on a friend, then dispel it",
        ];
        let hints = if layout.hints { &hints[..] } else { &[] };
        for (line, hint) in hints.iter().enumerate() {
            self.fitted(
                list,
                (rx, layout.hint_y + s.line(type_scale::META) * line as f32),
                hint,
                type_scale::META,
                rw,
                color::TEXT_MUTED,
            );
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one encounter row, fully described"
    )]
    fn push_member(
        &self,
        list: &mut UiDrawList,
        rect: UiRect,
        index: usize,
        entity: EntityId,
        kind: EnemyKind,
        targeted: bool,
        simulation: &Simulation,
        scratch: &mut String,
    ) {
        let s = &self.style;
        let world = simulation.world();
        let alive = world.is_alive(entity);
        let fraction = world.health(entity).map_or(0.0, |health| health.fraction());
        let hovered = self.hovered == Some(Control::Member(index)) && alive;
        let inset = UiRect::new(
            rect.x + s.px(space::XS),
            rect.y + s.px(2.0),
            rect.width - s.px(space::XS) * 2.0,
            rect.height - s.px(4.0),
        );
        if targeted {
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
        let x = inset.x + s.px(space::SM);
        let right = inset.right() - s.px(space::SM);
        let name_centre = inset.y + s.px(13.0);
        let dot = s.px(4.0);
        list.disc(
            (x + dot, name_centre),
            dot,
            if alive {
                tint_of(kind)
            } else {
                color::TEXT_MUTED
            },
        );
        scratch.clear();
        if alive {
            let _ = write!(scratch, "{:.0}%", (fraction * 100.0).ceil());
        } else {
            scratch.push_str("DEAD");
        }
        let percent_width = s.width(type_scale::META, scratch);
        self.text(
            list,
            (right, s.centred_top(type_scale::META, name_centre)),
            scratch,
            type_scale::META,
            if alive {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_MUTED
            },
            TextAlign::Right,
        );
        let name_x = x + dot * 2.0 + s.px(space::SM);
        let name = &self.entries[self.by_kind[kind.index()]].name;
        self.fitted(
            list,
            (name_x, s.centred_top(type_scale::UI, name_centre)),
            name,
            type_scale::UI,
            right - percent_width - s.px(space::SM) - name_x,
            if alive {
                color::TEXT_PRIMARY
            } else {
                color::TEXT_MUTED
            },
        );
        // What it is doing, from its brain.
        let state_centre = inset.y + s.px(28.0);
        scratch.clear();
        let mut tint = color::TEXT_MUTED;
        if let Some(brain) = simulation.brain(entity) {
            if let Some(cast) = brain.cast().filter(|_| alive) {
                scratch.push_str("Casting ");
                scratch.push_str(&self.ability_names[cast.ability.index()]);
                tint = color::AMBER;
            } else if alive && brain.is_stunned() {
                scratch.push_str("Stunned");
                tint = color::FROST;
            } else {
                scratch.push_str(state_name(brain.state()));
                if brain.state().is_combat() {
                    tint = color::TEXT_SECONDARY;
                }
            }
        }
        self.fitted(
            list,
            (name_x, s.centred_top(type_scale::META, state_centre)),
            scratch,
            type_scale::META,
            right - name_x,
            tint,
        );
        let bar = UiRect::new(
            name_x,
            inset.bottom() - s.px(7.0),
            right - name_x,
            s.px(3.0),
        );
        list.panel(bar, s.px(1.5), color::SURFACE_3, color::SURFACE_3);
        if alive && fraction > 0.0 {
            let fill = UiRect::new(
                bar.x,
                bar.y,
                (bar.width * fraction).max(bar.height),
                bar.height,
            );
            list.panel(fill, s.px(1.5), color::CRIMSON, color::CRIMSON);
        }
    }

    fn scrollbar(&self, list: &mut UiDrawList, window: UiRect, scroll: f32, content: f32) {
        let s = &self.style;
        let max = (content - window.height).max(0.0);
        if max <= 0.0 {
            return;
        }
        let track = window.inset(s.px(space::XS));
        let thumb_height = (track.height * window.height / content).max(s.px(20.0));
        let thumb_y = track.y + (track.height - thumb_height) * (scroll / max);
        let thumb = UiRect::new(track.right() - s.px(3.0), thumb_y, s.px(3.0), thumb_height);
        list.panel(thumb, s.px(1.5), color::BRONZE_DIM, color::BRONZE_DIM);
    }

    fn label(&self, list: &mut UiDrawList, at: (f32, f32), text: &str) {
        self.text(
            list,
            at,
            text,
            type_scale::LABEL,
            color::BRONZE,
            TextAlign::Left,
        );
    }

    fn segment(
        &self,
        list: &mut UiDrawList,
        rect: UiRect,
        text: &str,
        active: bool,
        hovered: bool,
        first: bool,
    ) {
        let s = &self.style;
        // Two halves of one control: the outer corners rounded, the seam square. Drawn as a
        // rounded rectangle under a square patch over the inner side.
        let corner = s.px(radius::CONTROL);
        let (top, bottom) = if active {
            (color::BRONZE, color::BRONZE_DIM)
        } else if hovered {
            (color::SURFACE_3, color::SURFACE_2)
        } else {
            (color::SURFACE_2, color::SURFACE_1)
        };
        list.panel(rect, corner, top, bottom);
        let patch = if first {
            UiRect::new(rect.right() - corner, rect.y, corner, rect.height)
        } else {
            UiRect::new(rect.x, rect.y, corner, rect.height)
        };
        list.panel(patch, 0.0, top, bottom);
        list.panel_stroke(
            rect,
            corner,
            s.px(1.0),
            if active {
                color::BRONZE_BRIGHT
            } else {
                color::BORDER_STRONG
            },
        );
        self.text(
            list,
            (
                rect.x + rect.width * 0.5,
                s.centred_top(type_scale::UI, rect.y + rect.height * 0.5),
            ),
            text,
            type_scale::UI,
            if active {
                color::SURFACE_0_BOTTOM
            } else {
                color::TEXT_SECONDARY
            },
            TextAlign::Center,
        );
    }

    fn button(
        &self,
        list: &mut UiDrawList,
        rect: UiRect,
        text: &str,
        style: Style,
        hovered: bool,
        enabled: bool,
    ) {
        let s = &self.style;
        let corner = s.px(radius::CONTROL);
        let hovered = hovered && enabled;
        let (top, bottom, stroke, ink) = match (style, enabled) {
            (_, false) => (
                color::SURFACE_1,
                color::SURFACE_1,
                color::BORDER,
                color::TEXT_MUTED,
            ),
            (Style::Primary, true) => (
                if hovered {
                    color::BRONZE_BRIGHT
                } else {
                    color::BRONZE
                },
                color::BRONZE_DIM,
                color::BRONZE_BRIGHT,
                color::SURFACE_0_BOTTOM,
            ),
            (Style::Danger, true) => (
                blend(
                    color::SURFACE_2,
                    color::CRIMSON,
                    if hovered { 0.55 } else { 0.38 },
                ),
                blend(color::SURFACE_1, color::CRIMSON, 0.22),
                color::alpha(color::CRIMSON, if hovered { 1.0 } else { 0.7 }),
                color::TEXT_PRIMARY,
            ),
            (Style::Secondary, true) => (
                if hovered {
                    color::SURFACE_3
                } else {
                    color::SURFACE_2
                },
                color::SURFACE_1,
                if hovered {
                    color::BRONZE_BRIGHT
                } else {
                    color::BORDER_STRONG
                },
                color::TEXT_PRIMARY,
            ),
            (Style::Quiet, true) => (
                if hovered {
                    color::SURFACE_3
                } else {
                    color::alpha(color::SURFACE_2, 0.0)
                },
                if hovered {
                    color::SURFACE_2
                } else {
                    color::alpha(color::SURFACE_2, 0.0)
                },
                color::alpha(color::BORDER, 0.0),
                color::TEXT_SECONDARY,
            ),
        };
        list.panel(rect, corner, top, bottom);
        list.panel_stroke(rect, corner, s.px(1.0), stroke);
        let font = if matches!(style, Style::Primary | Style::Danger) {
            type_scale::HEADING
        } else {
            type_scale::UI
        };
        self.text(
            list,
            (
                rect.x + rect.width * 0.5,
                s.centred_top(font, rect.y + rect.height * 0.5),
            ),
            text,
            font,
            ink,
            TextAlign::Center,
        );
    }

    /// `text`, cut with an ellipsis only when it does not fit `width`.
    fn fitted(
        &self,
        list: &mut UiDrawList,
        at: (f32, f32),
        text: &str,
        style: TextStyle,
        width: f32,
        tint: UiColor,
    ) {
        if self.style.width(style, text) <= width {
            self.text(list, at, text, style, tint, TextAlign::Left);
        } else {
            let cut = self.style.fit(style, text, width.max(0.0));
            self.text(list, at, &cut, style, tint, TextAlign::Left);
        }
    }

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

/// How a button looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// The one thing the panel is for.
    Primary,
    /// Takes something away.
    Danger,
    /// Everything else.
    Secondary,
    /// A text button with no box until hovered.
    Quiet,
}

fn row_height(layout: &Layout, row: Row) -> f32 {
    match row {
        Row::Heading(_) => layout.heading_height,
        Row::Monster(_) => layout.row_height,
    }
}

/// A checkbox: an outline when clear, bronze with a tick when chosen.
fn checkbox(list: &mut UiDrawList, s: &MapStyle, rect: UiRect, checked: bool, hovered: bool) {
    let corner = s.px(radius::CHIP);
    if checked {
        list.panel(rect, corner, color::BRONZE_BRIGHT, color::BRONZE);
        // The tick, as a run of overlapping dots along its two strokes.
        let points = [(0.22, 0.52), (0.42, 0.72), (0.80, 0.30)];
        let dot = (rect.width * 0.075).max(0.9);
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let length = ((b.0 - a.0) * rect.width).hypot((b.1 - a.1) * rect.height);
            let steps = (length / (dot * 0.6)).ceil().max(1.0) as usize;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                list.disc(
                    (
                        rect.x + (a.0 + (b.0 - a.0) * t) * rect.width,
                        rect.y + (a.1 + (b.1 - a.1) * t) * rect.height,
                    ),
                    dot,
                    color::SURFACE_0_BOTTOM,
                );
            }
        }
    } else {
        list.panel(
            rect,
            corner,
            color::SURFACE_0_BOTTOM,
            color::SURFACE_0_BOTTOM,
        );
        list.panel_stroke(
            rect,
            corner,
            s.px(1.0),
            if hovered {
                color::BRONZE_BRIGHT
            } else {
                color::BORDER_STRONG
            },
        );
    }
}

/// A key's character in the search field, if it types one.
fn typed(key: KeyCode) -> Option<char> {
    let index = key.index();
    if (KeyCode::A.index()..=KeyCode::Z.index()).contains(&index) {
        return Some(char::from(b'a' + (index - KeyCode::A.index()) as u8));
    }
    if let Some(digit) = (0..=9u8).find(|digit| KeyCode::digit_key(*digit) == Some(key)) {
        return Some(char::from(b'0' + digit));
    }
    match key {
        KeyCode::Space => Some(' '),
        KeyCode::Minus => Some('-'),
        _ => None,
    }
}

/// "2.0s cast · 8s" or "Instant · 6s", for an ability line.
fn ability_meta(ability: EnemyAbility) -> String {
    let definition = ability.definition();
    let mut meta = if definition.is_cast() {
        format!("{:.1}s cast", definition.cast_time)
    } else {
        "Instant".to_owned()
    };
    if definition.cooldown > 0.0 {
        let _ = write!(meta, " \u{b7} {}s", trim_seconds(definition.cooldown));
    }
    meta
}

/// `8.0` as `"8"`, `2.5` as `"2.5"`.
fn trim_seconds(seconds: f32) -> String {
    if (seconds - seconds.round()).abs() < 0.05 {
        format!("{:.0}", seconds.round())
    } else {
        format!("{seconds:.1}")
    }
}

fn state_name(state: AiState) -> &'static str {
    match state {
        AiState::Idle => "Idle",
        AiState::Alert => "Noticed you",
        AiState::Chase => "Chasing",
        AiState::Attack => "Attacking",
        AiState::Cast => "Casting",
        AiState::Recover => "Recovering",
        AiState::Returning => "Returning home",
        AiState::Dead => "Dead",
    }
}

fn tint_of(kind: EnemyKind) -> UiColor {
    let tint = kind.archetype().look.tint;
    [tint[0], tint[1], tint[2], 1.0]
}

fn short_role(role: MonsterRole) -> &'static str {
    match role {
        MonsterRole::Melee => "Melee",
        MonsterRole::Ranged => "Ranged",
        MonsterRole::Support => "Support",
        MonsterRole::Elite => "Elite",
        MonsterRole::Boss => "Boss",
    }
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

/// Appends `value`, rounded, with thousands separators: `1234567.8` as `"1,234,568"`.
fn push_thousands(out: &mut String, value: f32) {
    let whole = value.max(0.0).round() as u64;
    let mut digits = [0u8; 20];
    let mut count = 0;
    let mut rest = whole;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for index in (0..count).rev() {
        out.push(char::from(digits[index]));
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::aura::{Aura, AuraKind};
    use engine_core::input::{KeyChord, Modifiers};
    use engine_game::arena::{self, ArenaConfig};

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn setup() -> (Simulation, MonsterPlayground, PlaygroundPanels) {
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(0));
        let mut playground = MonsterPlayground::default();
        playground.install(&mut simulation);
        let panels = PlaygroundPanels::new(UiScale::default(), &playground);
        (simulation, playground, panels)
    }

    fn click(at: (f32, f32)) -> UiInput {
        UiInput {
            pointer: at,
            pointer_down: true,
            pointer_pressed: true,
            ..UiInput::default()
        }
    }

    fn centre(rect: UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    /// Where the visible row showing `kind` is, before scrolling.
    fn row_of(panels: &PlaygroundPanels, kind: EnemyKind) -> UiRect {
        let layout = panels.layout(VIEWPORT);
        let mut top = layout.list.y - panels.list_scroll;
        for row in &panels.rows {
            let height = row_height(&layout, *row);
            if let Row::Monster(index) = row
                && panels.entries[*index].kind == kind
            {
                return UiRect::new(layout.list.x, top, layout.list.width, height);
            }
            top += height;
        }
        panic!("{kind:?} is not shown");
    }

    fn apply(
        action: PlaygroundAction,
        playground: &mut MonsterPlayground,
        simulation: &mut Simulation,
    ) {
        match action {
            PlaygroundAction::Toggle(kind) => {
                playground.selection_mut().toggle(kind);
            }
            PlaygroundAction::SpawnSelected => {
                let origin = simulation.player_position();
                playground.spawn_selected(simulation, origin, 0.0);
            }
            PlaygroundAction::ResetEncounter => {
                playground.reset_encounter(simulation);
            }
            _ => {}
        }
    }

    #[test]
    fn every_registered_monster_is_listed_under_its_tier() {
        let (_, playground, panels) = setup();
        assert_eq!(panels.visible_monsters(), EnemyKind::ALL.len());
        assert!(matches!(panels.rows[0], Row::Heading(MonsterTier::Normal)));
        assert_eq!(
            panels
                .rows
                .iter()
                .filter(|row| matches!(row, Row::Heading(_)))
                .count(),
            3
        );
        assert_eq!(panels.entries.len(), playground.library().len());
    }

    #[test]
    fn clicking_rows_toggles_them_and_spawn_spawns_the_selection() {
        let (mut simulation, mut playground, mut panels) = setup();
        let layout = panels.layout(VIEWPORT);
        // Spawn does nothing with nothing chosen.
        let action = panels.update(
            &click(centre(layout.spawn)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert_eq!(action, PlaygroundAction::None);

        for kind in [EnemyKind::CryptArcher, EnemyKind::GraveBrute] {
            let action = panels.update(
                &click(centre(row_of(&panels, kind))),
                0.0,
                PlaygroundKeys::default(),
                VIEWPORT,
                &playground,
                &simulation,
            );
            assert_eq!(action, PlaygroundAction::Toggle(kind));
            apply(action, &mut playground, &mut simulation);
        }
        assert_eq!(playground.selection().len(), 2);
        let action = panels.update(
            &click(centre(layout.spawn)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert_eq!(action, PlaygroundAction::SpawnSelected);
        apply(action, &mut playground, &mut simulation);
        assert_eq!(playground.encounter().len(), 2);

        // A click on an encounter row targets that monster.
        let first = UiRect::new(
            layout.encounter.x,
            layout.encounter.y,
            layout.encounter.width,
            layout.member_height,
        );
        let action = panels.update(
            &click(centre(first)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert_eq!(
            action,
            PlaygroundAction::Target(playground.encounter().members()[0].entity)
        );

        let action = panels.update(
            &click(centre(layout.reset)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert_eq!(action, PlaygroundAction::ResetEncounter);
    }

    #[test]
    fn the_player_controls_ask_for_what_they_say() {
        let (simulation, playground, mut panels) = setup();
        let layout = panels.layout(VIEWPORT);
        let mut press = |rect: UiRect| {
            panels.update(
                &click(centre(rect)),
                0.0,
                PlaygroundKeys::default(),
                VIEWPORT,
                &playground,
                &simulation,
            )
        };
        assert_eq!(
            press(layout.health_normal),
            PlaygroundAction::SetHealth(HealthMode::Normal)
        );
        assert_eq!(
            press(layout.health_test),
            PlaygroundAction::SetHealth(HealthMode::Test)
        );
        assert_eq!(press(layout.reset_player), PlaygroundAction::ResetPlayer);
        assert_eq!(press(layout.passive), PlaygroundAction::SetPassive(true));
        assert_eq!(press(layout.level_up), PlaygroundAction::SetLevel(2));
        assert_eq!(
            press(layout.level_down),
            PlaygroundAction::None,
            "level one is the floor"
        );
    }

    #[test]
    fn typing_in_the_search_field_filters_the_library() {
        let (simulation, playground, mut panels) = setup();
        let layout = panels.layout(VIEWPORT);
        panels.update(
            &click(centre(layout.search)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert!(panels.is_typing());
        for key in [KeyCode::A, KeyCode::R, KeyCode::C, KeyCode::H] {
            let input = UiInput {
                captured_key: Some(KeyChord::new(key, Modifiers::none())),
                ..UiInput::default()
            };
            let action = panels.update(
                &input,
                0.0,
                PlaygroundKeys::default(),
                VIEWPORT,
                &playground,
                &simulation,
            );
            assert_eq!(action, PlaygroundAction::None, "typing is not playing");
        }
        assert_eq!(panels.search(), "arch");
        // A click on a row ends the typing and is that row's.
        let row = row_of(&panels, EnemyKind::CryptArcher);
        assert_eq!(
            panels.update(
                &click(centre(row)),
                0.0,
                PlaygroundKeys::default(),
                VIEWPORT,
                &playground,
                &simulation,
            ),
            PlaygroundAction::Toggle(EnemyKind::CryptArcher)
        );
        assert!(!panels.is_typing());
        panels.update(
            &click(centre(layout.search)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert!(panels.is_typing());
        assert!(
            panels.visible_monsters() >= 2,
            "the archer and its empowered form"
        );
        assert!(panels.visible_monsters() < EnemyKind::ALL.len());
        let enter = UiInput {
            captured_key: Some(KeyChord::new(KeyCode::Enter, Modifiers::none())),
            ..UiInput::default()
        };
        panels.update(
            &enter,
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert!(!panels.is_typing());
        // The clear button brings everything back.
        panels.update(
            &click(centre(layout.search_clear)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert_eq!(panels.search(), "");
        assert_eq!(panels.visible_monsters(), EnemyKind::ALL.len());
    }

    #[test]
    fn the_keys_browse_toggle_spawn_and_reset() {
        let (simulation, mut playground, mut panels) = setup();
        let keys = |set: fn(&mut PlaygroundKeys)| {
            let mut keys = PlaygroundKeys::default();
            set(&mut keys);
            keys
        };
        let none = UiInput::default();
        panels.update(
            &none,
            0.0,
            keys(|k| k.down = true),
            VIEWPORT,
            &playground,
            &simulation,
        );
        let action = panels.update(
            &none,
            0.0,
            keys(|k| k.toggle = true),
            VIEWPORT,
            &playground,
            &simulation,
        );
        let first = playground.library().entries()[0].kind;
        assert_eq!(action, PlaygroundAction::Toggle(first));
        playground.selection_mut().toggle(first);
        assert_eq!(
            panels.update(
                &none,
                0.0,
                keys(|k| k.spawn = true),
                VIEWPORT,
                &playground,
                &simulation
            ),
            PlaygroundAction::SpawnSelected
        );
        assert_eq!(
            panels.update(
                &none,
                0.0,
                keys(|k| k.reset = true),
                VIEWPORT,
                &playground,
                &simulation
            ),
            PlaygroundAction::ResetEncounter
        );
        assert_eq!(
            panels.update(
                &none,
                0.0,
                keys(|k| k.kill = true),
                VIEWPORT,
                &playground,
                &simulation
            ),
            PlaygroundAction::KillEncounter
        );
    }

    #[test]
    fn the_panels_fit_the_screen_and_leave_the_middle_to_the_fight() {
        let (_, playground, _) = setup();
        for viewport in [(1280, 720), (1920, 1080), (3840, 2160)] {
            for factor in [1.0, 1.5] {
                let panels = PlaygroundPanels::new(UiScale::new(factor), &playground);
                let layout = panels.layout(viewport);
                let (w, h) = (viewport.0 as f32, viewport.1 as f32);
                for rect in [layout.left, layout.right] {
                    assert!(rect.x >= 0.0 && rect.right() <= w, "{viewport:?} x{factor}");
                    assert!(rect.bottom() <= h + 0.5, "{viewport:?} x{factor}");
                }
                assert!(layout.left.right() < layout.right.x);
                let (left, right) = panels.reserved(viewport);
                assert!(
                    w - left - right >= w * 0.45,
                    "{viewport:?} x{factor}: the fight gets {} px",
                    w - left - right
                );
                assert!(layout.list.height >= layout.row_height * 3.0);
                assert!(layout.encounter.bottom() <= layout.kill.y);
                assert!(
                    layout.kill.bottom() <= layout.reset.y
                        || (layout.kill.right() <= layout.reset.x
                            && layout.kill.y == layout.reset.y),
                    "{viewport:?} x{factor}: the buttons overlap"
                );
                // Two monsters in view at the normal scale; at 720p and half again as large
                // the list scrolls, and at least one is always in view.
                let rows = if factor > 1.0 && viewport.1 < 1000 {
                    1.0
                } else {
                    2.0
                };
                assert!(
                    layout.encounter.height >= layout.member_height * rows - 0.5,
                    "{viewport:?} x{factor}: the encounter has room for {} rows",
                    layout.encounter.height / layout.member_height
                );
                assert!(layout.reset.bottom() <= layout.right.bottom());
                assert!(layout.spawn.bottom() <= layout.left.bottom());
                // The effects footer: TIME - 30s +  STACKS - 1 +, nothing on anything.
                assert!(
                    layout.time_up.right() + panels.style.px(space::XS) <= layout.stacks_down.x,
                    "{viewport:?} x{factor}: the steppers overlap"
                );
                if factor == 1.0 {
                    let stacks_label = panels.style.width(type_scale::LABEL, "STACKS");
                    assert!(
                        layout.time_up.right() + stacks_label + panels.style.px(space::SM)
                            <= layout.stacks_down.x,
                        "{viewport:?}: the STACKS label has no room at the normal scale"
                    );
                }
                assert!(layout.stacks_up.right() <= layout.left.right());
                assert!(panels.wants_pointer(centre(layout.spawn), viewport));
                assert!(!panels.wants_pointer((w * 0.5, h * 0.5), viewport));
            }
        }
    }

    #[test]
    fn hiding_gives_back_the_pointer_and_the_hud_space() {
        let (simulation, playground, mut panels) = setup();
        let hide = PlaygroundKeys {
            hide: true,
            ..PlaygroundKeys::default()
        };
        panels.update(
            &UiInput::default(),
            0.0,
            hide,
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert!(panels.is_hidden());
        assert_eq!(panels.reserved(VIEWPORT), (0.0, 0.0));
        let layout = panels.layout(VIEWPORT);
        assert!(!panels.wants_pointer(centre(layout.spawn), VIEWPORT));
        assert!(panels.wants_pointer(centre(layout.show), VIEWPORT));
        panels.update(
            &click(centre(layout.show)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            &playground,
            &simulation,
        );
        assert!(!panels.is_hidden());
    }

    #[test]
    fn the_panels_draw_the_library_the_details_and_the_encounter() {
        let (mut simulation, mut playground, mut panels) = setup();
        playground.selection_mut().select(EnemyKind::CryptArcher);
        let origin = simulation.player_position();
        playground.spawn_selected(&mut simulation, origin, 0.0);
        let mut list = UiDrawList::new();
        panels.push(&mut list, VIEWPORT, &playground, &simulation);
        let text = list.text();
        assert!(text.contains("Monster Library"));
        assert!(text.contains("Playground"));
        assert!(text.contains("SPAWN SELECTED"));
        assert!(text.contains("RESET ENCOUNTER"));
        assert!(text.contains("1 MONSTER SELECTED"));
        assert!(text.contains("Crypt Archer"));
        assert!(text.contains("1,000,000 / 1,000,000"));
        assert!(text.contains("100%"));
        // The details card lists the archer's own abilities, from its table row.
        for ability in EnemyKind::CryptArcher.archetype().abilities() {
            assert!(
                text.contains(&title_case(ability.name())),
                "{:?} is missing",
                ability
            );
        }
    }

    /// The panels in Effects mode, with their inspector built.
    fn effects_mode(
        panels: &mut PlaygroundPanels,
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) -> Layout {
        let layout = panels.layout(VIEWPORT);
        panels.update(
            &click(centre(layout.tab_effects)),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            playground,
            simulation,
        );
        assert_eq!(panels.mode(), PlaygroundMode::Effects);
        panels.layout(VIEWPORT)
    }

    fn press(
        panels: &mut PlaygroundPanels,
        at: (f32, f32),
        playground: &MonsterPlayground,
        simulation: &Simulation,
    ) -> PlaygroundAction {
        panels.update(
            &click(at),
            0.0,
            PlaygroundKeys::default(),
            VIEWPORT,
            playground,
            simulation,
        )
    }

    /// Where the inspector draws item `index`, before scrolling.
    fn item_rect(panels: &PlaygroundPanels, layout: &Layout, index: usize) -> UiRect {
        let mut top = layout.encounter.y - panels.encounter_scroll;
        for (at, item) in panels.items.iter().enumerate() {
            let height = panels.item_height(layout, item);
            if at == index {
                return UiRect::new(layout.encounter.x, top, layout.encounter.width, height);
            }
            top += height;
        }
        panic!("no item {index}");
    }

    #[test]
    fn the_effect_library_is_the_registry_split_into_buffs_and_debuffs() {
        let (simulation, playground, mut panels) = setup();
        effects_mode(&mut panels, &playground, &simulation);
        assert_eq!(panels.visible_effects(), StatusKind::ALL.len());
        let headings: Vec<_> = panels
            .effect_rows
            .iter()
            .filter_map(|row| match row {
                EffectRow::Heading(class) => Some(*class),
                EffectRow::Effect(_) => None,
            })
            .collect();
        assert_eq!(
            headings,
            [
                engine_game::status::EffectClass::Buff,
                engine_game::status::EffectClass::Debuff
            ]
        );
        // Every row under BUFFS is a buff, and under DEBUFFS a debuff.
        let mut harmful = false;
        for row in &panels.effect_rows {
            match row {
                EffectRow::Heading(class) => {
                    harmful = *class == engine_game::status::EffectClass::Debuff;
                }
                EffectRow::Effect(kind) => assert_eq!(kind.is_harmful(), harmful, "{kind:?}"),
            }
        }
    }

    #[test]
    fn choosing_an_effect_and_a_time_then_apply_asks_for_exactly_that() {
        let (simulation, playground, mut panels) = setup();
        let layout = effects_mode(&mut panels, &playground, &simulation);
        // Nothing chosen: APPLY does nothing.
        assert_eq!(
            press(&mut panels, centre(layout.spawn), &playground, &simulation),
            PlaygroundAction::None
        );
        panels.choose_effect(StatusKind::Fortitude);
        press(
            &mut panels,
            centre(layout.time_up),
            &playground,
            &simulation,
        );
        assert_eq!(panels.duration, 45.0, "the next stop up from 30");
        press(
            &mut panels,
            centre(layout.stacks_up),
            &playground,
            &simulation,
        );
        press(
            &mut panels,
            centre(layout.stacks_up),
            &playground,
            &simulation,
        );
        press(
            &mut panels,
            centre(layout.stacks_up),
            &playground,
            &simulation,
        );
        assert_eq!(panels.stacks, 3, "Fortitude stacks to three, no further");
        assert_eq!(
            press(&mut panels, centre(layout.spawn), &playground, &simulation),
            PlaygroundAction::ApplyEffect {
                kind: StatusKind::Fortitude,
                duration: 45.0,
                stacks: 3
            }
        );

        // Typing a time: click the value, type 7 and 5, Enter.
        press(
            &mut panels,
            centre(layout.time_value),
            &playground,
            &simulation,
        );
        assert!(panels.is_typing());
        for key in [KeyCode::Digit7, KeyCode::Digit5, KeyCode::Enter] {
            let input = UiInput {
                captured_key: Some(KeyChord::new(key, Modifiers::none())),
                ..UiInput::default()
            };
            panels.update(
                &input,
                0.0,
                PlaygroundKeys::default(),
                VIEWPORT,
                &playground,
                &simulation,
            );
        }
        assert!(!panels.is_typing());
        assert_eq!(panels.duration, 75.0);
    }

    #[test]
    fn the_inspector_lists_targets_and_a_click_points_the_effects_at_one() {
        let (mut simulation, mut playground, mut panels) = setup();
        playground.selection_mut().select(EnemyKind::CryptArcher);
        let origin = simulation.player_position();
        playground.spawn_selected(&mut simulation, origin, 0.0);
        let archer = playground.encounter().members()[0].entity;
        let layout = effects_mode(&mut panels, &playground, &simulation);
        let index = panels
            .items
            .iter()
            .position(|item| matches!(item, Item::Target(entry) if entry.entity == archer))
            .expect("the archer is a target");
        let rect = item_rect(&panels, &layout, index);
        assert_eq!(
            press(&mut panels, centre(rect), &playground, &simulation),
            PlaygroundAction::SetEffectTarget(archer)
        );
        let player = simulation.player().entity;
        assert!(
            panels
                .items
                .iter()
                .any(|item| matches!(item, Item::Target(entry) if entry.entity == player)),
            "the player is a target"
        );
    }

    #[test]
    fn buffs_and_debuffs_are_separate_lines_and_each_can_be_removed() {
        let (mut simulation, playground, mut panels) = setup();
        for kind in [
            StatusKind::Fortitude,
            StatusKind::Bloodlust,
            StatusKind::Poisoned,
            StatusKind::CurseOfWeakness,
        ] {
            playground.apply_effect(&mut simulation, kind, 30.0, 1);
        }
        let layout = effects_mode(&mut panels, &playground, &simulation);
        let position = |title: &str| {
            panels
                .items
                .iter()
                .position(|item| matches!(item, Item::Heading(name, _) if *name == title))
                .expect("a section")
        };
        let (buffs, debuffs) = (position("BUFFS"), position("DEBUFFS"));
        assert!(buffs < debuffs);
        let kinds_between = |from: usize, to: usize| -> Vec<StatusKind> {
            panels.items[from..to]
                .iter()
                .filter_map(|item| match item {
                    Item::Effect(Aura {
                        kind: AuraKind::Status(kind),
                        ..
                    }) => Some(*kind),
                    _ => None,
                })
                .collect()
        };
        let shown_buffs = kinds_between(buffs, debuffs);
        let shown_debuffs = kinds_between(debuffs, panels.items.len());
        assert!(shown_buffs.contains(&StatusKind::Fortitude));
        assert!(shown_buffs.contains(&StatusKind::Bloodlust));
        assert!(shown_debuffs.contains(&StatusKind::Poisoned));
        assert!(shown_debuffs.contains(&StatusKind::CurseOfWeakness));
        assert!(shown_buffs.iter().all(|kind| !kind.is_harmful()));
        assert!(shown_debuffs.iter().all(|kind| kind.is_harmful()));

        // The remove button on the poison's line asks for exactly that.
        let index = panels
            .items
            .iter()
            .position(|item| {
                matches!(
                    item,
                    Item::Effect(Aura {
                        kind: AuraKind::Status(StatusKind::Poisoned),
                        ..
                    })
                )
            })
            .expect("the poison's line");
        // Scrolled into view first, as a tester would.
        panels.encounter_scroll = 0.0;
        let offset = item_rect(&panels, &layout, index).y - layout.encounter.y;
        let most = (panels.inspector_height(&layout) - layout.encounter.height).max(0.0);
        panels.encounter_scroll = offset.min(most);
        let line = item_rect(&panels, &layout, index);
        let remove = panels.remove_rect(line);
        assert_eq!(
            press(&mut panels, centre(remove), &playground, &simulation),
            PlaygroundAction::RemoveEffect(StatusKind::Poisoned)
        );

        // And the stats show what the buffs did.
        let mut list = UiDrawList::new();
        panels.push(&mut list, VIEWPORT, &playground, &simulation);
        let text = list.text();
        for word in [
            "BUFFS",
            "DEBUFFS",
            "Fortitude",
            "Bloodlust",
            "Poisoned",
            "STATS",
            "(+20)",
        ] {
            assert!(text.contains(word), "{word} is not drawn");
        }
    }

    #[test]
    fn the_effects_buttons_and_the_hero_tabs_ask_for_what_they_say() {
        let (simulation, playground, mut panels) = setup();
        let layout = effects_mode(&mut panels, &playground, &simulation);
        assert_eq!(
            press(
                &mut panels,
                centre(layout.test_effects),
                &playground,
                &simulation
            ),
            PlaygroundAction::AddTestEffects
        );
        assert_eq!(
            press(
                &mut panels,
                centre(layout.clear_buffs),
                &playground,
                &simulation
            ),
            PlaygroundAction::ClearEffects { buffs: true }
        );
        assert_eq!(
            press(
                &mut panels,
                centre(layout.clear_debuffs),
                &playground,
                &simulation
            ),
            PlaygroundAction::ClearEffects { buffs: false }
        );
        assert_eq!(
            press(
                &mut panels,
                centre(layout.heroes[2]),
                &playground,
                &simulation
            ),
            PlaygroundAction::SetHero(Hero::Druid)
        );
        // Back to the monsters, where the same row is Kill all again.
        press(
            &mut panels,
            centre(layout.tab_monsters),
            &playground,
            &simulation,
        );
        assert_eq!(panels.mode(), PlaygroundMode::Monsters);
    }

    #[test]
    fn every_effect_at_once_still_draws_and_scrolls() {
        let (mut simulation, playground, mut panels) = setup();
        let applied = playground.add_test_effects(&mut simulation);
        let layout = effects_mode(&mut panels, &playground, &simulation);
        let lines = panels
            .items
            .iter()
            .filter(|item| matches!(item, Item::Effect(_)))
            .count();
        assert!(
            lines >= applied,
            "every effect has a line: {lines} of {applied}"
        );
        assert!(
            panels
                .items
                .iter()
                .any(|item| matches!(item, Item::Note(note) if note.contains("limit"))),
            "the HUD's limit is called out"
        );
        assert!(
            panels.inspector_height(&layout) > layout.encounter.height,
            "more than fits, so it scrolls"
        );
        // A wheel over the inspector scrolls it, and only as far as there is.
        for _ in 0..200 {
            panels.update(
                &UiInput::default()
                    .with_pointer(layout.encounter.x + 10.0, layout.encounter.y + 10.0),
                -1.0,
                PlaygroundKeys::default(),
                VIEWPORT,
                &playground,
                &simulation,
            );
        }
        let most = panels.inspector_height(&layout) - layout.encounter.height;
        assert!((panels.encounter_scroll - most).abs() < 1.0);
        let mut list = UiDrawList::new();
        panels.push(&mut list, VIEWPORT, &playground, &simulation);
        assert!(list.text().contains("Withering Plague"));
    }

    #[test]
    fn thousands_are_grouped() {
        let mut out = String::new();
        push_thousands(&mut out, 1_000_000.0);
        assert_eq!(out, "1,000,000");
        out.clear();
        push_thousands(&mut out, 950.0);
        assert_eq!(out, "950");
        out.clear();
        push_thousands(&mut out, 16_000.4);
        assert_eq!(out, "16,000");
    }
}
