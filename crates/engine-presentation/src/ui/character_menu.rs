//! The character sheet: stats down one side, fifteen equipment slots around the character,
//! and the bag underneath.
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │ CHARACTER                                    FIREMAGE · LV --  │
//! ├────────────┬────────────────────────────────┬─────────────────┤
//! │ PRIMARY    │ [head]                  [back] │ DERIVED         │
//! │  INTELLECT │ [shld]                  [neck] │  HEALTH         │
//! │  STAMINA   │ [chst]   the character   [wrst]│  SPELL POWER    │
//! │            │ [hand]                  [feet] │  CRIT CHANCE    │
//! │ SECONDARY  │ [legs]                         │  GLOBAL COOLDOWN│
//! │  CRIT ...  │        [ring][ring][trinket…]  │                 │
//! ├────────────┴────────────────────────────────┴─────────────────┤
//! │ INVENTORY    ▣ ▣ ▣ ▢ ▢ ▢ ▢ ▢                                  │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! # The menu owns no items
//!
//! Like [`super::talent_menu`], this reads the frame's [`UiSnapshot`] and emits a request.
//! Dragging a ring onto a finger produces [`CharacterMenuOutcome::Move`]; the application
//! turns it into a [`engine_game::PlayerCommand::MoveItem`] and the simulation decides. So
//! the same drag is reproducible in a replay, and a drop the interface allowed but gameplay
//! refuses simply does not happen — the panel is drawn from the next frame's snapshot, which
//! is the sheet as it actually is.
//!
//! The one thing the menu does keep is the drag in progress: where the item was picked up
//! from, and whether the last drop was refused. Both are questions about the *pointer*,
//! which gameplay has no opinion about.
//!
//! # Every slot is the same slot
//!
//! An equipment slot and a bag cell are both an [`ItemSlot`], hit-tested by
//! [`CharacterMenu::slot_at`] and drawn by the same function. There is no equip path, no
//! unequip path and no rearrange path — there is one drag, from a slot to a slot, and which
//! of the three it turns out to be is a consequence of the two addresses.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};

use engine_game::character::equipment::{INVENTORY_COLUMNS, INVENTORY_SLOTS};
use engine_game::character::stat::StatGroup;
use engine_game::character::{
    EquipSlot, ItemIcon, ItemQuality, ItemSlot, ItemStack, Stat, StatBlock,
};
use engine_game::hero::Hero;

use super::input::UiInput;
use crate::snapshot::{CharacterView, UiSnapshot};

/// Layout of the panel, in design pixels.
///
/// One `mod` of plain numbers, converted to physical pixels once by [`CharacterMetrics`],
/// exactly as [`super::talent_menu`] and the HUD do. Nothing below reads a logical number.
///
/// The whole panel fits inside 1280x720 at an interface scale of one, which is the smallest
/// window the game runs in. A test asserts it, because a sheet that is taller than the screen
/// is a sheet whose bag cannot be reached — and the interface scale slider only makes it
/// bigger from here.
mod layout {
    /// Width of the whole panel.
    pub const PANEL_WIDTH: f32 = 1120.0;
    /// Height of the whole panel.
    pub const PANEL_HEIGHT: f32 = 704.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 18.0;
    /// Height of the heading strip.
    pub const HEADER: f32 = 52.0;
    /// Edge of one equipment or inventory slot.
    pub const SLOT: f32 = 48.0;
    /// Gap between two equipment slots in a column.
    pub const SLOT_GAP: f32 = 10.0;
    /// Gap between two inventory cells.
    pub const CELL_GAP: f32 = 8.0;
    /// Width of the stats column on the left.
    pub const STATS_WIDTH: f32 = 250.0;
    /// Width of the derived column on the right.
    pub const DERIVED_WIDTH: f32 = 250.0;
    /// Gap between the three columns.
    pub const COLUMN_GAP: f32 = 20.0;
    /// Width of the middle column: the character and the gear around them.
    pub const GEAR_WIDTH: f32 = 520.0;
    /// How many equipment slots stand in the taller of the two columns.
    pub const GEAR_ROWS: usize = 5;
    /// Height of the section heading above a group of stats or the bag.
    pub const SECTION: f32 = 24.0;
    /// Height of one row of the stat list.
    pub const STAT_ROW: f32 = 20.0;
    /// Gap between the gear block and the row of rings and trinkets.
    pub const JEWELRY_GAP: f32 = 12.0;
    /// Gap between the row of rings and trinkets and the bag heading.
    pub const BAG_GAP: f32 = 40.0;
    /// Width of a tooltip.
    pub const TOOLTIP_WIDTH: f32 = 290.0;
    /// Height of one line inside a tooltip.
    pub const TOOLTIP_LINE: f32 = 16.0;
}

/// The five slots down the left of the character, in drawn order.
const LEFT_COLUMN: [EquipSlot; 5] = [
    EquipSlot::Head,
    EquipSlot::Shoulder,
    EquipSlot::Chest,
    EquipSlot::Hands,
    EquipSlot::Legs,
];

/// The four slots down the right of the character, in drawn order.
const RIGHT_COLUMN: [EquipSlot; 4] = [
    EquipSlot::Back,
    EquipSlot::Necklace,
    EquipSlot::Wrist,
    EquipSlot::Feet,
];

/// The six slots under the character: both rings, then all four trinkets.
const JEWELRY_ROW: [EquipSlot; 6] = [
    EquipSlot::Ring1,
    EquipSlot::Ring2,
    EquipSlot::Trinket1,
    EquipSlot::Trinket2,
    EquipSlot::Trinket3,
    EquipSlot::Trinket4,
];

/// Every character-panel measurement, converted from its design size to physical pixels once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterMetrics {
    /// The scale these were derived with.
    pub scale: UiScale,
    /// `layout::PANEL_WIDTH` in physical pixels.
    pub panel_width: f32,
    /// `layout::PANEL_HEIGHT` in physical pixels.
    pub panel_height: f32,
    /// `layout::PADDING` in physical pixels.
    pub padding: f32,
    /// `layout::HEADER` in physical pixels.
    pub header: f32,
    /// `layout::SLOT` in physical pixels.
    pub slot: f32,
    /// `layout::SLOT_GAP` in physical pixels.
    pub slot_gap: f32,
    /// `layout::CELL_GAP` in physical pixels.
    pub cell_gap: f32,
    /// `layout::STATS_WIDTH` in physical pixels.
    pub stats_width: f32,
    /// `layout::DERIVED_WIDTH` in physical pixels.
    pub derived_width: f32,
    /// `layout::COLUMN_GAP` in physical pixels.
    pub column_gap: f32,
    /// `layout::GEAR_WIDTH` in physical pixels.
    pub gear_width: f32,
    /// `layout::SECTION` in physical pixels.
    pub section: f32,
    /// `layout::STAT_ROW` in physical pixels.
    pub stat_row: f32,
    /// `layout::JEWELRY_GAP` in physical pixels.
    pub jewelry_gap: f32,
    /// `layout::BAG_GAP` in physical pixels.
    pub bag_gap: f32,
    /// `layout::TOOLTIP_WIDTH` in physical pixels.
    pub tooltip_width: f32,
    /// `layout::TOOLTIP_LINE` in physical pixels.
    pub tooltip_line: f32,
}

impl CharacterMetrics {
    /// The panel's measurements at `scale`.
    #[must_use]
    pub fn new(scale: UiScale) -> Self {
        Self {
            scale,
            panel_width: scale.px(layout::PANEL_WIDTH),
            panel_height: scale.px(layout::PANEL_HEIGHT),
            padding: scale.px(layout::PADDING),
            header: scale.px(layout::HEADER),
            slot: scale.px(layout::SLOT),
            slot_gap: scale.px(layout::SLOT_GAP),
            cell_gap: scale.px(layout::CELL_GAP),
            stats_width: scale.px(layout::STATS_WIDTH),
            derived_width: scale.px(layout::DERIVED_WIDTH),
            column_gap: scale.px(layout::COLUMN_GAP),
            gear_width: scale.px(layout::GEAR_WIDTH),
            section: scale.px(layout::SECTION),
            stat_row: scale.px(layout::STAT_ROW),
            jewelry_gap: scale.px(layout::JEWELRY_GAP),
            bag_gap: scale.px(layout::BAG_GAP),
            tooltip_width: scale.px(layout::TOOLTIP_WIDTH),
            tooltip_line: scale.px(layout::TOOLTIP_LINE),
        }
    }

    /// Distance from one equipment slot to the next down a column.
    #[must_use]
    pub fn slot_pitch(&self) -> f32 {
        self.slot + self.slot_gap
    }

