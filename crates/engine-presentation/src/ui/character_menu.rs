//! The character sheet: stats down the left, the character and their thirteen slots in the
//! middle, the bag and the numbers the gear produces on the right.
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────────────────────┐
//! │ Character          Fire Mage · ranged fire                    Item level 4.3  │
//! ├──────────────┬────────────────────────┬──────────────────────────────────────┤
//! │ PRIMARY      │ [head]         [back]  │ INVENTORY                      3 / 32 │
//! │  Intellect   │ [shld]         [neck]  │  ▣ ▣ ▣ ▢ ▢ ▢ ▢ ▢                      │
//! │  Stamina     │ [chst]  figure [wrst]  │  ▢ ▢ ▢ ▢ ▢ ▢ ▢ ▢   (4 rows of 8)      │
//! │ SECONDARY    │ [hand]         [feet]  ├──────────────────────────────────────┤
//! │  Haste 1.96% │ [legs]                 │ DERIVED            FROM EQUIPMENT    │
//! │  ...         │   [ring] [ring]        │  Health            +2 Haste          │
//! │ DEFENSIVE    │     [trk] [trk]        │  Spell power       +3 Intellect      │
//! └──────────────┴────────────────────────┴──────────────────────────────────────┘
//! ```
//!
//! # Drawn in the same language as the tooltips
//!
//! Panels, type, colours and spacing are the interface kit's ([`super::kit`],
//! [`super::map_style`]), the same tokens the spell tooltip and the dungeon map are built
//! from. Slots are 64 logical pixels, labels are the proportional faces at 13 to 17 pixels, and
//! an item is explained by the item tooltip ([`super::tooltip::ItemTooltip`]) docked beside its
//! slot - never at the pointer, never in a corner.
//!
//! # The menu owns no items
//!
//! Like [`super::talent_menu`], this reads the frame's [`UiSnapshot`] and emits a request.
//! Dragging a ring onto a finger produces [`CharacterMenuOutcome::Move`]; the application
//! turns it into a [`engine_game::PlayerCommand::MoveItem`] and the simulation decides. So
//! the same drag is reproducible in a replay, and a drop the interface allowed but gameplay
//! refuses simply does not happen - the panel is drawn from the next frame's snapshot, which
//! is the sheet as it actually is. A right-click is the same request without the drag: a bag
//! item goes to its slot, a worn item to the first free bag cell.
//!
//! The one thing the menu does keep is the drag in progress: where the item was picked up
//! from, and whether the last drop was refused. Both are questions about the *pointer*,
//! which gameplay has no opinion about.
//!
//! # Every slot is the same slot
//!
//! An equipment slot and a bag cell are both an [`ItemSlot`], hit-tested by
//! [`CharacterMenu::slot_at`] and drawn by the same function. There is no equip path, no
//! unequip path and no rearrange path - there is one drag, from a slot to a slot, and which
//! of the three it turns out to be is a consequence of the two addresses.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiIcon, UiRect, UiScale};

use engine_game::character::equipment::{INVENTORY_COLUMNS, INVENTORY_SLOTS};
use engine_game::character::stat::{StatGroup, armor_reduction};
use engine_game::character::{
    EquipSlot, ItemIcon, ItemQuality, ItemSlot, ItemStack, Stat, StatBlock, active_sets_of,
};
use engine_game::hero::Hero;

use super::input::UiInput;
use super::kit::{self, Kit, tokens};
use super::map_style::{TextStyle, blend, color, push_title_case, radius, type_scale};
use super::tooltip::{ItemContext, ItemTooltip, write_set_bonus};
use crate::game_ui::write_integer;
use crate::snapshot::{CharacterView, UiSnapshot};

/// Layout of the panel, in design pixels.
///
/// One `mod` of plain numbers, converted to physical pixels once by [`CharacterMetrics`],
/// exactly as [`super::talent_menu`] and the HUD do.
///
/// The whole panel fits inside 1280x720 at an interface scale of one, which is the smallest
/// window the game runs in. A test asserts it, because a sheet that is taller than the screen
/// is a sheet whose bag cannot be reached - and the interface scale slider only makes it
/// bigger from here.
mod layout {
    /// Width of the whole panel.
    pub const PANEL_WIDTH: f32 = 1264.0;
    /// Height of the whole panel.
    pub const PANEL_HEIGHT: f32 = 704.0;
    /// Padding inside the panel.
    pub const PADDING: f32 = 20.0;
    /// Height of the heading strip.
    pub const HEADER: f32 = 60.0;
    /// Height of the hint line along the bottom.
    pub const FOOTER: f32 = 30.0;
    /// Edge of one equipment slot.
    pub const SLOT: f32 = 64.0;
    /// Gap between two equipment slots.
    pub const SLOT_GAP: f32 = 12.0;
    /// Edge of one bag cell.
    pub const CELL: f32 = 60.0;
    /// Gap between two bag cells.
    pub const CELL_GAP: f32 = 6.0;
    /// Width of the stats column on the left.
    pub const STATS_WIDTH: f32 = 300.0;
    /// Width of the middle column: the character and the gear around them.
    pub const GEAR_WIDTH: f32 = 330.0;
    /// Gap between the three columns.
    pub const COLUMN_GAP: f32 = 16.0;
    /// How many equipment slots stand in the taller of the two columns.
    pub const GEAR_ROWS: usize = 5;
    /// Height of a section heading.
    pub const SECTION: f32 = 24.0;
    /// Height of one row of a stat list.
    pub const ROW: f32 = 26.0;
    /// Gap between the gear block and the rows of rings and trinkets.
    pub const JEWELRY_GAP: f32 = 18.0;
    /// Height of the bag card's heading, above the grid.
    pub const BAG_HEADING: f32 = 40.0;
    /// Gap between the bag card and the derived card under it.
    pub const CARD_GAP: f32 = 16.0;
}

/// Edge of one bag cell, in design pixels: what other screens that show bag items match.
pub const BAG_CELL: f32 = layout::CELL;
/// Gap between two bag cells, in design pixels.
pub const BAG_CELL_GAP: f32 = layout::CELL_GAP;

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

/// The two rings, in the row under the character.
const RING_ROW: [EquipSlot; 2] = [EquipSlot::Ring1, EquipSlot::Ring2];

