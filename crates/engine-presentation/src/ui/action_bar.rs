//! Eight action bars: what is in each slot, which key presses it, and where it is drawn.
//!
//! ```text
//! HudConfig  <->  ActionBarSet  ->  SlotAddress + FireAbility  ->  PlayerCommand
//!                      |
//!                      v
//!                 slot rectangles, from the HudLayout's rectangle for the bar
//! ```
//!
//! ## This is a model, not a widget
//!
//! Nothing here draws, and nothing here reads a pointer. A slot knows which spell it holds
//! and which key presses it; who is hovering it, what colour it turns and whether its
//! cooldown has finished are all somewhere else. That separation is the whole point of the
//! brief's "the action bar knows slot -> spell and slot -> key rather than a widget holding
//! both": the set survives a resize, a scale change, a reload of the interface and a change
//! of resolution, because none of those are facts about it.
//!
//! ## It performs no gameplay either
//!
//! Pressing a slot produces a [`FireAbility`], and the application turns that into a
//! [`PlayerCommand`](engine_game::command::PlayerCommand) — through exactly the path the
//! existing bar already uses, including the one that opens a targeting circle for an aimed
//! ability instead of casting it. The bar is a lookup table with a layout.
//!
//! ## One key, one action
//!
//! [`ActionBarSet::assign_key`] unbinds the previous holder in the same call, so there is no
//! moment at which two slots claim one key. What the *interface* does about that — ask, or
//! refuse outright for a key the game already needs — is decided by
//! [`ActionBarSet::conflict`] before the assignment is made.

use engine_core::input::KeyCode;
use engine_game::firemage::FireAbility;
use engine_game::input_map::Bindings;
use engine_render_data::ui::UiRect;
use engine_settings::hud::{
    ACTION_BAR_SLOTS, ACTION_BARS, HudConfig, HudOrientation, HudSlotConfig,
};

/// Size of one slot, in logical pixels.
///
/// The same measurement the existing bar is drawn at, so a default layout is pixel for pixel
/// the bar the game already has.
pub const SLOT_SIZE: f32 = 48.0;

/// Gap between two slots, in logical pixels.
pub const SLOT_GAP: f32 = 6.0;

/// Which slot, on which bar.
///
/// A pair rather than a flat index, because everything that acts on a slot — the editor, the
/// menu, the conflict message — has to name the bar to the player anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlotAddress {
    /// Which bar, from zero.
    pub bar: usize,
    /// Which slot within it, from zero.
    pub slot: usize,
}

impl SlotAddress {
    /// The address of slot `slot` on bar `bar`, both counted from zero.
    #[must_use]
    pub const fn new(bar: usize, slot: usize) -> Self {
        Self { bar, slot }
    }

    /// Whether this names a slot that exists.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.bar < ACTION_BARS && self.slot < ACTION_BAR_SLOTS
    }

    /// Writes this address the way a player reads it: bar and slot counted from one.
    pub fn write_label(self, out: &mut String) {
        use std::fmt::Write as _;
        out.clear();
        let _ = write!(out, "BAR {} SLOT {}", self.bar + 1, self.slot + 1);
    }
}

/// What one slot holds.
///
/// Both halves are independent and both are optional. A spell with no key is an ordinary
/// mouse-only slot; a key with no spell is a binding the player set up before deciding what
/// to put there, and neither is a broken state worth refusing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActionBarSlot {
    /// The spell this slot casts.
    pub ability: Option<FireAbility>,
    /// The key that presses it.
    pub key: Option<KeyCode>,
}

impl ActionBarSlot {
    /// Whether the slot holds nothing at all.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.ability.is_none() && self.key.is_none()
    }
}

/// One bar's slots.
///
/// The placement, the orientation and whether the bar is shown all live in the
/// [`HudLayout`](super::hud_layout::HudLayout), because those are true of every arrangeable
/// element and this is the part that is only true of a bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionBar {
    slots: [ActionBarSlot; ACTION_BAR_SLOTS],
}