    /// Distance from one inventory cell to the next across the grid.
    #[must_use]
    pub fn cell_pitch(&self) -> f32 {
        self.slot + self.cell_gap
    }
}

impl Default for CharacterMetrics {
    fn default() -> Self {
        Self::new(UiScale::default())
    }
}

/// What the panel decided this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CharacterMenuOutcome {
    /// Nothing happened.
    Idle,
    /// The player dropped an item somewhere it may go.
    ///
    /// A *request*. The simulation applies it, and the panel redraws from the result.
    Move {
        /// Where the item was picked up.
        from: ItemSlot,
        /// Where it was dropped.
        to: ItemSlot,
    },
    /// The panel was closed.
    Closed,
}

/// An item in the air: where it came from, and where the pointer took hold of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drag {
    /// The slot the item is still in as far as gameplay is concerned.
    pub from: ItemSlot,
    /// What is being dragged, so the icon can be drawn under the pointer.
    pub stack: ItemStack,
    /// Where inside the slot the pointer took hold, so the icon does not jump on pick-up.
    pub grab: (f32, f32),
    /// Where the pointer is now, updated every frame the item is in the air.
    ///
    /// Carried on the drag rather than left to the painter to work out from the slot under
    /// the cursor: an item being dragged across the gap between two slots is over no slot at
    /// all, and an icon that snapped back to its origin every time the pointer crossed a gap
    /// would not look like it was being carried.
    pub pointer: (f32, f32),
}

impl Drag {
    /// Where the dragged icon's top-left corner belongs, for a slot of `size` pixels.
    ///
    /// The pointer, less where inside the slot it took hold, so the icon sits under the
    /// cursor exactly where it was picked up rather than jumping its own width on the first
    /// frame.
    #[must_use]
    pub fn icon_origin(&self) -> (f32, f32) {
        (self.pointer.0 - self.grab.0, self.pointer.1 - self.grab.1)
    }
}

/// The character panel: whether it is open, what the pointer is over, and what it is holding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterMenu {
    open: bool,
    hovered: Option<ItemSlot>,
    drag: Option<Drag>,
    refused: Option<ItemSlot>,
    metrics: CharacterMetrics,
}

impl Default for CharacterMenu {
    fn default() -> Self {
        Self::new()
    }
}

impl CharacterMenu {
    /// Closed.
    #[must_use]
    pub fn new() -> Self {
        Self::with_scale(UiScale::default())
    }

    /// Closed, laid out at `scale`.
    #[must_use]
    pub fn with_scale(scale: UiScale) -> Self {
        Self {
            open: false,
            hovered: None,
            drag: None,
            refused: None,
            metrics: CharacterMetrics::new(scale),
        }
    }

    /// Lays the panel out at a new scale from here on.
    pub fn set_scale(&mut self, scale: UiScale) {
        self.metrics = CharacterMetrics::new(scale);
    }

    /// The scale the panel is laid out at.
    #[must_use]
    pub fn scale(&self) -> UiScale {
        self.metrics.scale
    }

    /// The panel's measurements in physical pixels.
    #[must_use]
    pub fn metrics(&self) -> &CharacterMetrics {
        &self.metrics
    }

    /// Whether the panel is on screen.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Which slot the pointer is over, if any. What the tooltip is drawn for.
    #[must_use]
    pub fn hovered(&self) -> Option<ItemSlot> {
        self.hovered
    }

    /// The item currently in the air, if one is.
    #[must_use]
    pub fn drag(&self) -> Option<Drag> {
        self.drag
    }

    /// The slot the last drop was refused on, if the pointer has not moved on yet.
    ///
    /// What the painter outlines in red. Cleared by the next pick-up, so a refusal is
    /// answered by trying again rather than by a timer.
    #[must_use]
    pub fn refused(&self) -> Option<ItemSlot> {
        self.refused
    }

    /// Opens it.
    pub fn open(&mut self) {
        self.open = true;
        self.hovered = None;
        self.drag = None;
        self.refused = None;
    }

    /// Closes it, dropping anything in the air back where it came from.
    ///
    /// Nothing has to be put back: the item never left the sheet. That is the whole benefit
    /// of the drag being a pointer state rather than a removal.
    pub fn close(&mut self) {
        self.open = false;
        self.hovered = None;
        self.drag = None;
        self.refused = None;
    }

    /// Opens it if it is closed, closes it if it is open.
    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    /// Reads the frame's pointer and reports what the player asked for.
    ///
    /// The order is the order of a drag: pick up on the press, follow while held, resolve on
    /// the release. A release outside every slot is a cancel and not a refusal — the player
    /// let go over the panel's background, which is how a drag is called off.
    pub fn update(
        &mut self,
        input: &UiInput,
        snapshot: &UiSnapshot,
        viewport: (u32, u32),
    ) -> CharacterMenuOutcome {
        if !self.open {
            return CharacterMenuOutcome::Idle;
        }
        if input.cancel {
            self.close();
            return CharacterMenuOutcome::Closed;
        }

        let (x, y) = input.pointer;
        let over = self.slot_at(x, y, viewport);
        self.hovered = over;
        if let Some(drag) = &mut self.drag {
            drag.pointer = (x, y);
        }
        let character = &snapshot.character;

        // Picking up: a press on a slot with something in it.
        if input.pointer_pressed {
            self.refused = None;
            if let Some(slot) = over
                && let Some(stack) = character.at(slot)
            {
                let rect = self.slot_rect(slot, viewport);
                self.drag = Some(Drag {
                    from: slot,
                    stack,
                    grab: (x - rect.x, y - rect.y),
                    pointer: (x, y),
                });
                return CharacterMenuOutcome::Idle;
            }
            // A press on the panel's background is nothing; a press outside it closes.
            if !self.panel_rect(viewport).contains(x, y) {
                self.close();
                return CharacterMenuOutcome::Closed;
            }
        }

        // Dropping.
        if input.pointer_released
            && let Some(drag) = self.drag.take()
        {
            let Some(target) = over else {
                // Let go over nothing: the drag is called off, and nothing was refused.
                return CharacterMenuOutcome::Idle;
            };
            if target == drag.from {
                return CharacterMenuOutcome::Idle;
            }
            if !character.can_move(drag.from, target) {
                self.refused = Some(target);
                return CharacterMenuOutcome::Idle;
            }
            return CharacterMenuOutcome::Move {
                from: drag.from,
                to: target,
            };
        }

        // The button was let go somewhere this frame did not see, or was never down: an
        // item cannot be left in the air across a frame in which nothing is held.
        if !input.pointer_down {
            self.drag = None;
        }

        CharacterMenuOutcome::Idle
    }

    /// The panel the sheet draws in, centred on the viewport.
    #[must_use]
    pub fn panel_rect(&self, viewport: (u32, u32)) -> UiRect {
        let (width, height) = (viewport.0 as f32, viewport.1 as f32);
        UiRect::new(
            ((width - self.metrics.panel_width) * 0.5).max(0.0),
            ((height - self.metrics.panel_height) * 0.5).max(0.0),
            self.metrics.panel_width,
            self.metrics.panel_height,
        )
    }