/// The two trinkets, in the row under the rings.
const TRINKET_ROW: [EquipSlot; 2] = [EquipSlot::Trinket1, EquipSlot::Trinket2];

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
    /// `layout::FOOTER` in physical pixels.
    pub footer: f32,
    /// `layout::SLOT` in physical pixels.
    pub slot: f32,
    /// `layout::SLOT_GAP` in physical pixels.
    pub slot_gap: f32,
    /// `layout::CELL` in physical pixels.
    pub cell: f32,
    /// `layout::CELL_GAP` in physical pixels.
    pub cell_gap: f32,
    /// `layout::STATS_WIDTH` in physical pixels.
    pub stats_width: f32,
    /// `layout::GEAR_WIDTH` in physical pixels.
    pub gear_width: f32,
    /// `layout::COLUMN_GAP` in physical pixels.
    pub column_gap: f32,
    /// `layout::SECTION` in physical pixels.
    pub section: f32,
    /// `layout::ROW` in physical pixels.
    pub row: f32,
    /// `layout::JEWELRY_GAP` in physical pixels.
    pub jewelry_gap: f32,
    /// `layout::BAG_HEADING` in physical pixels.
    pub bag_heading: f32,
    /// `layout::CARD_GAP` in physical pixels.
    pub card_gap: f32,
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
            footer: scale.px(layout::FOOTER),
            slot: scale.px(layout::SLOT),
            slot_gap: scale.px(layout::SLOT_GAP),
            cell: scale.px(layout::CELL),
            cell_gap: scale.px(layout::CELL_GAP),
            stats_width: scale.px(layout::STATS_WIDTH),
            gear_width: scale.px(layout::GEAR_WIDTH),
            column_gap: scale.px(layout::COLUMN_GAP),
            section: scale.px(layout::SECTION),
            row: scale.px(layout::ROW),
            jewelry_gap: scale.px(layout::JEWELRY_GAP),
            bag_heading: scale.px(layout::BAG_HEADING),
            card_gap: scale.px(layout::CARD_GAP),
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
        self.cell + self.cell_gap
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
    /// The player dropped (or right-clicked) an item somewhere it may go.
    ///
    /// A *request*. The simulation applies it, and the panel redraws from the result.
    Move {
        /// Where the item was picked up.
        from: ItemSlot,
        /// Where it was dropped.
        to: ItemSlot,
    },
    /// The player right-clicked a book in the bag: read it. A request, like a move; the server
    /// consumes the item and learns the book.
    Use {
        /// The bag cell holding the book.
        cell: usize,
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
    /// Where the dragged icon's top-left corner belongs.
    ///
    /// The pointer, less where inside the slot it took hold, so the icon sits under the
    /// cursor exactly where it was picked up rather than jumping its own width on the first
    /// frame.
    #[must_use]
    pub fn icon_origin(&self) -> (f32, f32) {
        (self.pointer.0 - self.grab.0, self.pointer.1 - self.grab.1)
    }
}

/// Where an item of `stack`'s family would be worn: the first empty slot that takes it, or
/// the first slot that takes it when every one is occupied (a swap).
///
/// What right-clicking a bag item asks for, and what the item tooltip compares against.
#[must_use]
pub fn equip_target(character: &CharacterView, stack: ItemStack) -> Option<EquipSlot> {
    let def = stack.def()?;
    let mut first = None;
    for slot in def.family.slots() {
        first.get_or_insert(slot);
        if character.equipment[slot.index()].is_none() {
            return Some(slot);
        }
    }
    first
}

/// The first empty bag cell.
fn free_cell(character: &CharacterView) -> Option<usize> {
    character.inventory.iter().position(Option::is_none)
}