impl Default for ActionBar {
    fn default() -> Self {
        Self {
            slots: [ActionBarSlot::default(); ACTION_BAR_SLOTS],
        }
    }
}

impl ActionBar {
    /// Its slots, in slot order.
    #[must_use]
    pub fn slots(&self) -> &[ActionBarSlot; ACTION_BAR_SLOTS] {
        &self.slots
    }

    /// Whether every slot is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|slot| slot.is_empty())
    }
}

/// Why a key cannot simply be assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyConflict {
    /// Another action bar slot already has it. Replacing is offered.
    Slot(SlotAddress),
    /// The game itself needs it — movement, targeting, the menu. Replacing is not offered.
    ///
    /// Carries what the key does, so the interface can say *why* rather than only that it
    /// cannot. The string comes from [`Bindings::reserved`], which is the only place that
    /// knows.
    Reserved(&'static str),
}

/// All eight bars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionBarSet {
    bars: [ActionBar; ACTION_BARS],
}

impl Default for ActionBarSet {
    /// Eight empty bars.
    ///
    /// Empty rather than filled: what a *fresh install* starts with is
    /// [`ActionBarSet::with_default_bindings`], and the difference matters. A player who
    /// deliberately cleared bar one must get their empty bar back on the next launch, not
    /// the class's ten abilities helpfully put back.
    fn default() -> Self {
        Self {
            bars: [ActionBar::default(); ACTION_BARS],
        }
    }
}

impl ActionBarSet {
    /// The bars a new player starts with: bar one holding the class, the rest empty.
    ///
    /// Bar one is exactly what the game's single bar is today — the ten abilities of
    /// [`FireAbility::BAR`] on the digits [`Bindings::ability_key`] gives them — so a fresh
    /// install plays identically to the build before eight bars existed.
    #[must_use]
    pub fn with_default_bindings() -> Self {
        let mut set = Self::default();
        for slot in 0..ACTION_BAR_SLOTS {
            set.bars[0].slots[slot] = ActionBarSlot {
                ability: FireAbility::from_slot(slot),
                key: Bindings::ability_key(slot),
            };
        }
        set
    }

    /// The bars, in bar order.
    #[must_use]
    pub fn bars(&self) -> &[ActionBar; ACTION_BARS] {
        &self.bars
    }

    /// Bar `index`, if it exists.
    #[must_use]
    pub fn bar(&self, index: usize) -> Option<&ActionBar> {
        self.bars.get(index)
    }

    /// What is in one slot.
    #[must_use]
    pub fn slot(&self, address: SlotAddress) -> Option<ActionBarSlot> {
        self.bars
            .get(address.bar)
            .and_then(|bar| bar.slots.get(address.slot))
            .copied()
    }