    /// The left-hand column: the primary and secondary stat lists.
    #[must_use]
    pub fn stats_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        UiRect::new(
            panel.x + self.metrics.padding,
            panel.y + self.metrics.header,
            self.metrics.stats_width,
            panel.bottom() - self.metrics.padding - (panel.y + self.metrics.header),
        )
    }

    /// The right-hand column: the numbers the stats produce.
    #[must_use]
    pub fn derived_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        UiRect::new(
            panel.right() - self.metrics.padding - self.metrics.derived_width,
            panel.y + self.metrics.header,
            self.metrics.derived_width,
            panel.bottom() - self.metrics.padding - (panel.y + self.metrics.header),
        )
    }

    /// The middle column: the character, with the gear around them.
    #[must_use]
    pub fn gear_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        UiRect::new(
            panel.x + self.metrics.padding + self.metrics.stats_width + self.metrics.column_gap,
            panel.y + self.metrics.header,
            self.metrics.gear_width,
            f32::from(u8::try_from(layout::GEAR_ROWS).unwrap_or(5)) * self.metrics.slot_pitch(),
        )
    }

    /// Where the character themselves is drawn: between the two columns of gear.
    #[must_use]
    pub fn portrait_rect(&self, viewport: (u32, u32)) -> UiRect {
        let gear = self.gear_rect(viewport);
        let inset = self.metrics.slot + self.metrics.slot_gap;
        UiRect::new(
            gear.x + inset,
            gear.y,
            (gear.width - inset * 2.0).max(0.0),
            gear.height,
        )
    }

    /// Where the equipment slot for `slot` sits.
    #[must_use]
    pub fn equipment_rect(&self, slot: EquipSlot, viewport: (u32, u32)) -> UiRect {
        let gear = self.gear_rect(viewport);
        let pitch = self.metrics.slot_pitch();

        if let Some(row) = LEFT_COLUMN.iter().position(|entry| *entry == slot) {
            return UiRect::new(
                gear.x,
                gear.y + row as f32 * pitch,
                self.metrics.slot,
                self.metrics.slot,
            );
        }
        if let Some(row) = RIGHT_COLUMN.iter().position(|entry| *entry == slot) {
            return UiRect::new(
                gear.right() - self.metrics.slot,
                gear.y + row as f32 * pitch,
                self.metrics.slot,
                self.metrics.slot,
            );
        }
        let column = JEWELRY_ROW
            .iter()
            .position(|entry| *entry == slot)
            .unwrap_or(0);
        let row_width = JEWELRY_ROW.len() as f32 * pitch - self.metrics.slot_gap;
        let start = gear.x + (gear.width - row_width) * 0.5;
        UiRect::new(
            start + column as f32 * pitch,
            gear.bottom() + self.metrics.jewelry_gap,
            self.metrics.slot,
            self.metrics.slot,
        )
    }

    /// The bag: the heading strip and the grid under it.
    #[must_use]
    pub fn inventory_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        let jewelry = self.equipment_rect(EquipSlot::Ring1, viewport);
        let pitch = self.metrics.cell_pitch();
        let rows = INVENTORY_SLOTS.div_ceil(INVENTORY_COLUMNS);
        let width = INVENTORY_COLUMNS as f32 * pitch - self.metrics.cell_gap;
        let height = rows as f32 * pitch - self.metrics.cell_gap;
        UiRect::new(
            panel.x + (panel.width - width) * 0.5,
            jewelry.bottom() + self.metrics.bag_gap,
            width,
            height,
        )
    }

    /// Where the bag cell at `index` sits.
    #[must_use]
    pub fn cell_rect(&self, index: usize, viewport: (u32, u32)) -> UiRect {
        let grid = self.inventory_rect(viewport);
        let pitch = self.metrics.cell_pitch();
        let column = index % INVENTORY_COLUMNS;
        let row = index / INVENTORY_COLUMNS;
        UiRect::new(
            grid.x + column as f32 * pitch,
            grid.y + row as f32 * pitch,
            self.metrics.slot,
            self.metrics.slot,
        )
    }

    /// Where any slot sits, worn or carried.
    ///
    /// The one function the painter and the hit test both use, so what is drawn and what is
    /// clicked cannot drift apart.
    #[must_use]
    pub fn slot_rect(&self, slot: ItemSlot, viewport: (u32, u32)) -> UiRect {
        match slot {
            ItemSlot::Equipment(worn) => self.equipment_rect(worn, viewport),
            ItemSlot::Inventory(index) => self.cell_rect(index, viewport),
        }
    }

    /// Which slot the point `(x, y)` is inside, if any.
    #[must_use]
    pub fn slot_at(&self, x: f32, y: f32, viewport: (u32, u32)) -> Option<ItemSlot> {
        for worn in EquipSlot::ALL {
            if self.equipment_rect(worn, viewport).contains(x, y) {
                return Some(ItemSlot::Equipment(worn));
            }
        }
        for index in 0..INVENTORY_SLOTS {
            if self.cell_rect(index, viewport).contains(x, y) {
                return Some(ItemSlot::Inventory(index));
            }
        }
        None
    }
}

/// How the character panel is coloured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterTheme {
    /// The wash over the game behind the panel.
    pub scrim: UiColor,
    /// The panel.
    pub panel: UiColor,
    /// The panel's border.
    pub border: UiColor,
    /// A section of the panel: the stat columns, the portrait, the bag.
    pub section: UiColor,
    /// The line around a section.
    pub section_border: UiColor,
    /// An empty slot.
    pub slot: UiColor,
    /// A slot with something in it.
    pub slot_filled: UiColor,
    /// The outline on the slot under the pointer.
    pub hover: UiColor,
    /// The outline on a slot an item in the air may be dropped on.
    pub valid: UiColor,
    /// The outline on a slot it may not.
    pub invalid: UiColor,
    /// Ordinary text.
    pub text: UiColor,
    /// Dimmed text: labels, a stat at zero, a slot that is empty.
    pub text_dim: UiColor,
    /// The heading.
    pub title: UiColor,
    /// The main stat, which is printed larger than the rest.
    pub main_stat: UiColor,
    /// The tooltip's background.
    pub tooltip: UiColor,
    /// The character themselves.
    pub portrait: UiColor,
    /// The shaded side of the character.
    pub portrait_shade: UiColor,
    /// The ground the character stands on.
    pub portrait_floor: UiColor,
}

impl Default for CharacterTheme {
    /// The same dark slate the talent menu and the pause menu already use, so the character
    /// sheet reads as another window of the same interface rather than as a new one.
    fn default() -> Self {
        Self {
            scrim: [0.02, 0.03, 0.05, 0.80],
            panel: [0.07, 0.08, 0.11, 0.98],
            border: [0.42, 0.50, 0.64, 1.0],
            section: [0.10, 0.11, 0.15, 0.96],
            section_border: [0.22, 0.26, 0.34, 1.0],
            slot: [0.13, 0.14, 0.18, 1.0],
            slot_filled: [0.17, 0.19, 0.24, 1.0],
            hover: [0.98, 0.84, 0.35, 1.0],
            valid: [0.36, 0.82, 0.44, 1.0],
            invalid: [0.88, 0.28, 0.24, 1.0],
            text: [0.92, 0.94, 0.98, 1.0],
            text_dim: [0.52, 0.55, 0.62, 1.0],
            title: [0.98, 0.78, 0.35, 1.0],
            main_stat: [0.99, 0.90, 0.62, 1.0],
            tooltip: [0.04, 0.05, 0.07, 0.98],
            portrait: [0.30, 0.34, 0.42, 1.0],
            portrait_shade: [0.20, 0.23, 0.29, 1.0],
            portrait_floor: [0.14, 0.15, 0.19, 1.0],
        }
    }
}

/// The colour an item of `quality` writes its name in.
///
/// The genre's palette, and the one place it is stated. Gameplay carries the rank; this
/// turns a rank into a colour.
#[must_use]
pub fn quality_color(quality: ItemQuality) -> UiColor {
    match quality {
        ItemQuality::Poor => [0.62, 0.62, 0.62, 1.0],
        ItemQuality::Common => [1.00, 1.00, 1.00, 1.0],
        ItemQuality::Uncommon => [0.12, 1.00, 0.00, 1.0],
        ItemQuality::Rare => [0.00, 0.44, 0.87, 1.0],
        ItemQuality::Epic => [0.64, 0.21, 0.93, 1.0],
        ItemQuality::Legendary => [1.00, 0.50, 0.00, 1.0],
    }
}

/// The picture gameplay asked for, as an icon this renderer has.
///
/// The seam: `engine-game` names [`ItemIcon::Helm`] and never learns there is a texture.
#[must_use]
pub fn icon_for_item(icon: ItemIcon) -> UiIcon {
    match icon {
        ItemIcon::Helm => UiIcon::Helm,
        ItemIcon::Shoulders => UiIcon::Shoulders,
        ItemIcon::Chestpiece => UiIcon::Chestpiece,
        ItemIcon::Cloak => UiIcon::Cloak,
        ItemIcon::Gloves => UiIcon::Gloves,
        ItemIcon::Leggings => UiIcon::Leggings,
        ItemIcon::Boots => UiIcon::Boots,
        ItemIcon::Bracers => UiIcon::Bracers,
        ItemIcon::Amulet => UiIcon::Amulet,
        ItemIcon::Ring => UiIcon::Ring,
        ItemIcon::Trinket => UiIcon::Trinket,
    }
}

/// The colour a hero's silhouette is tinted.
#[must_use]
fn hero_tint(hero: Hero) -> UiColor {
    match hero {
        Hero::FireMage => [0.86, 0.42, 0.20, 1.0],
        Hero::Warrior => [0.72, 0.68, 0.62, 1.0],
        Hero::Druid => [0.38, 0.70, 0.44, 1.0],
    }
}