/// The character panel: whether it is open, what the pointer is over, and what it is holding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterMenu {
    open: bool,
    hovered: Option<ItemSlot>,
    drag: Option<Drag>,
    refused: Option<ItemSlot>,
    /// The character is committed to a run (queued, in a party, in a dungeon): the sheet shows
    /// the locked configuration and moves nothing.
    locked: bool,
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
            locked: false,
            metrics: CharacterMetrics::new(scale),
        }
    }

    /// Locks or unlocks the sheet. Set every frame by the application from what the realm says;
    /// a locked sheet is read-only: it starts no drag and emits no move.
    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
        if locked {
            self.drag = None;
        }
    }

    /// Whether the sheet is read-only.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked
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

    /// The measurements for a window of `viewport` pixels: the configured scale, reduced
    /// until the whole panel fits.
    ///
    /// The sheet is a fixed arrangement of three columns and cannot reflow, so a scale that
    /// would push it past the edge of a small window shrinks it instead - a sheet whose bag
    /// is off screen is worse than a smaller one.
    #[must_use]
    pub fn metrics_for(&self, viewport: (u32, u32)) -> CharacterMetrics {
        let fit =
            (viewport.0 as f32 / layout::PANEL_WIDTH).min(viewport.1 as f32 / layout::PANEL_HEIGHT);
        let configured = self.metrics.scale.factor();
        if fit >= configured || fit <= 0.0 {
            self.metrics
        } else {
            CharacterMetrics::new(UiScale::new(fit.max(UiScale::MIN)))
        }
    }

    /// Shorthand for [`CharacterMenu::metrics_for`] inside the layout functions.
    fn m(&self, viewport: (u32, u32)) -> CharacterMetrics {
        self.metrics_for(viewport)
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
    /// the release. A release outside every slot is a cancel and not a refusal - the player
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

        // A right-click is the whole drag in one gesture: a bag item goes where it is worn,
        // a worn item goes to the bag.
        if input.secondary_pressed
            && !self.locked
            && self.drag.is_none()
            && let Some(slot) = over
            && let Some(stack) = character.at(slot)
        {
            if let ItemSlot::Inventory(cell) = slot
                && stack.def().is_some_and(|def| def.family == engine_game::character::ItemFamily::Book)
            {
                return if character.has_read(stack.def().map_or("", |def| def.key)) {
                    self.refused = Some(slot);
                    CharacterMenuOutcome::Idle
                } else {
                    CharacterMenuOutcome::Use { cell }
                };
            }
            let target = match slot {
                ItemSlot::Inventory(_) => equip_target(character, stack).map(ItemSlot::Equipment),
                ItemSlot::Equipment(_) => free_cell(character).map(ItemSlot::Inventory),
            };
            if let Some(to) = target
                && character.can_move(slot, to)
            {
                return CharacterMenuOutcome::Move { from: slot, to };
            }
            self.refused = Some(target.unwrap_or(slot));
            return CharacterMenuOutcome::Idle;
        }

        // Picking up: a press on a slot with something in it.
        if input.pointer_pressed {
            self.refused = None;
            if !self.locked
                && let Some(slot) = over
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
            ((width - self.m(viewport).panel_width) * 0.5).max(0.0),
            ((height - self.m(viewport).panel_height) * 0.5).max(0.0),
            self.m(viewport).panel_width,
            self.m(viewport).panel_height,
        )
    }

    /// The top of everything under the heading strip.
    fn body_top(&self, viewport: (u32, u32)) -> f32 {
        self.panel_rect(viewport).y + self.m(viewport).header
    }

    /// The bottom of everything above the hint line.
    fn body_bottom(&self, viewport: (u32, u32)) -> f32 {
        self.panel_rect(viewport).bottom()
            - self.m(viewport).padding
            - self.m(viewport).footer * 0.5
    }

    /// The left-hand column: the primary, secondary and defensive stat lists.
    #[must_use]
    pub fn stats_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        let top = self.body_top(viewport);
        UiRect::new(
            panel.x + self.m(viewport).padding,
            top,
            self.m(viewport).stats_width,
            self.body_bottom(viewport) - top,
        )
    }

    /// The middle column: the character, with the gear around them.
    #[must_use]
    pub fn gear_rect(&self, viewport: (u32, u32)) -> UiRect {
        let stats = self.stats_rect(viewport);
        UiRect::new(
            stats.right() + self.m(viewport).column_gap,
            stats.y,
            self.m(viewport).gear_width,
            layout::GEAR_ROWS as f32 * self.m(viewport).slot_pitch() - self.m(viewport).slot_gap,
        )
    }

    /// Where the character themselves is drawn: between the two columns of gear.
    #[must_use]
    pub fn portrait_rect(&self, viewport: (u32, u32)) -> UiRect {
        let gear = self.gear_rect(viewport);
        let inset = self.m(viewport).slot + self.m(viewport).column_gap;
        UiRect::new(
            gear.x + inset,
            gear.y,
            (gear.width - inset * 2.0).max(0.0),
            gear.height,
        )
    }

    /// The right-hand column: the bag card over the derived card.
    #[must_use]
    pub fn right_rect(&self, viewport: (u32, u32)) -> UiRect {
        let panel = self.panel_rect(viewport);
        let gear = self.gear_rect(viewport);
        let left = gear.right() + self.m(viewport).column_gap;
        let top = self.body_top(viewport);
        UiRect::new(
            left,
            top,
            panel.right() - self.m(viewport).padding - left,
            self.body_bottom(viewport) - top,
        )
    }

    /// Where the equipment slot for `slot` sits.
    #[must_use]
    pub fn equipment_rect(&self, slot: EquipSlot, viewport: (u32, u32)) -> UiRect {
        let gear = self.gear_rect(viewport);
        let pitch = self.m(viewport).slot_pitch();
        let size = self.m(viewport).slot;

        if let Some(row) = LEFT_COLUMN.iter().position(|entry| *entry == slot) {
            return UiRect::new(gear.x, gear.y + row as f32 * pitch, size, size);
        }
        if let Some(row) = RIGHT_COLUMN.iter().position(|entry| *entry == slot) {
            return UiRect::new(gear.right() - size, gear.y + row as f32 * pitch, size, size);
        }
        let (row, column, count) = match RING_ROW.iter().position(|entry| *entry == slot) {
            Some(column) => (0.0, column, RING_ROW.len()),
            None => (
                1.0,
                TRINKET_ROW
                    .iter()
                    .position(|entry| *entry == slot)
                    .unwrap_or(0),
                TRINKET_ROW.len(),
            ),
        };
        let row_width = count as f32 * pitch - self.m(viewport).slot_gap;
        let start = gear.x + (gear.width - row_width) * 0.5;
        UiRect::new(
            start + column as f32 * pitch,
            gear.bottom() + self.m(viewport).jewelry_gap + row * pitch,
            size,
            size,
        )
    }

    /// The bag card: its heading and the grid inside it.
    #[must_use]
    pub fn inventory_card(&self, viewport: (u32, u32)) -> UiRect {
        let right = self.right_rect(viewport);
        let rows = INVENTORY_SLOTS.div_ceil(INVENTORY_COLUMNS);
        let height = self.m(viewport).bag_heading + rows as f32 * self.m(viewport).cell_pitch()
            - self.m(viewport).cell_gap
            + self.m(viewport).padding * 0.7;
        UiRect::new(right.x, right.y, right.width, height)
    }

    /// The bag's grid.
    #[must_use]
    pub fn inventory_rect(&self, viewport: (u32, u32)) -> UiRect {
        let card = self.inventory_card(viewport);
        let pitch = self.m(viewport).cell_pitch();
        let rows = INVENTORY_SLOTS.div_ceil(INVENTORY_COLUMNS);
        let width = INVENTORY_COLUMNS as f32 * pitch - self.m(viewport).cell_gap;
        let height = rows as f32 * pitch - self.m(viewport).cell_gap;
        UiRect::new(
            card.x + (card.width - width) * 0.5,
            card.y + self.m(viewport).bag_heading,
            width,
            height,
        )
    }

    /// The card under the bag: derived numbers and what the gear adds.
    #[must_use]
    pub fn derived_rect(&self, viewport: (u32, u32)) -> UiRect {
        let right = self.right_rect(viewport);
        let card = self.inventory_card(viewport);
        let top = card.bottom() + self.m(viewport).card_gap;
        UiRect::new(right.x, top, right.width, (right.bottom() - top).max(0.0))
    }

    /// Where the bag cell at `index` sits.
    #[must_use]
    pub fn cell_rect(&self, index: usize, viewport: (u32, u32)) -> UiRect {
        let grid = self.inventory_rect(viewport);
        let pitch = self.m(viewport).cell_pitch();
        let column = index % INVENTORY_COLUMNS;
        let row = index / INVENTORY_COLUMNS;
        UiRect::new(
            grid.x + column as f32 * pitch,
            grid.y + row as f32 * pitch,
            self.m(viewport).cell,
            self.m(viewport).cell,
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

/// How the character panel is coloured. Most of the panel takes the kit's colours; these are
/// the few the sheet decides for itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterTheme {
    /// The wash over the game behind the panel.
    pub scrim: UiColor,
    /// The outline on a slot an item in the air may be dropped on.
    pub valid: UiColor,
    /// The outline on a slot it may not.
    pub invalid: UiColor,
    /// The outline on the slot under the pointer.
    pub hover: UiColor,
    /// The main stat, which is printed larger than the rest.
    pub main_stat: UiColor,
}

impl Default for CharacterTheme {
    /// The palette the tooltips and the dungeon map already use, so the character sheet reads
    /// as another window of the same interface rather than as a new one.
    fn default() -> Self {
        Self {
            scrim: color::SCRIM,
            valid: color::VERDANT,
            invalid: color::CRIMSON,
            hover: tokens::ACCENT,
            main_stat: color::BRONZE_BRIGHT,
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
        ItemIcon::TrinketWard => UiIcon::TrinketWard,
        ItemIcon::TrinketDispel => UiIcon::TrinketDispel,
        ItemIcon::TrinketHaste => UiIcon::TrinketHaste,
        ItemIcon::TrinketAbsorb => UiIcon::TrinketAbsorb,
        ItemIcon::Gem => UiIcon::Gem,
        ItemIcon::GemRuby => UiIcon::GemRuby,
        ItemIcon::GemAmber => UiIcon::GemAmber,
        ItemIcon::GemTopaz => UiIcon::GemTopaz,
        ItemIcon::GemEmerald => UiIcon::GemEmerald,
        ItemIcon::GemAmethyst => UiIcon::GemAmethyst,
        ItemIcon::GemOnyx => UiIcon::GemOnyx,
        ItemIcon::GemOpal => UiIcon::GemOpal,
        ItemIcon::BookTalent => UiIcon::BookTalent,
        ItemIcon::BookEnhancement => UiIcon::BookEnhancement,
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

/// The average item level of what is worn. An empty slot is not counted against the
/// character: the number says how good the gear is, and the empty slots say what is missing.
#[must_use]
pub fn average_item_level(character: &CharacterView) -> f32 {
    let (total, count) = character
        .equipment
        .iter()
        .flatten()
        .filter_map(|stack| stack.def())
        .fold((0.0_f32, 0_u32), |(total, count), def| {
            (total + f32::from(def.item_level), count + 1)
        });
    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

/// What the sheet's header says while the character is committed to a run.
pub const LOCKED_BANNER: &str = "Locked for this run";

/// Draws the character sheet, if it is open.
///
/// `scratch` is the caller's, reused: the panel formats a dozen numbers every frame it is up
/// and formatting them would otherwise allocate once a frame each. `tooltip` is likewise the
/// caller's, so hovering an item rewrites strings that already exist.
pub fn push_character_menu(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    snapshot: &UiSnapshot,
    theme: &CharacterTheme,
    scratch: &mut String,
    tooltip: &mut ItemTooltip,
    viewport: (u32, u32),
) {
    if !menu.is_open() {
        return;
    }
    let kit = Kit::new(menu.metrics_for(viewport).scale);
    let panel = menu.panel_rect(viewport);
    let character = &snapshot.character;

    list.rect(
        UiRect::new(0.0, 0.0, viewport.0 as f32, viewport.1 as f32),
        theme.scrim,
    );
    kit::panel(list, &kit, panel, color::BORDER_STRONG);

    push_header(list, menu, &kit, character, scratch, viewport);
    push_stats_column(list, menu, &kit, character, scratch, viewport);
    push_portrait(list, menu, &kit, character, viewport);
    push_slots(list, menu, &kit, character, theme, viewport);
    push_inventory(list, menu, &kit, character, theme, scratch, viewport);
    push_derived_card(list, menu, &kit, snapshot, scratch, viewport);
    push_footer(list, menu, &kit, viewport);

    // Above everything: what the pointer is holding, then what it is over.
    push_dragged(list, menu, &kit, viewport);
    push_tooltip(list, menu, &kit, character, tooltip, viewport);
}

/// A card: a raised rounded surface with a hairline edge.
fn card(list: &mut UiDrawList, kit: &Kit, rect: UiRect) {
    let corner = kit.px(radius::CARD);
    list.panel(rect, corner, color::SURFACE_2, color::SURFACE_1);
    list.panel_stroke(rect, corner, 1.0, color::BORDER);
}

/// The heading: what this window is, who it is about and how well equipped they are.
fn push_header(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = &menu.metrics_for(viewport);
    let panel = menu.panel_rect(viewport);
    let centre = panel.y + metrics.header * 0.5 + kit.px(2.0);

    kit.text(
        list,
        panel.x + metrics.padding,
        centre,
        "Character",
        type_scale::TITLE,
        color::TEXT_TITLE,
        TextAlign::Left,
    );
    if menu.is_locked() {
        kit.text(
            list,
            panel.x + panel.width - metrics.padding,
            centre,
            LOCKED_BANNER,
            type_scale::NAME,
            color::AMBER,
            TextAlign::Right,
        );
    }

    // The hero, in their colour, and what they are, quieter after it.
    let tint = hero_tint(character.hero);
    scratch.clear();
    push_title_case(scratch, character.hero.name());
    let name_x = panel.x + metrics.padding + metrics.stats_width + metrics.column_gap;
    kit.text(
        list,
        name_x,
        centre,
        scratch,
        type_scale::NAME,
        tint,
        TextAlign::Left,
    );
    let after = name_x + kit.width(type_scale::NAME, scratch) + kit.px(12.0);
    scratch.clear();
    push_title_case(scratch, character.hero.description());
    kit.text(
        list,
        after,
        centre,
        scratch,
        type_scale::UI,
        color::TEXT_MUTED,
        TextAlign::Left,
    );

    // Gear score, right-aligned: a number a player compares.
    let level = average_item_level(character);
    scratch.clear();
    write_one_decimal(scratch, level);
    let right = panel.right() - metrics.padding;
    kit.text(
        list,
        right,
        centre,
        scratch,
        type_scale::ENCOUNTER,
        color::BRONZE_BRIGHT,
        TextAlign::Right,
    );
    let number = kit.width(type_scale::ENCOUNTER, scratch);
    kit.text(
        list,
        right - number - kit.px(10.0),
        centre,
        "Item level",
        type_scale::UI,
        color::TEXT_SECONDARY,
        TextAlign::Right,
    );

    list.rect(
        UiRect::new(
            panel.x + metrics.padding,
            panel.y + metrics.header - kit.px(6.0),
            panel.width - metrics.padding * 2.0,
            1.0,
        ),
        color::BORDER,
    );
}

/// The left column: the primary stats, then the secondary ones, then the defensive.
fn push_stats_column(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = &menu.metrics_for(viewport);
    let column = menu.stats_rect(viewport);
    card(list, kit, column);

    let inner = column.inset(kit.px(14.0));
    let mut y = inner.y;

    // Primary. The main stat first and larger: it is the number the character is built on.
    y += kit::section(list, kit, inner.x, y, inner.width, "PRIMARY");
    scratch.clear();
    push_title_case(scratch, character.main_stat.name());
    let main_centre = y + metrics.row * 0.6;
    kit.text(
        list,
        inner.x,
        main_centre,
        scratch,
        type_scale::NAME,
        color::TEXT_PRIMARY,
        TextAlign::Left,
    );
    scratch.clear();
    write_number(scratch, character.total.get(character.main_stat));
    kit.text(
        list,
        inner.right(),
        main_centre,
        scratch,
        type_scale::TITLE,
        color::BRONZE_BRIGHT,
        TextAlign::Right,
    );
    y += metrics.row * 1.3;
    push_stat_row(
        list,
        kit,
        UiRect::new(inner.x, y, inner.width, metrics.row),
        Stat::Stamina,
        &character.total,
        scratch,
    );
    y += metrics.row + kit.px(10.0);

    for (title, group) in [
        ("SECONDARY", StatGroup::Secondary),
        ("DEFENSIVE", StatGroup::Defensive),
    ] {
        y += kit::section(list, kit, inner.x, y, inner.width, title);
        for stat in Stat::ALL {
            if stat.group() != group {
                continue;
            }
            push_stat_row(
                list,
                kit,
                UiRect::new(inner.x, y, inner.width, metrics.row),
                stat,
                &character.total,
                scratch,
            );
            y += metrics.row;
        }
        y += kit.px(10.0);
    }
}

/// One row of a stat list: its name on the left, what it is worth on the right.
///
/// A rating prints the percentage it is worth first and the raw rating after it in muted
/// text, like `Haste   1.96%  (2)`, because the percentage is what combat multiplies by and
/// the rating is only what gear adds up to. Armour prints its flat value and the reduction it
/// buys.
fn push_stat_row(
    list: &mut UiDrawList,
    kit: &Kit,
    row: UiRect,
    stat: Stat,
    stats: &StatBlock,
    scratch: &mut String,
) {
    let value = stats.get(stat);
    let lit = value > 0.0;
    let centre = row.y + row.height * 0.5;
    let ink = if lit {
        color::TEXT_PRIMARY
    } else {
        color::TEXT_MUTED
    };

    scratch.clear();
    push_title_case(scratch, stat.name());
    kit.text(
        list,
        row.x,
        centre,
        scratch,
        type_scale::BODY,
        if lit { color::TEXT_SECONDARY } else { ink },
        TextAlign::Left,
    );

    if stat.is_rating() || stat == Stat::Armor {
        // The muted raw value on the far right, the meaningful one before it.
        scratch.clear();
        scratch.push('(');
        write_number(scratch, value);
        scratch.push(')');
        let raw = kit.width(type_scale::UI, scratch);
        kit.text(
            list,
            row.right(),
            centre,
            scratch,
            type_scale::UI,
            color::TEXT_MUTED,
            TextAlign::Right,
        );
        scratch.clear();
        if stat == Stat::Armor {
            write_percent(scratch, armor_reduction(value));
        } else {
            write_percent(scratch, stats.percent(stat));
        }
        kit.text(
            list,
            row.right() - raw - kit.px(8.0),
            centre,
            scratch,
            type_scale::BODY,
            ink,
            TextAlign::Right,
        );
    } else {
        scratch.clear();
        write_number(scratch, value);
        kit.text(
            list,
            row.right(),
            centre,
            scratch,
            type_scale::BODY,
            ink,
            TextAlign::Right,
        );
    }
}

/// The right-hand card under the bag: the numbers the stats produce, and what the gear adds.
fn push_derived_card(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    snapshot: &UiSnapshot,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let metrics = &menu.metrics_for(viewport);
    let rect = menu.derived_rect(viewport);
    if rect.height < metrics.row * 3.0 {
        return;
    }
    card(list, kit, rect);
    let character = &snapshot.character;
    let inner = rect.inset(kit.px(14.0));
    let half = (inner.width - kit.px(24.0)) * 0.5;
    let left = UiRect::new(inner.x, inner.y, half, inner.height);
    let right = UiRect::new(inner.right() - half, inner.y, half, inner.height);

    // Derived: every row here is a value some system reads, printed where a player can see it.
    let mut y = left.y + kit::section(list, kit, left.x, left.y, left.width, "DERIVED");
    let derived = |list: &mut UiDrawList, label: &str, value: &str, y: &mut f32| {
        let centre = *y + metrics.row * 0.5;
        kit.text(
            list,
            left.x,
            centre,
            label,
            type_scale::BODY,
            color::TEXT_SECONDARY,
            TextAlign::Left,
        );
        kit.text(
            list,
            left.right(),
            centre,
            value,
            type_scale::BODY,
            color::TEXT_PRIMARY,
            TextAlign::Right,
        );
        *y += metrics.row;
    };
    scratch.clear();
    write_number(scratch, character.max_health);
    derived(list, "Health", scratch, &mut y);
    scratch.clear();
    write_number(scratch, snapshot.spell_power);
    derived(list, "Spell power", scratch, &mut y);
    scratch.clear();
    write_percent(scratch, snapshot.crit_chance);
    derived(list, "Crit chance", scratch, &mut y);
    scratch.clear();
    write_percent(scratch, snapshot.haste);
    derived(list, "Haste", scratch, &mut y);
    scratch.clear();
    write_seconds(
        scratch,
        engine_game::firemage::GLOBAL_COOLDOWN / (1.0 + snapshot.haste),
    );
    derived(list, "Global cooldown", scratch, &mut y);
    // The Spirit buff, when it is running.
    if character.spirit_stacks > 0 {
        scratch.clear();
        write_integer(scratch, i64::from(character.spirit_stacks));
        scratch.push_str(" stacks");
        derived(list, "Spirit", scratch, &mut y);
    }

    // What the gear is contributing: the difference the sheet already reports between base
    // and total, not a sum the panel makes up.
    let mut y = right.y + kit::section(list, kit, right.x, right.y, right.width, "FROM EQUIPMENT");
    if character.gear.is_empty() {
        kit.text(
            list,
            right.x,
            y + metrics.row * 0.5,
            "No combat stats",
            type_scale::BODY,
            color::TEXT_MUTED,
            TextAlign::Left,
        );
    } else {
        for (stat, value) in character.gear.entries() {
            scratch.clear();
            scratch.push('+');
            write_number(scratch, value);
            scratch.push(' ');
            push_title_case(scratch, stat.display_for(character.hero));
            kit.text(
                list,
                right.x,
                y + metrics.row * 0.5,
                scratch,
                type_scale::BODY,
                color::VERDANT,
                TextAlign::Left,
            );
            y += metrics.row;
        }
    }

    // The sets the gear forms: each with a bonus on, and every tier that is on. Derived from
    // the worn items by the same function the server uses; the panel only prints it.
    let sets = active_sets_of(&character.equipment);
    if sets.iter().any(|active| active.tiers_active() > 0) {
        y += kit::section(list, kit, right.x, y, right.width, "SET BONUSES");
        for active in sets.iter().filter(|active| active.tiers_active() > 0) {
            scratch.clear();
            scratch.push_str(&active.set.name);
            scratch.push_str(" (");
            write_integer(scratch, i64::from(active.equipped));
            scratch.push('/');
            write_integer(scratch, active.set.pieces.len() as i64);
            scratch.push(')');
            let mut rows = vec![(color::BRONZE_BRIGHT, scratch.clone())];
            for bonus in active.active_bonuses() {
                scratch.clear();
                write_set_bonus(scratch, bonus, character.hero);
                rows.extend(
                    scratch
                        .split('\n')
                        .map(|line| (color::VERDANT, line.to_owned())),
                );
            }
            for (ink, line) in rows {
                if y + metrics.row > inner.bottom() {
                    return;
                }
                kit.text(
                    list,
                    right.x,
                    y + metrics.row * 0.5,
                    &line,
                    type_scale::BODY,
                    ink,
                    TextAlign::Left,
                );
                y += metrics.row;
            }
        }
    }
}

/// The character themselves, standing between their gear.
///
/// A silhouette built from rectangles, tinted by class. It is a *placeholder for the model*,
/// and deliberately a readable one: a panel with a hole where the character should be reads as
/// broken, and rendering the actual skinned model into a texture is a renderer feature rather
/// than a layout one.
fn push_portrait(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    viewport: (u32, u32),
) {
    let frame = menu.portrait_rect(viewport);
    let tint = hero_tint(character.hero);
    card(list, kit, frame);
    // A soft pool of the hero's colour behind them.
    list.glow(
        frame.inset(kit.px(10.0)),
        kit.px(radius::CARD),
        kit.px(36.0),
        color::alpha(tint, 0.16),
    );

    let figure = [0.30, 0.34, 0.42, 1.0];
    let shade = [0.20, 0.23, 0.29, 1.0];
    let floor_height = frame.height * 0.05;
    list.rect(
        UiRect::new(
            frame.x + frame.width * 0.14,
            frame.bottom() - floor_height * 3.2,
            frame.width * 0.72,
            floor_height,
        ),
        [0.10, 0.11, 0.14, 1.0],
    );

    let centre = frame.x + frame.width * 0.5;
    let unit = frame.height * 0.0088;
    let top = frame.y + frame.height * 0.08;
    let head = unit * 9.0;
    list.rect(UiRect::new(centre - head * 0.5, top, head, head), figure);
    let torso_w = unit * 20.0;
    let torso_h = unit * 26.0;
    let torso_y = top + head + unit * 2.0;
    list.rect(
        UiRect::new(centre - torso_w * 0.5, torso_y, torso_w, torso_h),
        figure,
    );
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
        shade,
    );
    list.rect(
        UiRect::new(centre + torso_w * 0.5, torso_y + unit * 3.0, arm_w, arm_h),
        shade,
    );
    let leg_w = unit * 8.0;
    let leg_h = unit * 30.0;
    let leg_y = torso_y + torso_h;
    list.rect(
        UiRect::new(centre - leg_w - unit, leg_y, leg_w, leg_h),
        figure,
    );
    list.rect(UiRect::new(centre + unit, leg_y, leg_w, leg_h), shade);

    kit.text(
        list,
        centre,
        frame.bottom() - kit.px(18.0),
        match character.hero {
            Hero::FireMage => "Fire Mage",
            Hero::Warrior => "Warrior",
            Hero::Druid => "Druid",
        },
        type_scale::HEADING,
        tint,
        TextAlign::Center,
    );
}

/// Every equipment slot.
fn push_slots(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    theme: &CharacterTheme,
    viewport: (u32, u32),
) {
    for worn in EquipSlot::ALL {
        let rect = menu.equipment_rect(worn, viewport);
        push_slot(
            list,
            menu,
            kit,
            character,
            ItemSlot::Equipment(worn),
            rect,
            theme,
        );
    }
}

/// The bag card: a heading with the count, then the grid.
fn push_inventory(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    theme: &CharacterTheme,
    scratch: &mut String,
    viewport: (u32, u32),
) {
    let card_rect = menu.inventory_card(viewport);
    card(list, kit, card_rect);
    let inner_x = card_rect.x + kit.px(14.0);
    let width = card_rect.width - kit.px(28.0);
    kit::section(
        list,
        kit,
        inner_x,
        card_rect.y + kit.px(10.0),
        width,
        "INVENTORY",
    );

    scratch.clear();
    write_integer(
        scratch,
        i64::try_from(character.inventory.iter().flatten().count()).unwrap_or(0),
    );
    scratch.push_str(" / ");
    write_integer(scratch, i64::try_from(INVENTORY_SLOTS).unwrap_or(0));
    kit.text(
        list,
        card_rect.right() - kit.px(14.0),
        card_rect.y + kit.px(10.0) + kit.px(11.0),
        scratch,
        type_scale::UI,
        color::TEXT_SECONDARY,
        TextAlign::Right,
    );

    for index in 0..INVENTORY_SLOTS {
        let slot = ItemSlot::Inventory(index);
        let rect = menu.cell_rect(index, viewport);
        push_slot(list, menu, kit, character, slot, rect, theme);
    }
}

/// One slot, worn or carried: its frame, what is in it, and how the pointer feels about it.
fn push_slot(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    slot: ItemSlot,
    rect: UiRect,
    theme: &CharacterTheme,
) {
    let held = character.at(slot);
    let dragged_from = menu.drag().is_some_and(|drag| drag.from == slot);
    let shown = held.filter(|_| !dragged_from);
    let corner = kit.px(radius::CONTROL);
    let hovered = menu.hovered() == Some(slot);

    // The surface: a well when empty, a lit pocket tinted by the item's quality when full.
    match shown.and_then(ItemStack::def) {
        Some(def) => {
            let quality = quality_color(def.quality);
            list.panel(
                rect,
                corner,
                blend(tokens::CONTROL, quality, 0.22),
                blend(tokens::WELL, quality, 0.08),
            );
        }
        None => list.panel(rect, corner, tokens::WELL, tokens::WELL),
    }

    // What is in it. The item being dragged is drawn under the pointer instead, so its own
    // slot reads as empty for as long as it is in the air.
    if let Some(stack) = shown {
        if let Some(def) = stack.def() {
            list.icon(
                rect.inset(rect.width * 0.14),
                icon_for_item(def.icon),
                [1.0, 1.0, 1.0, 1.0],
            );
            // Item level in the corner: the number a player scans a bag for.
            let mut text = String::with_capacity(4);
            if stack.quantity > 1 {
                write_integer(&mut text, i64::from(stack.quantity));
            } else {
                write_integer(&mut text, i64::from(def.item_level));
            }
            let badge_w = kit.width(type_scale::BADGE, &text) + kit.px(8.0);
            let badge_h = kit.px(16.0);
            let badge = UiRect::new(
                rect.right() - badge_w - kit.px(3.0),
                rect.bottom() - badge_h - kit.px(3.0),
                badge_w,
                badge_h,
            );
            list.panel(
                badge,
                kit.px(4.0),
                [0.0, 0.0, 0.0, 0.62],
                [0.0, 0.0, 0.0, 0.62],
            );
            kit.text(
                list,
                badge.x + badge.width * 0.5,
                badge.y + badge.height * 0.5,
                &text,
                type_scale::BADGE,
                if stack.quantity > 1 {
                    color::TEXT_PRIMARY
                } else {
                    color::alpha(quality_color(def.quality), 1.0)
                },
                TextAlign::Center,
            );
        }
    } else if let ItemSlot::Equipment(worn) = slot {
        // An empty equipment slot shows what belongs there, dimmed, and says its name. A bag
        // cell shows nothing, because nothing in particular belongs in it.
        let hint = rect.inset(rect.width * 0.2);
        list.icon(
            UiRect::new(hint.x, hint.y - kit.px(6.0), hint.width, hint.height),
            icon_for_item(ItemIcon::of_family(worn.family())),
            [1.0, 1.0, 1.0, 0.16],
        );
        let mut label = String::with_capacity(12);
        push_title_case(&mut label, worn.name());
        kit.text(
            list,
            rect.x + rect.width * 0.5,
            rect.bottom() - kit.px(11.0),
            &label,
            type_scale::LABEL,
            color::TEXT_MUTED,
            TextAlign::Center,
        );
    }

    // The edge: quality when full, a hairline when empty; while something is in the air
    // every slot says whether it would take it; the pointer's slot is lit.
    let (width, edge) = match menu.drag() {
        Some(drag) if drag.from != slot => {
            if character.can_move(drag.from, slot) {
                (if hovered { 2.5 } else { 1.5 }, theme.valid)
            } else if hovered {
                (2.5, theme.invalid)
            } else {
                (1.0, color::BORDER)
            }
        }
        _ if menu.refused() == Some(slot) => (2.0, theme.invalid),
        _ if hovered => (2.0, theme.hover),
        _ => match shown.and_then(ItemStack::def) {
            Some(def) => (2.0, color::alpha(quality_color(def.quality), 0.85)),
            None => (1.0, color::BORDER),
        },
    };
    if hovered && menu.drag().is_none() {
        list.glow(rect, corner, kit.px(10.0), color::alpha(theme.hover, 0.35));
    }
    list.panel_stroke(rect, corner, kit.px(width).max(1.0), edge);
}

/// The item in the air, drawn under the pointer.
fn push_dragged(list: &mut UiDrawList, menu: &CharacterMenu, kit: &Kit, viewport: (u32, u32)) {
    let Some(drag) = menu.drag() else {
        return;
    };
    let Some(def) = drag.stack.def() else {
        return;
    };
    let metrics = &menu.metrics_for(viewport);
    let (x, y) = drag.icon_origin();
    let ghost = UiRect::new(x, y, metrics.slot, metrics.slot);
    let corner = kit.px(radius::CONTROL);
    let quality = quality_color(def.quality);
    list.shadow(ghost, corner, kit.px(16.0), [0.0, 0.0, 0.0, 0.6]);
    list.panel(
        ghost,
        corner,
        blend(tokens::CONTROL_HOVER, quality, 0.25),
        blend(tokens::CONTROL, quality, 0.1),
    );
    list.icon(
        ghost.inset(ghost.width * 0.14),
        icon_for_item(def.icon),
        [1.0, 1.0, 1.0, 0.92],
    );
    list.panel_stroke(ghost, corner, kit.px(2.0), quality);
}

/// The tooltip for whatever the pointer is over, docked beside that slot.
fn push_tooltip(
    list: &mut UiDrawList,
    menu: &CharacterMenu,
    kit: &Kit,
    character: &CharacterView,
    tooltip: &mut ItemTooltip,
    viewport: (u32, u32),
) {
    // Nothing while an item is in the air: the player is looking at where it is going.
    if menu.drag().is_some() {
        return;
    }
    let Some(slot) = menu.hovered() else {
        return;
    };
    let Some((stack, context)) = ItemContext::for_slot(character, slot) else {
        return;
    };
    tooltip.push(
        list,
        kit,
        stack,
        character.hero,
        &context,
        menu.slot_rect(slot, viewport),
        viewport,
    );
}

/// The line along the bottom that says how the panel is worked.
fn push_footer(list: &mut UiDrawList, menu: &CharacterMenu, kit: &Kit, viewport: (u32, u32)) {
    let metrics = &menu.metrics_for(viewport);
    let panel = menu.panel_rect(viewport);
    kit.text(
        list,
        panel.x + metrics.padding,
        panel.bottom() - metrics.padding - metrics.footer * 0.25,
        if menu.is_locked() {
            "Equipment is read-only while you are queued, in a party or in a dungeon  -  Esc closes"
        } else {
            "Drag an item onto a slot to equip it  -  Right-click to equip or unequip  -  Esc closes"
        },
        type_scale::META,
        color::TEXT_MUTED,
        TextAlign::Left,
    );
}

/// Writes a stat value: whole when it is whole, one decimal when it is not.
fn write_number(out: &mut String, value: f32) {
    use core::fmt::Write as _;
    if (value - value.round()).abs() < 0.05 {
        let _ = write!(out, "{}", value.round() as i64);
    } else {
        let _ = write!(out, "{value:.1}");
    }
}

/// Writes a number to one decimal place.
fn write_one_decimal(out: &mut String, value: f32) {
    use core::fmt::Write as _;
    let _ = write!(out, "{value:.1}");
}

/// Writes a fraction as a percentage with two decimals: `0.15` becomes `15.00%`.
fn write_percent(out: &mut String, fraction: f32) {
    use core::fmt::Write as _;
    let _ = write!(out, "{:.2}%", fraction * 100.0);
}

/// Writes a duration in seconds, to two decimals.
fn write_seconds(out: &mut String, seconds: f32) {
    use core::fmt::Write as _;
    let _ = write!(out, "{seconds:.2} s");
}

/// Keeps [`TextStyle`] in the signature of the helpers above without a lint: the sheet's own
/// type choices are all named tokens.
#[allow(dead_code, reason = "documents the token type the sheet draws with")]
type Style = TextStyle;

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
        let mut simulation = crate::snapshot::dressed_training_arena();
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
        let mut tooltip = ItemTooltip::new();
        push_character_menu(
            &mut list,
            menu,
            snapshot,
            &CharacterTheme::default(),
            &mut scratch,
            &mut tooltip,
            VIEWPORT,
        );
        list
    }

    /// Every run of text, in either face.
    fn texts(list: &UiDrawList) -> Vec<String> {
        list.commands()
            .iter()
            .filter_map(|command| match command {
                UiCommand::Text { range, .. } | UiCommand::Label { range, .. } => {
                    Some(list.text_of(*range).to_owned())
                }
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

    fn right_click(x: f32, y: f32) -> UiInput {
        UiInput {
            secondary_pressed: true,
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
    fn slots_are_sixty_four_pixels_at_scale_one() {
        let menu = open();
        assert_eq!(menu.metrics().slot, 64.0);
        assert_eq!(menu.equipment_rect(EquipSlot::Head, VIEWPORT).width, 64.0);
    }

    #[test]
    fn all_thirteen_slots_are_drawn_inside_the_panel_and_none_overlaps_another() {
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
        assert_eq!(rects.len(), 13);
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
        let card = menu.inventory_card(VIEWPORT);
        for index in 0..INVENTORY_SLOTS {
            let rect = menu.cell_rect(index, VIEWPORT);
            assert!(rect.x >= panel.x && rect.right() <= panel.right() + 0.01);
            assert!(
                rect.bottom() <= panel.bottom() + 0.01,
                "row {index} escaped"
            );
            assert!(
                rect.x >= card.x && rect.right() <= card.right(),
                "cell {index} is outside its card"
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
    fn the_stats_panel_lists_the_active_set_bonuses_and_only_those() {
        use engine_game::character::item_by_key;
        engine_realm::loot::ensure_items_registered();
        let piece = |key: &str| ItemStack::one(item_by_key(key).expect("a set piece").id);
        let mut snapshot = snapshot();
        snapshot.character.equipment[EquipSlot::Head.index()] = Some(piece("emberwake_hood"));
        let one = texts(&render(&open(), &snapshot));
        assert!(!one.iter().any(|text| text.contains("SET BONUSES")), "{one:?}");

        snapshot.character.equipment[EquipSlot::Shoulder.index()] =
            Some(piece("emberwake_mantle"));
        snapshot.character.gear = StatBlock::of(Stat::Haste, 4.0);
        let two = texts(&render(&open(), &snapshot));
        assert!(two.contains(&"SET BONUSES".to_owned()), "{two:?}");
        assert!(two.contains(&"Emberwake Regalia (2/5)".to_owned()), "{two:?}");
        assert!(two.iter().any(|text| text.starts_with("(2) +6 Critical Strike")));
        assert!(!two.iter().any(|text| text.starts_with("(4)")), "tier 4 is not earned");
    }

    #[test]
    fn the_panel_names_the_hero_the_stats_and_every_slot() {
        let snapshot = snapshot();
        let drawn = texts(&render(&open(), &snapshot));

        assert!(drawn.contains(&"Character".to_owned()), "{drawn:?}");
        assert!(
            drawn.iter().any(|text| text.contains("Firemage")),
            "{drawn:?}"
        );
        for heading in ["PRIMARY", "SECONDARY", "DEFENSIVE", "DERIVED", "INVENTORY"] {
            assert!(drawn.contains(&heading.to_owned()), "{heading} missing");
        }
        for stat in Stat::ALL {
            if stat == Stat::Strength {
                continue; // Not the FireMage's; only the main stat and the ratings show.
            }
            let mut name = String::new();
            push_title_case(&mut name, stat.name());
            assert!(drawn.contains(&name), "{name} is missing: {drawn:?}");
        }
        // Empty slots say what they are; worn ones are explained by their tooltip.
        let mut label = String::new();
        push_title_case(&mut label, EquipSlot::Ring1.name());
        assert!(drawn.contains(&label), "an empty slot has no label");
    }

    #[test]
    fn the_header_reports_the_average_item_level() {
        let snapshot = snapshot();
        let level = average_item_level(&snapshot.character);
        let mut expected = String::new();
        write_one_decimal(&mut expected, level);
        let drawn = texts(&render(&open(), &snapshot));
        assert!(drawn.contains(&expected), "{expected} not in {drawn:?}");
        assert!(drawn.contains(&"Item level".to_owned()));
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
            let name = |stat: Stat| {
                let mut out = String::new();
                push_title_case(&mut out, stat.name());
                out
            };
            assert!(drawn.contains(&name(expected)), "{hero:?}");
            assert!(
                !drawn.contains(&name(absent)),
                "{hero:?} shows {}",
                absent.name()
            );
        }
    }

    #[test]
    fn a_rating_prints_its_percentage_first_and_the_rating_in_brackets() {
        let snapshot = snapshot();
        let drawn = texts(&render(&open(), &snapshot));
        assert!(
            drawn.iter().any(|text| text == "0.00%"),
            "a rating is not printed as a percentage: {drawn:?}"
        );
        assert!(drawn.iter().any(|text| text == "(0)"), "{drawn:?}");
    }

    #[test]
    fn the_worn_items_draw_their_art_and_the_empty_slots_draw_a_hint() {
        let snapshot = snapshot();
        let drawn = icons(&render(&open(), &snapshot));
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
    fn hovering_an_item_docks_its_tooltip_beside_the_slot() {
        let snapshot = snapshot();
        let mut menu = open();
        let chest = menu.equipment_rect(EquipSlot::Chest, VIEWPORT);
        let (x, y) = centre(chest);
        menu.update(&at(x, y), &snapshot, VIEWPORT);
        assert_eq!(menu.hovered(), Some(ItemSlot::Equipment(EquipSlot::Chest)));

        let list = render(&menu, &snapshot);
        let def = starting_item(ItemFamily::Chest).expect("a chestpiece");
        let mut name = String::new();
        push_title_case(&mut name, def.name);
        let drawn = texts(&list);
        assert!(drawn.contains(&name), "{drawn:?}");
        assert!(
            drawn.contains(&"Grants no combat stats".to_owned()),
            "{drawn:?}"
        );
        assert!(drawn.iter().any(|text| text.starts_with("Equipped")));
        // Beside the slot: the title is drawn to its right, vertically within reach of it.
        let title_x = list
            .commands()
            .iter()
            .find_map(|command| match command {
                UiCommand::Label { origin, range, .. } if list.text_of(*range) == name => {
                    Some(origin.0)
                }
                _ => None,
            })
            .expect("the title is drawn");
        assert!(
            title_x > chest.right(),
            "{title_x} is not right of {chest:?}"
        );
    }

    #[test]
    fn hovering_an_empty_slot_shows_no_tooltip() {
        let snapshot = snapshot();
        let mut menu = open();
        let (x, y) = centre(menu.equipment_rect(EquipSlot::Ring1, VIEWPORT));
        menu.update(&at(x, y), &snapshot, VIEWPORT);
        let drawn = texts(&render(&menu, &snapshot));
        assert!(!drawn.contains(&"Grants no combat stats".to_owned()));
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
    fn right_clicking_a_bag_item_equips_it_and_a_worn_one_unequips_it() {
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
        let (x, y) = centre(menu.cell_rect(index, VIEWPORT));
        let outcome = menu.update(&right_click(x, y), &snapshot, VIEWPORT);
        let CharacterMenuOutcome::Move { from, to } = outcome else {
            panic!("right-click did nothing: {outcome:?}");
        };
        assert_eq!(from, ItemSlot::Inventory(index));
        assert!(matches!(to, ItemSlot::Equipment(slot) if slot.family() == ItemFamily::Ring));

        let (x, y) = centre(menu.equipment_rect(EquipSlot::Feet, VIEWPORT));
        let outcome = menu.update(&right_click(x, y), &snapshot, VIEWPORT);
        assert!(matches!(
            outcome,
            CharacterMenuOutcome::Move {
                from: ItemSlot::Equipment(EquipSlot::Feet),
                to: ItemSlot::Inventory(_),
            }
        ));
    }

    #[test]
    fn a_locked_sheet_says_so_and_moves_nothing() {
        let snapshot = snapshot();
        let mut menu = open();
        menu.set_locked(true);
        assert!(texts(&render(&menu, &snapshot)).contains(&LOCKED_BANNER.to_owned()));
        let mut unlocked = open();
        unlocked.set_locked(false);
        assert!(!texts(&render(&unlocked, &snapshot)).contains(&LOCKED_BANNER.to_owned()));

        // Right-click on a worn item: nothing.
        let (x, y) = centre(menu.equipment_rect(EquipSlot::Feet, VIEWPORT));
        assert_eq!(
            menu.update(&right_click(x, y), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Idle
        );
        // A full drag from a worn item to the bag: no pick-up, no move.
        let free = snapshot
            .character
            .inventory
            .iter()
            .position(Option::is_none)
            .expect("a free bag cell");
        assert_eq!(
            menu.update(&press(x, y), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Idle
        );
        let (bx, by) = centre(menu.cell_rect(free, VIEWPORT));
        assert_eq!(
            menu.update(&release(bx, by), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Idle
        );
        // The same gesture unlocked does move it.
        let mut open_menu = open();
        open_menu.update(&press(x, y), &snapshot, VIEWPORT);
        assert!(matches!(
            open_menu.update(&release(bx, by), &snapshot, VIEWPORT),
            CharacterMenuOutcome::Move { .. }
        ));
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
        let (x, y) = centre(menu.equipment_rect(EquipSlot::Trinket2, VIEWPORT));
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

        for menu in [small, large] {
            let panel = menu.panel_rect(VIEWPORT);
            for slot in EquipSlot::ALL {
                assert!(menu.equipment_rect(slot, VIEWPORT).x >= panel.x);
            }
        }
    }

    #[test]
    fn the_panel_redraws_from_the_sheet_rather_than_from_what_was_dragged() {
        let mut simulation = crate::snapshot::dressed_training_arena();
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
        let card = menu.inventory_card(SMALLEST);
        assert!(card.bottom() <= panel.bottom(), "the bag is off the panel");
        for index in 0..INVENTORY_SLOTS {
            let cell = menu.cell_rect(index, SMALLEST);
            assert!(cell.bottom() <= card.bottom() && cell.right() <= panel.right());
        }
        for slot in EquipSlot::ALL {
            let rect = menu.equipment_rect(slot, SMALLEST);
            assert!(rect.bottom() <= panel.bottom() - menu.metrics().padding);
            assert!(rect.right() <= menu.right_rect(SMALLEST).x);
        }
        assert!(menu.derived_rect(SMALLEST).height > 150.0);
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

        // Moved across the panel, over no slot at all: the item stays in the air.
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
        assert_eq!(
            opaque_in_origin(&menu),
            1,
            "the item is drawn both in its slot and in the air"
        );
        let drag = menu.drag().expect("something is held");
        assert_eq!(drag.icon_origin(), (origin.x, origin.y));

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
            list.len() < 700,
            "the character sheet costs {} commands",
            list.len()
        );
    }
}