    /// Every slot with something in it, in bar and slot order.
    pub fn occupied(&self) -> impl Iterator<Item = (SlotAddress, ActionBarSlot)> + '_ {
        self.bars.iter().enumerate().flat_map(|(bar, contents)| {
            contents
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| !slot.is_empty())
                .map(move |(index, slot)| (SlotAddress::new(bar, index), *slot))
        })
    }

    /// Puts `ability` in a slot, or empties it of its spell if `ability` is `None`.
    ///
    /// The key is left alone. Changing what a slot casts is not a reason to make the player
    /// bind it again, and the genre's action bars behave the same way.
    ///
    /// Returns whether the slot exists. An unassignable ability — the passive damage-meter
    /// row, the Ultimate — is refused, because a slot holding one could never be cast.
    pub fn assign_spell(&mut self, address: SlotAddress, ability: Option<FireAbility>) -> bool {
        if ability.is_some_and(|ability| !ability.is_assignable()) {
            return false;
        }
        let Some(slot) = self.slot_mut(address) else {
            return false;
        };
        slot.ability = ability;
        true
    }

    /// Empties a slot completely: no spell and no key.
    ///
    /// Returns whether the slot exists. Both halves go, because "clear slot" in the menu is
    /// one action and leaving a stranded binding behind would be a key the player cannot see
    /// and cannot get back except by finding the empty slot it is hiding in.
    pub fn clear_slot(&mut self, address: SlotAddress) -> bool {
        let Some(slot) = self.slot_mut(address) else {
            return false;
        };
        *slot = ActionBarSlot::default();
        true
    }

    /// What stands in the way of giving `address` the key `key`, if anything.
    ///
    /// `bindings` is the game's own key map, so a key the player needs to walk or to close a
    /// menu is refused rather than quietly stolen. A key already held by *this* slot is not
    /// a conflict: rebinding a slot to the key it already has is a no-op, not a clash.
    #[must_use]
    pub fn conflict(
        &self,
        address: SlotAddress,
        key: KeyCode,
        bindings: &Bindings,
    ) -> Option<KeyConflict> {
        if let Some(what) = bindings.reserved(key) {
            return Some(KeyConflict::Reserved(what));
        }
        self.slot_for_key(key)
            .filter(|holder| *holder != address)
            .map(KeyConflict::Slot)
    }

    /// Gives `address` the key `key`, taking it from whichever slot had it.
    ///
    /// Returns whether anything was assigned. Unbinding the previous holder happens in the
    /// same call on purpose: there is no instant at which two slots claim one key, so no
    /// caller can observe — or persist — an ambiguous configuration. A reserved key is
    /// refused outright; the caller is expected to have asked [`ActionBarSet::conflict`]
    /// first and to have told the player why.
    pub fn assign_key(&mut self, address: SlotAddress, key: KeyCode, bindings: &Bindings) -> bool {
        if bindings.reserved(key).is_some() || !address.is_valid() {
            return false;
        }
        if let Some(previous) = self.slot_for_key(key)
            && previous != address
            && let Some(slot) = self.slot_mut(previous)
        {
            slot.key = None;
        }
        let Some(slot) = self.slot_mut(address) else {
            return false;
        };
        slot.key = Some(key);
        true
    }

    /// Takes the key away from a slot, leaving its spell.
    pub fn clear_key(&mut self, address: SlotAddress) -> bool {
        let Some(slot) = self.slot_mut(address) else {
            return false;
        };
        slot.key = None;
        true
    }

    /// Which slot `key` presses, if any.
    #[must_use]
    pub fn slot_for_key(&self, key: KeyCode) -> Option<SlotAddress> {
        self.occupied()
            .find(|(_, slot)| slot.key == Some(key))
            .map(|(address, _)| address)
    }

    /// One slot, mutably, if it exists.
    fn slot_mut(&mut self, address: SlotAddress) -> Option<&mut ActionBarSlot> {
        self.bars
            .get_mut(address.bar)
            .and_then(|bar| bar.slots.get_mut(address.slot))
    }

    /// Reads the bars out of a saved layout.
    ///
    /// A spell or key name this build does not recognise leaves that half of the slot empty:
    /// a file written by a newer build, or edited by hand, must produce a usable bar rather
    /// than a refusal. The names themselves are left in the file untouched by
    /// `engine-settings`, so downgrading and upgrading again does not lose them.
    #[must_use]
    pub fn from_config(config: &HudConfig) -> Self {
        let mut set = Self::default();
        for (index, bar) in config.bars.iter().enumerate().take(ACTION_BARS) {
            for (slot_index, saved) in bar.slots.iter().enumerate().take(ACTION_BAR_SLOTS) {
                let ability = saved
                    .spell
                    .as_deref()
                    .and_then(FireAbility::from_config_name)
                    .filter(|ability| ability.is_assignable());
                let key = saved.key.as_deref().and_then(KeyCode::from_name);
                if saved.spell.is_some() && ability.is_none() {
                    log::warn!(
                        "hud: action bar {} slot {} holds an unknown spell; leaving it empty",
                        index + 1,
                        slot_index + 1
                    );
                }
                if saved.key.is_some() && key.is_none() {
                    log::warn!(
                        "hud: action bar {} slot {} is bound to an unknown key; leaving it \
                         unbound",
                        index + 1,
                        slot_index + 1
                    );
                }
                set.bars[index].slots[slot_index] = ActionBarSlot { ability, key };
            }
        }
        set
    }

    /// Writes the bars into a layout, ready to be saved.
    ///
    /// Only the slots are touched. Where a bar is, which way it runs and whether it is shown
    /// belong to the [`HudLayout`](super::hud_layout::HudLayout), which owns the same
    /// configuration — so this must not overwrite them.
    pub fn write_config(&self, config: &mut HudConfig) {
        for (index, bar) in self.bars.iter().enumerate() {
            let Some(saved) = config.bars.get_mut(index) else {
                continue;
            };
            saved
                .slots
                .resize(ACTION_BAR_SLOTS, HudSlotConfig::default());
            for (slot_index, slot) in bar.slots.iter().enumerate() {
                saved.slots[slot_index] = HudSlotConfig {
                    spell: slot.ability.map(|ability| ability.config_name().to_owned()),
                    key: slot
                        .key
                        .and_then(KeyCode::name)
                        .map(std::borrow::ToOwned::to_owned),
                };
            }
        }
    }
}