/// Draws the character sheet, if it is open.
///
/// `scratch` is the caller's, reused: the panel formats a dozen numbers every frame it is up
/// and formatting them would otherwise allocate once a frame each.
pub fn push_character_menu(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    snapshot: &UiSnapshot,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    if !menu.is_open() {
        return;
    }
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let panel = menu.panel_rect(viewport);
    let character = &snapshot.character;

    list.rect(
        UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
        theme.scrim,
    );
    list.rect(panel, theme.panel);
    list.outline(panel, scale.px(2.0), theme.border);
    // A second line just inside the first: the decorative double frame the genre's panels
    // have, made of the two primitives this renderer has rather than of a border texture.
    list.outline(
        panel.inset(scale.px(5.0)),
        scale.px(1.0),
        theme.section_border,
    );

    push_header(list, menu, character, theme, scratch, viewport);
    push_stats_column(list, menu, character, theme, scratch, viewport);
    push_derived_column(list, menu, snapshot, theme, scratch, viewport);
    push_portrait(list, menu, character, theme, viewport);
    push_slots(list, menu, character, theme, scratch, viewport);
    push_inventory(list, menu, character, theme, scratch, viewport);
    push_footer(list, menu, theme, viewport);

    // Above everything: what the pointer is holding, then what it is over.
    push_dragged(list, menu, theme);
    push_tooltip(list, menu, character, theme, scratch, viewport);
}

/// The heading: what this window is, and who it is about.
fn push_header(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let panel = menu.panel_rect(viewport);

    list.text_at(
        (panel.x + metrics.padding, panel.y + scale.px(16.0)),
        "CHARACTER",
        theme.title,
        scale.text(3.0),
    );

    // Name and class on one line, right-aligned: the hero is both here.
    scratch.clear();
    scratch.push_str(character.hero.name());
    scratch.push_str("  ·  ");
    scratch.push_str(character.hero.description());
    list.text_aligned(
        (panel.right() - metrics.padding, panel.y + scale.px(22.0)),
        scratch,
        theme.text,
        scale.text(1.0),
        TextAlign::Right,
    );

    let rule = UiRect::new(
        panel.x + metrics.padding,
        panel.y + metrics.header - scale.px(8.0),
        panel.width - metrics.padding * 2.0,
        scale.px(1.0),
    );
    list.rect(rule, theme.section_border);
}

/// The left column: the primary stats, then the secondary ones.
fn push_stats_column(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let column = menu.stats_rect(viewport);
    list.rect(column, theme.section);
    list.outline(column, scale.px(1.0), theme.section_border);

    let mut y = column.y + scale.px(10.0);

    // Primary. The main stat first and larger: it is the number the character is built on.
    y = push_section_heading(list, column, y, "PRIMARY", theme, metrics);
    push_main_stat(list, column, y, character, theme, scratch, metrics);
    y += metrics.stat_row * 1.6;
    push_stat_row(
        list,
        stat_row_rect(column, y, metrics),
        Stat::Stamina,
        &character.total,
        theme,
        scratch,
        metrics,
    );
    y += metrics.stat_row;

    // Secondary. All four, in table order, whatever they read.
    y += metrics.section * 0.5;
    y = push_section_heading(list, column, y, "SECONDARY", theme, metrics);
    for stat in Stat::ALL {
        if stat.group() != StatGroup::Secondary {
            continue;
        }
        push_stat_row(
            list,
            stat_row_rect(column, y, metrics),
            stat,
            &character.total,
            theme,
            scratch,
            metrics,
        );
        y += metrics.stat_row;
    }

    // Defensive. Armour, Block, Parry and Dodge: what an incoming attack meets. Its own
    // section rather than four more Secondary rows, because it is read as a group and
    // because Armour is printed flat while the other three read as percentages — see
    // `Stat::is_rating`.
    y += metrics.section * 0.5;
    y = push_section_heading(list, column, y, "DEFENSIVE", theme, metrics);
    for stat in Stat::ALL {
        if stat.group() != StatGroup::Defensive {
            continue;
        }
        push_stat_row(
            list,
            stat_row_rect(column, y, metrics),
            stat,
            &character.total,
            theme,
            scratch,
            metrics,
        );
        y += metrics.stat_row;
    }

    // What the gear is contributing, as one line. The panel does not add this up; it prints
    // the difference the sheet already reports between base and total.
    y += metrics.section * 0.5;
    y = push_section_heading(list, column, y, "FROM EQUIPMENT", theme, metrics);
    if character.gear.is_empty() {
        list.text_at(
            (column.x + scale.px(10.0), y),
            "NO COMBAT STATS",
            theme.text_dim,
            scale.text(1.0),
        );
    } else {
        for (stat, value) in character.gear.entries() {
            scratch.clear();
            scratch.push('+');
            push_number(scratch, value);
            scratch.push(' ');
            scratch.push_str(stat.name());
            list.text_at(
                (column.x + scale.px(10.0), y),
                scratch,
                theme.text,
                scale.text(1.0),
            );
            y += metrics.stat_row;
        }
    }
}

/// The right column: the numbers the stats produce.
fn push_derived_column(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    snapshot: &UiSnapshot,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let column = menu.derived_rect(viewport);
    let character = &snapshot.character;
    list.rect(column, theme.section);
    list.outline(column, scale.px(1.0), theme.section_border);

    let mut y = column.y + scale.px(10.0);
    y = push_section_heading(list, column, y, "DERIVED", theme, metrics);

    // Health, which is Stamina, and the numbers the class already had. Nothing invented:
    // every row here is a value some system reads, printed where a player can see it.
    scratch.clear();
    push_number(scratch, character.max_health);
    push_derived_row(list, column, y, "HEALTH", scratch, theme, metrics);
    y += metrics.stat_row;

    scratch.clear();
    push_number(scratch, snapshot.spell_power);
    push_derived_row(list, column, y, "SPELL POWER", scratch, theme, metrics);
    y += metrics.stat_row;

    scratch.clear();
    push_percent(scratch, snapshot.crit_chance);
    push_derived_row(list, column, y, "CRIT CHANCE", scratch, theme, metrics);
    y += metrics.stat_row;

    scratch.clear();
    push_percent(scratch, snapshot.haste);
    push_derived_row(list, column, y, "HASTE", scratch, theme, metrics);
    y += metrics.stat_row;

    scratch.clear();
    push_seconds(
        scratch,
        engine_game::firemage::GLOBAL_COOLDOWN / (1.0 + snapshot.haste),
    );
    push_derived_row(list, column, y, "GLOBAL COOLDOWN", scratch, theme, metrics);
    y += metrics.stat_row;

    // The Spirit buff, when it is running. It cannot be, on the current roster, and the row
    // is here so that the moment an item grants Spirit the sheet already reports it.
    if character.spirit_stacks > 0 {
        y += metrics.section * 0.5;
        y = push_section_heading(list, column, y, "BUFFS", theme, metrics);
        scratch.clear();
        scratch.push_str("SPIRIT ");
        push_integer(scratch, u32::from(character.spirit_stacks));
        push_derived_row(
            list,
            column,
            y,
            scratch.clone().as_str(),
            "",
            theme,
            metrics,
        );
    }
}

/// The character themselves, standing between their gear.
///
/// A silhouette built from the two primitives this renderer has, tinted by class. It is a
/// *placeholder for the model*, and deliberately a readable one: a panel with a hole where
/// the character should be reads as broken, and rendering the actual skinned model into a
/// texture is a renderer feature rather than a layout one.
fn push_portrait(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    theme: &CharacterTheme,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let frame = menu.portrait_rect(viewport);
    list.rect(frame, theme.section);
    list.outline(frame, scale.px(1.0), theme.section_border);

    // The ground, so the figure is standing rather than floating.
    let floor_height = frame.height * 0.06;
    list.rect(
        UiRect::new(
            frame.x + frame.width * 0.18,
            frame.bottom() - floor_height * 2.0,
            frame.width * 0.64,
            floor_height,
        ),
        theme.portrait_floor,
    );

    let tint = hero_tint(character.hero);
    let centre = frame.x + frame.width * 0.5;
    let unit = frame.height * 0.01;
    let top = frame.y + frame.height * 0.10;

    // Head, torso, arms, legs — the proportions of the shared humanoid rig, at a tenth of
    // its height per unit.
    let head = unit * 9.0;
    list.rect(
        UiRect::new(centre - head * 0.5, top, head, head),
        theme.portrait,
    );
    let torso_w = unit * 20.0;
    let torso_h = unit * 26.0;
    let torso_y = top + head + unit * 2.0;
    list.rect(
        UiRect::new(centre - torso_w * 0.5, torso_y, torso_w, torso_h),
        theme.portrait,
    );
    // A shoulder band in the class colour: the one place the figure is not neutral.
    list.rect(
        UiRect::new(centre - torso_w * 0.62, torso_y, torso_w * 1.24, unit * 5.0),
        tint,
    );

    let arm_w = unit * 5.0;
    let arm_h = unit * 24.0;
    list.rect(
        UiRect::new(
            centre - torso_w * 0.5 - arm_w,
            torso_y + unit * 3.0,
            arm_w,
            arm_h,
        ),
        theme.portrait_shade,
    );
    list.rect(
        UiRect::new(centre + torso_w * 0.5, torso_y + unit * 3.0, arm_w, arm_h),
        theme.portrait_shade,
    );

    let leg_w = unit * 8.0;
    let leg_h = unit * 30.0;
    let leg_y = torso_y + torso_h;
    list.rect(
        UiRect::new(centre - leg_w - unit, leg_y, leg_w, leg_h),
        theme.portrait,
    );
    list.rect(
        UiRect::new(centre + unit, leg_y, leg_w, leg_h),
        theme.portrait_shade,
    );

    // Who this is, under the figure.
    let (origin, align) = ((centre, frame.bottom() - scale.px(26.0)), TextAlign::Center);
    list.text_aligned(origin, character.hero.name(), tint, scale.text(2.0), align);
}

/// Every equipment slot: its frame, its icon, and what it is called.
fn push_slots(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;

    for worn in EquipSlot::ALL {
        let slot = ItemSlot::Equipment(worn);
        let rect = menu.equipment_rect(worn, viewport);
        push_slot(list, menu, character, slot, rect, theme);

        // The slot's own name, outside the frame, so an empty slot still says what it is.
        // Left column labels sit to the right of the icon and vice versa, which is what
        // keeps the text inside the panel on both sides.
        let is_left = LEFT_COLUMN.contains(&worn);
        let is_right = RIGHT_COLUMN.contains(&worn);
        scratch.clear();
        scratch.push_str(worn.name());
        if is_left {
            list.text_at(
                (rect.right() + scale.px(6.0), rect.y + scale.px(4.0)),
                scratch,
                theme.text_dim,
                scale.text(1.0),
            );
        } else if is_right {
            list.text_aligned(
                (rect.x - scale.px(6.0), rect.y + scale.px(4.0)),
                scratch,
                theme.text_dim,
                scale.text(1.0),
                TextAlign::Right,
            );
        } else {
            list.text_aligned(
                (rect.x + rect.width * 0.5, rect.bottom() + scale.px(4.0)),
                scratch,
                theme.text_dim,
                scale.text(1.0),
                TextAlign::Center,
            );
        }
    }
}

/// The bag: a heading, then the grid.
fn push_inventory(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let grid = menu.inventory_rect(viewport);

    list.text_at(
        (grid.x, grid.y - metrics.section),
        "INVENTORY",
        theme.title,
        scale.text(1.5),
    );
    scratch.clear();
    push_integer(
        scratch,
        u32::try_from(character.inventory.iter().flatten().count()).unwrap_or(0),
    );
    scratch.push('/');
    push_integer(scratch, u32::try_from(INVENTORY_SLOTS).unwrap_or(0));
    list.text_aligned(
        (grid.right(), grid.y - metrics.section + scale.px(2.0)),
        scratch,
        theme.text_dim,
        scale.text(1.0),
        TextAlign::Right,
    );

    let backing = grid.inset(-scale.px(6.0));
    list.rect(backing, theme.section);
    list.outline(backing, scale.px(1.0), theme.section_border);

    for index in 0..INVENTORY_SLOTS {
        let slot = ItemSlot::Inventory(index);
        let rect = menu.cell_rect(index, viewport);
        push_slot(list, menu, character, slot, rect, theme);
    }
}

/// One slot, worn or carried: its frame, what is in it, and how the pointer feels about it.
fn push_slot(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    slot: ItemSlot,
    rect: UiRect,
    theme: &CharacterTheme,
) {
    let scale = menu.metrics().scale;
    let held = character.at(slot);
    let dragged_from = menu.drag().is_some_and(|drag| drag.from == slot);

    list.rect(
        rect,
        if held.is_some() && !dragged_from {
            theme.slot_filled
        } else {
            theme.slot
        },
    );

    // What is in it. The item being dragged is drawn under the pointer instead, so its own
    // slot reads as empty for as long as it is in the air.
    if let Some(stack) = held.filter(|_| !dragged_from) {
        if let Some(def) = stack.def() {
            list.icon(
                rect.inset(scale.px(3.0)),
                icon_for_item(def.icon),
                [1.0, 1.0, 1.0, 1.0],
            );
            // A quality frame around the art, which is how the genre says "this is blue"
            // without a word of text.
            list.outline(rect, scale.px(2.0), quality_color(def.quality));
        }
        if stack.quantity > 1 {
            // Drawn without the scratch buffer the rest of the panel shares, because this
            // runs inside the slot loop; two digits are cheap.
            let mut count = String::with_capacity(4);
            push_integer(&mut count, u32::from(stack.quantity));
            list.text_aligned(
                (rect.right() - scale.px(3.0), rect.bottom() - scale.px(11.0)),
                &count,
                theme.text,
                scale.text(1.0),
                TextAlign::Right,
            );
        }
    } else if let ItemSlot::Equipment(worn) = slot {
        // An empty equipment slot shows what belongs there, dimmed. A bag cell shows
        // nothing, because nothing in particular belongs in it.
        list.icon(
            rect.inset(scale.px(9.0)),
            icon_for_item(ItemIcon::of_family(worn.family())),
            [1.0, 1.0, 1.0, 0.16],
        );
    }

    // The pointer. While something is in the air every slot says whether it would take it;
    // otherwise only the one under the cursor is outlined.
    let outline = match menu.drag() {
        Some(drag) if drag.from != slot => {
            if character.can_move(drag.from, slot) {
                let hovered = menu.hovered() == Some(slot);
                Some((if hovered { 2.5 } else { 1.5 }, theme.valid))
            } else if menu.hovered() == Some(slot) {
                Some((2.5, theme.invalid))
            } else {
                None
            }
        }
        _ if menu.refused() == Some(slot) => Some((2.0, theme.invalid)),
        _ if menu.hovered() == Some(slot) => Some((2.0, theme.hover)),
        _ => Some((1.0, theme.section_border)),
    };
    if let Some((thickness, color)) = outline {
        list.outline(rect, scale.px(thickness), color);
    }
}

/// The item in the air, drawn under the pointer.
fn push_dragged(list: &mut UiDrawList, menu: &CharacterMenu, theme: &CharacterTheme) {
    let Some(drag) = menu.drag() else {
        return;
    };
    let Some(def) = drag.stack.def() else {
        return;
    };
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let (x, y) = drag.icon_origin();
    let ghost = UiRect::new(x, y, metrics.slot, metrics.slot);
    list.rect(ghost, theme.slot_filled);
    list.icon(
        ghost.inset(scale.px(3.0)),
        icon_for_item(def.icon),
        [1.0, 1.0, 1.0, 0.92],
    );
    list.outline(ghost, scale.px(2.0), quality_color(def.quality));
}

/// The tooltip for whatever the pointer is over.
fn push_tooltip(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    // Nothing while an item is in the air: the player is looking at where it is going.
    if menu.drag().is_some() {
        return;
    }
    let Some(slot) = menu.hovered() else {
        return;
    };
    let Some(def) = character.at(slot).and_then(|stack| stack.def()) else {
        return;
    };

    let metrics = menu.metrics();
    let scale = metrics.scale;
    let anchor = menu.slot_rect(slot, viewport);

    // Four fixed lines — name, quality and level, where it goes, and the stat line or the
    // sentence that replaces it — plus one per granted stat.
    let stat_lines = def.stats.entries().count().max(1);
    let lines = 4 + stat_lines;
    let height = metrics.tooltip_line * lines as f32 + scale.px(16.0);
    let mut x = anchor.right() + scale.px(10.0);
    if x + metrics.tooltip_width > viewport.0 as f32 {
        x = (anchor.x - metrics.tooltip_width - scale.px(10.0)).max(0.0);
    }
    let y = anchor.y.min((viewport.1 as f32 - height).max(0.0));
    let frame = UiRect::new(x, y, metrics.tooltip_width, height);

    list.rect(frame, theme.tooltip);
    list.outline(frame, scale.px(1.0), quality_color(def.quality));

    let left = frame.x + scale.px(8.0);
    let mut line = frame.y + scale.px(8.0);
    list.text_at(
        (left, line),
        def.name,
        quality_color(def.quality),
        scale.text(1.5),
    );
    line += metrics.tooltip_line * 1.4;

    scratch.clear();
    scratch.push_str(def.quality.name());
    scratch.push_str("  ·  ITEM LEVEL ");
    push_integer(scratch, u32::from(def.item_level));
    list.text_at((left, line), scratch, theme.text_dim, scale.text(1.0));
    line += metrics.tooltip_line;

    scratch.clear();
    scratch.push_str(def.family.name());
    if let ItemSlot::Equipment(worn) = slot {
        scratch.push_str("  ·  EQUIPPED: ");
        scratch.push_str(worn.name());
    }
    list.text_at((left, line), scratch, theme.text, scale.text(1.0));
    line += metrics.tooltip_line * 1.3;

    if def.stats.is_empty() {
        list.text_at(
            (left, line),
            "GRANTS NO COMBAT STATS",
            theme.text_dim,
            scale.text(1.0),
        );
        line += metrics.tooltip_line;
    } else {
        for (stat, value) in def.stats.entries() {
            scratch.clear();
            scratch.push('+');
            push_number(scratch, value);
            scratch.push(' ');
            scratch.push_str(stat.name());
            list.text_at((left, line), scratch, theme.text, scale.text(1.0));
            line += metrics.tooltip_line;
        }
    }

    list.text_at(
        (left, line + scale.px(4.0)),
        "DRAG TO MOVE OR EQUIP",
        theme.text_dim,
        1.0,
    );
}