/// How big a bar is in logical pixels, before any scale is applied.
///
/// The full ten slots either way, whether or not they hold anything. An empty slot is still a
/// place the player can drop a spell, so a bar that shrank to fit its contents would be a bar
/// that got harder to fill the emptier it was.
#[must_use]
pub fn bar_size(orientation: HudOrientation) -> (f32, f32) {
    let along = ACTION_BAR_SLOTS as f32 * SLOT_SIZE + (ACTION_BAR_SLOTS as f32 - 1.0) * SLOT_GAP;
    match orientation {
        HudOrientation::Horizontal => (along, SLOT_SIZE),
        HudOrientation::Vertical => (SLOT_SIZE, along),
    }
}

/// Where slot `index` sits inside a bar's rectangle.
///
/// Derived from the rectangle rather than from the constants, so the slots follow the bar's
/// scale and the player's interface size without either being passed in again — and so a
/// slot can never be drawn somewhere the bar is not.
#[must_use]
pub fn slot_rect(bar: UiRect, orientation: HudOrientation, index: usize) -> Option<UiRect> {
    if index >= ACTION_BAR_SLOTS {
        return None;
    }
    let count = ACTION_BAR_SLOTS as f32;
    let (along, size) = match orientation {
        HudOrientation::Horizontal => (bar.width, bar.height),
        HudOrientation::Vertical => (bar.height, bar.width),
    };
    // The gap keeps its share of the length at every scale, which is what makes a bar at
    // 150 % look like the same bar rather than like ten squares that drifted apart.
    let gap = along * (SLOT_GAP / (count * SLOT_SIZE + (count - 1.0) * SLOT_GAP));
    let step = (along - gap * (count - 1.0)) / count + gap;
    let offset = index as f32 * step;
    Some(match orientation {
        HudOrientation::Horizontal => UiRect::new(bar.x + offset, bar.y, step - gap, size),
        HudOrientation::Vertical => UiRect::new(bar.x, bar.y + offset, size, step - gap),
    })
}