/// The line along the bottom that says how the panel is worked.
fn push_footer(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    theme: &CharacterTheme,
    viewport: (u32, u32),
) {
    let metrics = menu.metrics();
    let scale = metrics.scale;
    let panel = menu.panel_rect(viewport);
    list.text_at(
        (panel.x + metrics.padding, panel.bottom() - scale.px(24.0)),
        "DRAG AN ITEM ONTO A SLOT TO EQUIP IT. ESCAPE CLOSES.",
        theme.text_dim,
        scale.text(1.0),
    );
}

/// A heading over a group, and the y the first row under it starts at.
fn push_section_heading(
    list: &mut UiDrawList,
    column: UiRect,
    y: f32,
    label: &str,
    theme: &CharacterTheme,
    metrics: &CharacterMetrics,
) -> f32 {
    let scale = metrics.scale;
    list.text_at(
        (column.x + scale.px(10.0), y),
        label,
        theme.title,
        scale.text(1.5),
    );
    let rule = UiRect::new(
        column.x + scale.px(10.0),
        y + scale.px(16.0),
        column.width - scale.px(20.0),
        scale.px(1.0),
    );
    list.rect(rule, theme.section_border);
    y + metrics.section
}

/// The main stat, printed large.
fn push_main_stat(
    list: &mut UiDrawList,
    column: UiRect,
    y: f32,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    metrics: &CharacterMetrics,
) {
    let scale = metrics.scale;
    list.text_at(
        (column.x + scale.px(10.0), y),
        character.main_stat.name(),
        theme.main_stat,
        scale.text(1.5),
    );
    scratch.clear();
    push_number(scratch, character.total.get(character.main_stat));
    list.text_aligned(
        (column.right() - scale.px(10.0), y - scale.px(2.0)),
        scratch,
        theme.main_stat,
        scale.text(2.0),
        TextAlign::Right,
    );
}

/// One row of the stat list: its name on the left, its value on the right.
///
/// A rating prints the percentage it is worth beside it, because that is the number gameplay
/// multiplies by and a rating on its own does not tell a player anything.
fn push_stat_row(
    list: &mut UiDrawList,
    row: UiRect,
    stat: Stat,
    stats: &StatBlock,
    theme: &CharacterTheme,
    scratch: &mut String,
    metrics: &CharacterMetrics,
) {
    let scale = metrics.scale;
    let value = stats.get(stat);
    let color = if value > 0.0 {
        theme.text
    } else {
        theme.text_dim
    };
    list.text_at(
        (row.x + scale.px(10.0), row.y),
        stat.name(),
        color,
        scale.text(1.0),
    );

    scratch.clear();
    push_number(scratch, value);
    if stat.is_rating() {
        scratch.push_str("  (");
        push_percent(scratch, stats.percent(stat));
        scratch.push(')');
    }
    list.text_aligned(
        (row.right() - scale.px(10.0), row.y),
        scratch,
        color,
        scale.text(1.0),
        TextAlign::Right,
    );
}

/// The rectangle one row of a stat list occupies inside `column`.
fn stat_row_rect(column: UiRect, y: f32, metrics: &CharacterMetrics) -> UiRect {
    UiRect::new(column.x, y, column.width, metrics.stat_row)
}

/// One row of the derived list.
fn push_derived_row(
    list: &mut UiDrawList,
    column: UiRect,
    y: f32,
    label: &str,
    value: &str,
    theme: &CharacterTheme,
    metrics: &CharacterMetrics,
) {
    let scale = metrics.scale;
    list.text_at(
        (column.x + scale.px(10.0), y),
        label,
        theme.text_dim,
        scale.text(1.0),
    );
    list.text_aligned(
        (column.right() - scale.px(10.0), y),
        value,
        theme.text,
        scale.text(1.0),
        TextAlign::Right,
    );
}

/// Writes a whole number into `out` without allocating.
fn push_integer(out: &mut String, value: u32) {
    use core::fmt::Write as _;
    let _ = write!(out, "{value}");
}

/// Writes a stat value: whole when it is whole, one decimal when it is not.
fn push_number(out: &mut String, value: f32) {
    use core::fmt::Write as _;
    if (value - value.round()).abs() < 0.05 {
        let _ = write!(out, "{}", value.round() as i64);
    } else {
        let _ = write!(out, "{value:.1}");
    }
}

/// Writes a fraction as a percentage with two decimals: `0.15` becomes `15.00%`.
fn push_percent(out: &mut String, fraction: f32) {
    use core::fmt::Write as _;
    let _ = write!(out, "{:.2}%", fraction * 100.0);
}

/// Writes a duration in seconds, to two decimals.
fn push_seconds(out: &mut String, seconds: f32) {
    use core::fmt::Write as _;
    let _ = write!(out, "{seconds:.2}S");
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::character::{ItemFamily, starting_item};
    use engine_render_data::ui::UiCommand;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn snapshot() -> UiSnapshot {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        UiSnapshot::build(&simulation)
    }

    fn open() -> CharacterMenu {
        let mut menu = CharacterMenu::new();
        menu.open();
        menu
    }

    fn render(menu: &CharacterMenu, snapshot: &UiSnapshot) -> UiDrawList {
        let mut list = UiDrawList::new();
        let mut scratch = String::new();
        push_character_menu(
            &mut list,
            menu,
            snapshot,
            &CharacterTheme::default(),
            &mut scratch,
            VIEWPORT,
        );
        list
    }

    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } => Some(list.text_of(*range).to_owned()),
                _ => None,
            })
            .collect()
    }

    fn icons(list: &UiDrawList) -> Vec<UiIcon> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Icon { icon, .. } => Some(*icon),
                _ => None,
            })
            .collect()
    }

    fn icon_rects(list: &UiDrawList) -> Vec<UiRect> {
        drawn_icons(list)
            .into_iter()
            .map(|(_, rect, _)| rect)
            .collect()
    }

    fn drawn_icons(list: &UiDrawList) -> Vec<(UiIcon, UiRect, UiColor)> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Icon { icon, rect, tint } => Some((*icon, *rect, *tint)),
                _ => None,
            })
            .collect()
    }

    fn at(x: f32, y: f32) -> UiInput {
        UiInput::new().with_pointer(x, y)
    }

    fn press(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..at(x, y)
        }
    }

    fn release(x: f32, y: f32) -> UiInput {
        UiInput {
            pointer_released: true,
            ..at(x, y)
        }
    }

    fn centre(rect: UiRect) -> (f32, f32) {
        (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5)
    }

    #[test]
    fn a_closed_panel_draws_nothing_and_answers_nothing() {
        let menu = CharacterMenu::new();
        assert!(!menu.is_open());
        assert!(render(&menu, &snapshot()).is_empty());
    }

    #[test]
    fn all_fifteen_slots_are_drawn_inside_the_panel_and_none_overlaps_another() {
        let menu = open();
        let panel = menu.panel_rect(VIEWPORT);
        let mut rects = Vec::new();
        for slot in EquipSlot::ALL {
            let rect = menu.equipment_rect(slot, VIEWPORT);
            assert!(rect.x >= panel.x, "{} escaped left", slot.name());
            assert!(
                rect.right() <= panel.right() + 0.01,
                "{} escaped right",
                slot.name()
            );
            assert!(
                rect.bottom() <= panel.bottom() + 0.01,
                "{} escaped below",
                slot.name()
            );
            rects.push((slot, rect));
        }
        assert_eq!(rects.len(), 15);
        for (a, first) in &rects {
            for (b, second) in &rects {
                if a == b {
                    continue;
                }
                let apart = first.right() <= second.x
                    || second.right() <= first.x
                    || first.bottom() <= second.y
                    || second.bottom() <= first.y;
                assert!(apart, "{} overlaps {}", a.name(), b.name());
            }
        }
    }

    #[test]
    fn every_bag_cell_is_inside_the_panel_and_the_grid_is_rectangular() {
        let menu = open();
        let panel = menu.panel_rect(VIEWPORT);
        for index in 0..INVENTORY_SLOTS {
            let rect = menu.cell_rect(index, VIEWPORT);
            assert!(rect.x >= panel.x && rect.right() <= panel.right() + 0.01);
            assert!(
                rect.bottom() <= panel.bottom() + 0.01,
                "row {index} escaped"
            );
        }
        let first = menu.cell_rect(0, VIEWPORT);
        let next_row = menu.cell_rect(INVENTORY_COLUMNS, VIEWPORT);
        assert!(
            (first.x - next_row.x).abs() < 0.01,
            "the grid is not square"
        );
        assert!(next_row.y > first.y);
    }

    #[test]
    fn what_is_drawn_and_what_is_clicked_are_the_same_rectangle() {
        let menu = open();
        for slot in EquipSlot::ALL {
            let rect = menu.equipment_rect(slot, VIEWPORT);
            let (x, y) = centre(rect);
            assert_eq!(
                menu.slot_at(x, y, VIEWPORT),
                Some(ItemSlot::Equipment(slot)),
                "{} does not hit-test where it is drawn",
                slot.name()
            );
        }
        for index in 0..INVENTORY_SLOTS {
            let (x, y) = centre(menu.cell_rect(index, VIEWPORT));
            assert_eq!(
                menu.slot_at(x, y, VIEWPORT),
                Some(ItemSlot::Inventory(index))
            );
        }
        assert_eq!(menu.slot_at(1.0, 1.0, VIEWPORT), None);
    }

    #[test]
    fn the_panel_names_the_hero_the_stats_and_every_slot() {
        let snapshot = snapshot();
        let drawn = texts(&render(&open(), &snapshot));

        assert!(drawn.contains(&"CHARACTER".to_owned()), "{drawn:?}");
        assert!(drawn.iter().any(|text| text.contains("FIREMAGE")));
        assert!(drawn.contains(&"PRIMARY".to_owned()));
        assert!(drawn.contains(&"SECONDARY".to_owned()));
        assert!(drawn.contains(&"DEFENSIVE".to_owned()));
        assert!(drawn.contains(&"DERIVED".to_owned()));
        assert!(drawn.contains(&"INVENTORY".to_owned()));
        for stat in Stat::ALL {
            if stat == Stat::Strength {
                continue; // Not the FireMage's; only the main stat and the four ratings show.
            }
            assert!(
                drawn.iter().any(|text| text == stat.name()),
                "{} is missing",
                stat.name()
            );
        }
        for slot in EquipSlot::ALL {
            assert!(
                drawn.iter().any(|text| text == slot.name()),
                "{} has no label",
                slot.name()
            );
        }
    }

    #[test]
    fn the_main_stat_shown_is_the_one_the_hero_has() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();

        for (hero, expected, absent) in [
            (Hero::FireMage, Stat::Intellect, Stat::Strength),
            (Hero::Warrior, Stat::Strength, Stat::Intellect),
            (Hero::Druid, Stat::Intellect, Stat::Strength),
        ] {
            simulation.set_hero(hero);
            let snapshot = UiSnapshot::build(&simulation);
            assert_eq!(snapshot.character.main_stat, expected);
            let drawn = texts(&render(&open(), &snapshot));
            assert!(drawn.iter().any(|text| text == expected.name()), "{hero:?}");
            assert!(
                !drawn.iter().any(|text| text == absent.name()),
                "{hero:?} shows {}",
                absent.name()
            );
        }
    }

    #[test]
    fn every_secondary_stat_is_shown_and_reads_zero() {
        let snapshot = snapshot();
        for stat in Stat::ALL {
            if stat.group() == StatGroup::Secondary {
                assert_eq!(snapshot.character.total.get(stat), 0.0);
            }
        }
        let drawn = texts(&render(&open(), &snapshot));
        assert!(
            drawn.iter().any(|text| text.starts_with("0  (0.00%)")),
            "a rating is not printed as a rating and a percentage: {drawn:?}"
        );
    }

    #[test]
    fn the_worn_items_draw_their_art_and_the_empty_slots_draw_a_hint() {
        let snapshot = snapshot();
        let drawn = icons(&render(&open(), &snapshot));
        // One per worn item, one per empty equipment slot, plus the bag's contents.
        for expected in [
            UiIcon::Chestpiece,
            UiIcon::Leggings,
            UiIcon::Bracers,
            UiIcon::Boots,
            UiIcon::Helm,
            UiIcon::Ring,
            UiIcon::Trinket,
        ] {
            assert!(drawn.contains(&expected), "{expected:?} is not drawn");
        }
        assert!(drawn.len() >= EquipSlot::COUNT);
    }

    #[test]
    fn hovering_an_item_shows_its_name_quality_and_that_it_grants_nothing() {
        let snapshot = snapshot();
        let mut menu = open();
        let chest = menu.equipment_rect(EquipSlot::Chest, VIEWPORT);
        let (x, y) = centre(chest);
        menu.update(&at(x, y), &snapshot, VIEWPORT);
        assert_eq!(menu.hovered(), Some(ItemSlot::Equipment(EquipSlot::Chest)));

        let drawn = texts(&render(&menu, &snapshot));
        let def = starting_item(ItemFamily::Chest).expect("a chestpiece");
        assert!(drawn.iter().any(|text| text == def.name), "{drawn:?}");
        assert!(drawn.iter().any(|text| text.starts_with("COMMON")));
        assert!(drawn.contains(&"GRANTS NO COMBAT STATS".to_owned()));
    }

    #[test]
    fn hovering_an_empty_slot_shows_no_tooltip() {
        let snapshot = snapshot();
        let mut menu = open();
        let (x, y) = centre(menu.equipment_rect(EquipSlot::Ring1, VIEWPORT));
        menu.update(&at(x, y), &snapshot, VIEWPORT);
        let drawn = texts(&render(&menu, &snapshot));
        assert!(!drawn.contains(&"GRANTS NO COMBAT STATS".to_owned()));
    }

    #[test]
    fn dragging_a_ring_from_the_bag_onto_a_finger_asks_for_it_to_be_equipped() {
        let snapshot = snapshot();
        let mut menu = open();
        let (index, _) = snapshot
            .character
            .inventory
            .iter()
            .enumerate()
            .find_map(|(index, cell)| {
                cell.filter(|stack| stack.fits(EquipSlot::Ring1))
                    .map(|stack| (index, stack))
            })
            .expect("a ring in the bag");

        let (from_x, from_y) = centre(menu.cell_rect(index, VIEWPORT));
        assert_eq!(
            menu.update(&press(from_x, from_y), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Idle
        );
        assert!(menu.drag().is_some(), "nothing was picked up");

        let (to_x, to_y) = centre(menu.equipment_rect(EquipSlot::Ring1, VIEWPORT));
        assert_eq!(
            menu.update(&release(to_x, to_y), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Move {
                from: ItemSlot::Inventory(index),
                to: ItemSlot::Equipment(EquipSlot::Ring1),
            }
        );
        assert!(menu.drag().is_none(), "the item stayed in the air");
        assert!(menu.is_open(), "equipping closed the panel");
    }

    #[test]
    fn dragging_a_worn_item_into_the_bag_asks_for_it_to_come_off() {
        let snapshot = snapshot();
        let mut menu = open();
        let free = snapshot
            .character
            .inventory
            .iter()
            .position(Option::is_none)
            .expect("a free cell");

        let (from_x, from_y) = centre(menu.equipment_rect(EquipSlot::Feet, VIEWPORT));
        menu.update(&press(from_x, from_y), &snapshot, VIEWPORT);
        let (to_x, to_y) = centre(menu.cell_rect(free, VIEWPORT));
        assert_eq!(
            menu.update(&release(to_x, to_y), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Move {
                from: ItemSlot::Equipment(EquipSlot::Feet),
                to: ItemSlot::Inventory(free),
            }
        );
    }

    #[test]
    fn dropping_an_item_where_it_does_not_belong_asks_for_nothing_and_says_so() {
        let snapshot = snapshot();
        let mut menu = open();
        let (from_x, from_y) = centre(menu.equipment_rect(EquipSlot::Chest, VIEWPORT));
        menu.update(&press(from_x, from_y), &snapshot, VIEWPORT);

        let (to_x, to_y) = centre(menu.equipment_rect(EquipSlot::Ring1, VIEWPORT));
        assert_eq!(
            menu.update(&release(to_x, to_y), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Idle
        );
        assert_eq!(
            menu.refused(),
            Some(ItemSlot::Equipment(EquipSlot::Ring1)),
            "the refusal is not reported"
        );
        assert!(menu.drag().is_none());
    }

    #[test]
    fn letting_go_over_nothing_calls_the_drag_off_without_refusing_it() {
        let snapshot = snapshot();
        let mut menu = open();
        let (x, y) = centre(menu.equipment_rect(EquipSlot::Chest, VIEWPORT));
        menu.update(&press(x, y), &snapshot, VIEWPORT);

        let panel = menu.panel_rect(VIEWPORT);
        let empty = (panel.x + panel.width * 0.5, panel.bottom() - 8.0);
        assert_eq!(
            menu.update(&release(empty.0, empty.1), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Idle
        );
        assert_eq!(menu.refused(), None);
        assert!(menu.drag().is_none());
    }

    #[test]
    fn a_drag_never_starts_from_an_empty_slot() {
        let snapshot = snapshot();
        let mut menu = open();
        let (x, y) = centre(menu.equipment_rect(EquipSlot::Trinket4, VIEWPORT));
        menu.update(&press(x, y), &snapshot, VIEWPORT);
        assert!(menu.drag().is_none());
    }

    #[test]
    fn escape_and_a_click_outside_both_close_it() {
        let snapshot = snapshot();
        let mut menu = open();
        assert_eq!(
            menu.update(
                &UiInput {
                    cancel: true,
                    ..UiInput::new()
                },
                &snapshot,
                VIEWPORT
            ),
            CharacterMenuOutcome::Closed
        );
        assert!(!menu.is_open());

        menu.open();
        assert_eq!(
            menu.update(&press(2.0, 2.0), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Closed
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn toggling_opens_and_closes_and_closing_drops_what_is_held() {
        let snapshot = snapshot();
        let mut menu = CharacterMenu::new();
        menu.toggle();
        assert!(menu.is_open());

        let (x, y) = centre(menu.equipment_rect(EquipSlot::Chest, VIEWPORT));
        menu.update(&press(x, y), &snapshot, VIEWPORT);
        assert!(menu.drag().is_some());

        menu.toggle();
        assert!(!menu.is_open());
        assert!(menu.drag().is_none());
    }

    #[test]
    fn every_measurement_grows_with_the_scale_and_the_layout_keeps_its_shape() {
        let small = CharacterMenu::with_scale(UiScale::new(1.0));
        let large = CharacterMenu::with_scale(UiScale::new(2.0));
        let ratio = |a: f32, b: f32| (b / a - 2.0).abs() < 0.01;

        assert!(ratio(
            small.metrics().panel_width,
            large.metrics().panel_width
        ));
        assert!(ratio(small.metrics().slot, large.metrics().slot));

        // The panel stays centred, and every slot stays inside it, at either size.
        for menu in [small, large] {
            let panel = menu.panel_rect(VIEWPORT);
            for slot in EquipSlot::ALL {
                assert!(menu.equipment_rect(slot, VIEWPORT).x >= panel.x);
            }
        }
    }

    #[test]
    fn the_panel_redraws_from_the_sheet_rather_than_from_what_was_dragged() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let before = UiSnapshot::build(&simulation);

        let (index, _) = before
            .character
            .inventory
            .iter()
            .enumerate()
            .find_map(|(index, cell)| {
                cell.filter(|stack| stack.fits(EquipSlot::Ring1))
                    .map(|stack| (index, stack))
            })
            .expect("a ring");
        simulation
            .move_item(
                ItemSlot::Inventory(index),
                ItemSlot::Equipment(EquipSlot::Ring1),
            )
            .expect("the ring goes on");
        let after = UiSnapshot::build(&simulation);

        let menu = open();
        assert_ne!(
            render(&menu, &before).commands(),
            render(&menu, &after).commands(),
            "equipping a ring changed nothing on screen"
        );
    }

    #[test]
    fn the_whole_panel_fits_the_smallest_window_the_game_runs_in() {
        const SMALLEST: (u32, u32) = (1280, 720);
        let menu = open();
        let panel = menu.panel_rect(SMALLEST);
        assert!(
            panel.x >= 0.0 && panel.y >= 0.0,
            "the panel starts off screen"
        );
        assert!(
            panel.right() <= SMALLEST.0 as f32 && panel.bottom() <= SMALLEST.1 as f32,
            "the panel is {}x{} in a {}x{} window",
            panel.width,
            panel.height,
            SMALLEST.0,
            SMALLEST.1
        );

        // And everything the player has to be able to reach is inside it.
        let grid = menu.inventory_rect(SMALLEST);
        assert!(grid.bottom() <= panel.bottom(), "the bag is off the panel");
        for index in 0..INVENTORY_SLOTS {
            assert!(menu.cell_rect(index, SMALLEST).bottom() <= panel.bottom());
        }
        for slot in EquipSlot::ALL {
            assert!(menu.equipment_rect(slot, SMALLEST).bottom() <= grid.y);
        }
    }

    #[test]
    fn the_dragged_item_is_drawn_under_the_cursor_and_follows_it() {
        let snapshot = snapshot();
        let mut menu = open();
        let origin = menu.equipment_rect(EquipSlot::Chest, VIEWPORT);

        // Picked up a few pixels in from the slot's corner.
        let grab = (origin.x + 9.0, origin.y + 7.0);
        menu.update(&press(grab.0, grab.1), &snapshot, VIEWPORT);
        let drag = menu.drag().expect("the chestpiece is in the air");
        assert_eq!(drag.grab, (9.0, 7.0));
        assert_eq!(drag.icon_origin(), (origin.x, origin.y));

        // Moved across the panel, over no slot at all: the item stays in the air and the
        // icon is under the cursor rather than back on the slot it came from.
        let held = UiInput {
            pointer_down: true,
            ..at(grab.0 + 260.0, grab.1 + 31.0)
        };
        menu.update(&held, &snapshot, VIEWPORT);
        let drag = menu.drag().expect("crossing a gap dropped the item");
        assert_eq!(drag.icon_origin(), (origin.x + 260.0, origin.y + 31.0));

        let drawn = icon_rects(&render(&menu, &snapshot));
        assert!(
            drawn
                .iter()
                .any(|rect| (rect.x - drag.icon_origin().0).abs() < menu.metrics().slot),
            "no icon is drawn near the cursor"
        );
    }

    #[test]
    fn the_slot_an_item_was_picked_up_from_reads_as_empty_while_it_is_held() {
        let snapshot = snapshot();
        let mut menu = open();
        let origin = menu.equipment_rect(EquipSlot::Chest, VIEWPORT);
        let (x, y) = centre(origin);

        // An empty slot draws the family it wants at a fraction of full alpha; a filled one
        // draws the item at full. Dragging the item out has to turn the first into the
        // second, or the item is on screen twice.
        let opaque_in_origin = |menu: &CharacterMenu| {
            drawn_icons(&render(menu, &snapshot))
                .into_iter()
                .filter(|(_, rect, tint)| {
                    tint[3] > 0.5
                        && rect.x >= origin.x
                        && rect.right() <= origin.right()
                        && rect.y >= origin.y
                        && rect.bottom() <= origin.bottom()
                })
                .count()
        };

        assert_eq!(opaque_in_origin(&menu), 1, "the chestpiece is not drawn");
        menu.update(&press(x, y), &snapshot, VIEWPORT);

        // One opaque icon in that rectangle, and it is the one in the air: on the frame of
        // the press the cursor has not moved yet, so the ghost sits exactly on the slot.
        assert_eq!(
            opaque_in_origin(&menu),
            1,
            "the item is drawn both in its slot and in the air"
        );
        let drag = menu.drag().expect("something is held");
        assert_eq!(drag.icon_origin(), (origin.x, origin.y));

        // Moved away, the origin slot keeps only its dim hint.
        let held = UiInput {
            pointer_down: true,
            ..at(x + 300.0, y)
        };
        menu.update(&held, &snapshot, VIEWPORT);
        assert_eq!(
            opaque_in_origin(&menu),
            0,
            "the item stayed behind in the slot it left"
        );
    }

    #[test]
    fn a_whole_frame_of_the_panel_stays_a_few_hundred_commands() {
        let list = render(&open(), &snapshot());
        assert!(
            list.len() < 500,
            "the character sheet costs {} commands",
            list.len()
        );
    }
}