/// Which slot of a bar contains a point, if any.
#[must_use]
pub fn slot_at(bar: UiRect, orientation: HudOrientation, x: f32, y: f32) -> Option<usize> {
    if !bar.contains(x, y) {
        return None;
    }
    (0..ACTION_BAR_SLOTS)
        .find(|index| slot_rect(bar, orientation, *index).is_some_and(|rect| rect.contains(x, y)))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::ui::hud_layout::{HudElementId, HudLayout};
    use engine_render_data::ui::UiScale;

    const FHD: (u32, u32) = (1920, 1080);

    fn address(bar: usize, slot: usize) -> SlotAddress {
        SlotAddress::new(bar, slot)
    }

    #[test]
    fn there_are_eight_bars_of_ten_slots_and_the_counts_agree_with_gameplay() {
        let set = ActionBarSet::default();
        assert_eq!(set.bars().len(), ACTION_BARS);
        assert_eq!(ACTION_BARS, 8);
        for bar in set.bars() {
            assert_eq!(bar.slots().len(), ACTION_BAR_SLOTS);
        }
        assert_eq!(ACTION_BAR_SLOTS, engine_game::firemage::ABILITY_SLOTS);
    }

    #[test]
    fn a_fresh_install_gets_the_bar_the_game_already_had() {
        let set = ActionBarSet::with_default_bindings();
        for slot in 0..ACTION_BAR_SLOTS {
            let contents = set.slot(address(0, slot)).expect("bar one exists");
            assert_eq!(contents.ability, FireAbility::from_slot(slot));
            assert_eq!(contents.key, Bindings::ability_key(slot));
        }
        // And nothing else is set up, because nothing else was asked for.
        for index in 1..ACTION_BARS {
            assert!(
                set.bar(index).expect("eight bars").is_empty(),
                "bar {index} came pre-filled"
            );
        }
    }

    #[test]
    fn a_spell_can_be_assigned_to_any_slot_on_any_bar_and_cleared_again() {
        let mut set = ActionBarSet::default();
        for bar in 0..ACTION_BARS {
            let at = address(bar, bar % ACTION_BAR_SLOTS);
            assert!(set.assign_spell(at, Some(FireAbility::Meteor)));
            assert_eq!(
                set.slot(at).expect("the slot exists").ability,
                Some(FireAbility::Meteor)
            );
        }
        let at = address(3, 3);
        assert!(set.assign_spell(at, None), "a slot could not be emptied");
        assert_eq!(set.slot(at).expect("the slot exists").ability, None);
    }

    #[test]
    fn changing_a_slots_spell_keeps_its_key() {
        let mut set = ActionBarSet::with_default_bindings();
        let at = address(0, 0);
        assert!(set.assign_spell(at, Some(FireAbility::Pyroblast)));
        let slot = set.slot(at).expect("the slot exists");
        assert_eq!(slot.ability, Some(FireAbility::Pyroblast));
        assert_eq!(slot.key, Some(KeyCode::Digit1), "the binding was lost");
    }

    #[test]
    fn clearing_a_slot_takes_the_key_with_it() {
        let mut set = ActionBarSet::with_default_bindings();
        let at = address(0, 4);
        assert!(set.clear_slot(at));
        assert!(set.slot(at).expect("the slot exists").is_empty());
        // And the key it held is free again.
        assert_eq!(set.slot_for_key(KeyCode::Digit5), None);
    }

    #[test]
    fn a_spell_that_cannot_be_cast_cannot_be_put_in_a_slot() {
        let mut set = ActionBarSet::default();
        for refused in [FireAbility::Ignite, FireAbility::Cataclysm] {
            assert!(
                !set.assign_spell(address(0, 0), Some(refused)),
                "{refused:?} was accepted"
            );
            assert!(set.slot(address(0, 0)).expect("the slot exists").is_empty());
        }
    }

    #[test]
    fn an_address_that_names_nothing_changes_nothing() {
        let mut set = ActionBarSet::default();
        let bindings = Bindings::default();
        for bad in [address(ACTION_BARS, 0), address(0, ACTION_BAR_SLOTS)] {
            assert!(!bad.is_valid());
            assert_eq!(set.slot(bad), None);
            assert!(!set.assign_spell(bad, Some(FireAbility::Blink)));
            assert!(!set.assign_key(bad, KeyCode::F1, &bindings));
            assert!(!set.clear_slot(bad));
        }
        assert_eq!(set, ActionBarSet::default());
    }

    #[test]
    fn a_key_can_be_assigned_to_any_slot() {
        let mut set = ActionBarSet::default();
        let bindings = Bindings::default();
        let at = address(5, 7);
        assert!(set.assign_key(at, KeyCode::F1, &bindings));
        assert_eq!(
            set.slot(at).expect("the slot exists").key,
            Some(KeyCode::F1)
        );
        assert_eq!(set.slot_for_key(KeyCode::F1), Some(at));

        assert!(set.clear_key(at));
        assert_eq!(set.slot_for_key(KeyCode::F1), None);
    }

    #[test]
    fn a_key_another_slot_holds_is_reported_as_a_conflict() {
        let set = ActionBarSet::with_default_bindings();
        let bindings = Bindings::default();
        assert_eq!(
            set.conflict(address(1, 0), KeyCode::Digit1, &bindings),
            Some(KeyConflict::Slot(address(0, 0)))
        );
        // A slot rebinding the key it already has is not a clash with itself.
        assert_eq!(
            set.conflict(address(0, 0), KeyCode::Digit1, &bindings),
            None
        );
        // A free key is free.
        assert_eq!(set.conflict(address(1, 0), KeyCode::F9, &bindings), None);
    }

    #[test]
    fn a_key_the_game_needs_is_refused_rather_than_taken() {
        let mut set = ActionBarSet::default();
        let bindings = Bindings::default();
        for key in [
            KeyCode::W,
            KeyCode::S,
            KeyCode::A,
            KeyCode::D,
            KeyCode::Space,
            KeyCode::Tab,
            KeyCode::Escape,
            KeyCode::R,
        ] {
            let conflict = set.conflict(address(0, 0), key, &bindings);
            assert!(
                matches!(conflict, Some(KeyConflict::Reserved(_))),
                "{key:?} was not reserved: {conflict:?}"
            );
            assert!(
                !set.assign_key(address(0, 0), key, &bindings),
                "{key:?} was assigned anyway"
            );
        }
        assert!(set.bar(0).expect("bar one").is_empty());
    }

    #[test]
    fn replacing_a_binding_leaves_exactly_one_slot_holding_the_key() {
        let mut set = ActionBarSet::with_default_bindings();
        let bindings = Bindings::default();
        let thief = address(2, 4);
        set.assign_spell(thief, Some(FireAbility::Blink));

        assert!(set.assign_key(thief, KeyCode::Digit1, &bindings));
        assert_eq!(set.slot_for_key(KeyCode::Digit1), Some(thief));
        assert_eq!(
            set.slot(address(0, 0)).expect("bar one slot one").key,
            None,
            "the old owner kept the key"
        );
        // The spell it cast is untouched; only the binding moved.
        assert_eq!(
            set.slot(address(0, 0)).expect("bar one slot one").ability,
            Some(FireAbility::Firebolt)
        );

        // No duplicates anywhere, which is the property the whole conflict rule exists for.
        let holders: Vec<_> = set
            .occupied()
            .filter(|(_, slot)| slot.key == Some(KeyCode::Digit1))
            .collect();
        assert_eq!(holders.len(), 1, "{holders:?}");
    }

    #[test]
    fn no_two_slots_ever_share_a_key_however_many_are_assigned() {
        let mut set = ActionBarSet::with_default_bindings();
        let bindings = Bindings::default();
        // Hand the same four keys around the bars over and over.
        let keys = [KeyCode::F1, KeyCode::F2, KeyCode::Digit1, KeyCode::Q];
        for step in 0..64 {
            let at = address(step % ACTION_BARS, (step * 3) % ACTION_BAR_SLOTS);
            set.assign_key(at, keys[step % keys.len()], &bindings);

            let mut bound: Vec<KeyCode> = set.occupied().filter_map(|(_, slot)| slot.key).collect();
            let count = bound.len();
            bound.sort_unstable();
            bound.dedup();
            assert_eq!(
                bound.len(),
                count,
                "two slots share a key after step {step}"
            );
        }
    }

    #[test]
    fn the_bars_survive_a_round_trip_through_the_saved_layout() {
        let mut set = ActionBarSet::with_default_bindings();
        let bindings = Bindings::default();
        set.assign_spell(address(4, 2), Some(FireAbility::Pyroblast));
        set.assign_key(address(4, 2), KeyCode::F4, &bindings);
        set.clear_slot(address(0, 9));

        let mut config = HudConfig::default();
        set.write_config(&mut config);
        let restored = ActionBarSet::from_config(&config.validated());
        assert_eq!(restored, set);
    }

    #[test]
    fn an_unknown_spell_or_key_in_the_file_leaves_that_half_of_the_slot_empty() {
        let mut config = HudConfig::default();
        config.bars[0].slots[0] = HudSlotConfig::new("frostbolt", "DIGIT1");
        config.bars[0].slots[1] = HudSlotConfig::new("meteor", "MOUSE4");
        // A spell that exists but can never be cast is not a slot's business either.
        config.bars[0].slots[2] = HudSlotConfig::new("ignite", "DIGIT3");

        let set = ActionBarSet::from_config(&config);
        assert_eq!(
            set.slot(address(0, 0)).expect("slot one"),
            ActionBarSlot {
                ability: None,
                key: Some(KeyCode::Digit1),
            }
        );
        assert_eq!(
            set.slot(address(0, 1)).expect("slot two"),
            ActionBarSlot {
                ability: Some(FireAbility::Meteor),
                key: None,
            }
        );
        assert_eq!(set.slot(address(0, 2)).expect("slot three").ability, None);
    }

    #[test]
    fn writing_the_bars_does_not_disturb_where_they_are_drawn() {
        let mut config = HudConfig::default();
        config.bars[1].placement.enabled = true;
        config.bars[1].orientation = HudOrientation::Vertical;
        config.frames.party.enabled = false;
        let before = config.clone();

        ActionBarSet::with_default_bindings().write_config(&mut config);
        for (index, bar) in config.bars.iter().enumerate() {
            assert_eq!(bar.placement, before.bars[index].placement, "bar {index}");
            assert_eq!(
                bar.orientation, before.bars[index].orientation,
                "bar {index}"
            );
        }
        assert_eq!(config.frames, before.frames);
    }

    #[test]
    fn a_horizontal_bar_runs_left_to_right_and_a_vertical_one_top_to_bottom() {
        let horizontal = bar_size(HudOrientation::Horizontal);
        let vertical = bar_size(HudOrientation::Vertical);
        assert!(horizontal.0 > horizontal.1);
        assert_eq!((horizontal.1, horizontal.0), vertical, "not a transpose");

        let rect = UiRect::new(100.0, 200.0, horizontal.0, horizontal.1);
        for index in 1..ACTION_BAR_SLOTS {
            let previous = slot_rect(rect, HudOrientation::Horizontal, index - 1).expect("a slot");
            let current = slot_rect(rect, HudOrientation::Horizontal, index).expect("a slot");
            assert!(current.x > previous.x, "slot {index} is not to the right");
            assert_eq!(current.y, previous.y, "slot {index} changed row");
            assert!(current.x >= previous.right(), "slot {index} overlaps");
        }

        let rect = UiRect::new(100.0, 200.0, vertical.0, vertical.1);
        for index in 1..ACTION_BAR_SLOTS {
            let previous = slot_rect(rect, HudOrientation::Vertical, index - 1).expect("a slot");
            let current = slot_rect(rect, HudOrientation::Vertical, index).expect("a slot");
            assert!(current.y > previous.y, "slot {index} is not below");
            assert_eq!(current.x, previous.x, "slot {index} changed column");
            assert!(current.y >= previous.bottom(), "slot {index} overlaps");
        }
    }

    #[test]
    fn a_slot_is_square_at_the_design_size_in_both_directions() {
        for orientation in [HudOrientation::Horizontal, HudOrientation::Vertical] {
            let size = bar_size(orientation);
            let rect = UiRect::new(0.0, 0.0, size.0, size.1);
            for index in 0..ACTION_BAR_SLOTS {
                let slot = slot_rect(rect, orientation, index).expect("a slot");
                assert!(
                    (slot.width - SLOT_SIZE).abs() < 0.01 && (slot.height - SLOT_SIZE).abs() < 0.01,
                    "{orientation:?} slot {index} is {}x{}",
                    slot.width,
                    slot.height
                );
            }
        }
    }

    #[test]
    fn every_slot_stays_inside_its_bar_at_any_scale() {
        for orientation in [HudOrientation::Horizontal, HudOrientation::Vertical] {
            for factor in [0.5, 1.0, 1.7, 2.0] {
                let size = bar_size(orientation);
                let rect = UiRect::new(40.0, 90.0, size.0 * factor, size.1 * factor);
                for index in 0..ACTION_BAR_SLOTS {
                    let slot = slot_rect(rect, orientation, index).expect("a slot");
                    assert!(
                        slot.x >= rect.x - 0.01
                            && slot.y >= rect.y - 0.01
                            && slot.right() <= rect.right() + 0.01
                            && slot.bottom() <= rect.bottom() + 0.01,
                        "{orientation:?} at {factor}: slot {index} {slot:?} escapes {rect:?}"
                    );
                }
            }
        }
        assert_eq!(
            slot_rect(
                UiRect::new(0.0, 0.0, 10.0, 10.0),
                HudOrientation::Horizontal,
                ACTION_BAR_SLOTS
            ),
            None,
            "a slot past the end of the bar has a rectangle"
        );
    }

    #[test]
    fn turning_a_bar_on_its_side_moves_nothing_between_slots() {
        // The property the brief asks for by name: changing orientation must not destroy or
        // reorder an assignment.
        let set = ActionBarSet::with_default_bindings();
        let mut layout = HudLayout::default();
        let before: Vec<_> = set.bar(0).expect("bar one").slots().to_vec();

        layout.flip_orientation(0);
        assert_eq!(layout.orientation(0), HudOrientation::Vertical);
        assert_eq!(set.bar(0).expect("bar one").slots().as_slice(), before);

        // And the geometry follows the flip, so slot three is still the third one along —
        // downwards now instead of rightwards.
        let rect = layout.resolve(
            HudElementId::ActionBar(0),
            bar_size(HudOrientation::Vertical),
            UiScale::default(),
            FHD,
        );
        let third = slot_rect(rect, HudOrientation::Vertical, 2).expect("a slot");
        let first = slot_rect(rect, HudOrientation::Vertical, 0).expect("a slot");
        assert!(third.y > first.y);
        assert_eq!(third.x, first.x);
    }

    #[test]
    fn a_point_finds_the_slot_it_is_in_and_nothing_when_it_is_between_them() {
        let size = bar_size(HudOrientation::Horizontal);
        let rect = UiRect::new(0.0, 0.0, size.0, size.1);
        for index in 0..ACTION_BAR_SLOTS {
            let slot = slot_rect(rect, HudOrientation::Horizontal, index).expect("a slot");
            let centre = (slot.x + slot.width * 0.5, slot.y + slot.height * 0.5);
            assert_eq!(
                slot_at(rect, HudOrientation::Horizontal, centre.0, centre.1),
                Some(index)
            );
        }
        // In the gap between two slots: inside the bar, inside no slot.
        let first = slot_rect(rect, HudOrientation::Horizontal, 0).expect("a slot");
        let gap = first.right() + SLOT_GAP * 0.5;
        assert_eq!(slot_at(rect, HudOrientation::Horizontal, gap, 10.0), None);
        // Outside the bar entirely.
        assert_eq!(
            slot_at(rect, HudOrientation::Horizontal, -50.0, -50.0),
            None
        );
    }

    #[test]
    fn a_disabled_bar_offers_nothing_to_click() {
        // "Disabled bars must not consume interaction space" is enforced by there being no
        // rectangle: the caller skips a bar the layout says is off, and this is the assertion
        // that the layout does say so.
        let mut layout = HudLayout::default();
        assert!(layout.is_enabled(HudElementId::ActionBar(0)));
        for index in 1..ACTION_BARS {
            assert!(!layout.is_enabled(HudElementId::ActionBar(index)));
        }
        layout.set_enabled(HudElementId::ActionBar(0), false);
        assert!(!layout.is_enabled(HudElementId::ActionBar(0)));
    }

    #[test]
    fn an_address_reads_as_a_player_would_say_it() {
        let mut label = String::new();
        address(0, 0).write_label(&mut label);
        assert_eq!(label, "BAR 1 SLOT 1");
        address(7, 9).write_label(&mut label);
        assert_eq!(label, "BAR 8 SLOT 10");
    }
}
