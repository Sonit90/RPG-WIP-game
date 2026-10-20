//! Equipment sets: pieces that are worth more together.
//!
//! ```text
//! sets.json ──► SetDef (pieces, tiered bonuses) ──► register_sets
//!                                                      │
//!   Equipment ──active_sets──► ActiveSets ─┬─ stats()   ──► CharacterSheet::gear_stats
//!                                          └─ effects() ──► SetEffects ──► Simulation
//! ```
//!
//! # Where a set bonus goes
//!
//! A bonus is **derived from what is worn**, never stored: nothing in a save or a database
//! says "this character has the 4-piece". [`active_sets`] counts the *distinct* pieces of each
//! set on a body (two copies of the same ring are one piece) and a bonus tier is active when
//! the count reaches its threshold.
//!
//! * **Stats** are folded into [`super::CharacterSheet::gear_stats`] additively, so combat, the
//!   character panel and the realm all read them from the one aggregation. They are *not*
//!   scaled by an item's quality or sharpening: a set bonus is a property of the set.
//! * **Effects** change how an ability behaves and ride the machinery that already exists for
//!   it: damage in the one place damage is applied, cooldowns in the
//!   [`crate::ability::AbilityBook`], resource generation where the class gains its resource.
//!   [`SetEffects`] is the resolved, per-ability form of every active effect.
//!
//! The server computes both; the client calls the same pure functions only to *display* them.
//!
//! # Why the registry is installed
//!
//! This crate keeps no serde and cannot read the data catalog. `engine-realm` parses
//! `assets/catalog/sets.json`, checks it against the item catalog (every piece exists and names
//! its set, and the reverse) and [`register_sets`]s it once per process. Until then there are
//! no sets, which is every unit test in this crate that does not install one.

use std::sync::OnceLock;

use super::equipment::Equipment;
use super::item::{EquipSlot, ItemId, ItemStack};
use super::stat::{Stat, StatBlock};
use crate::ability::{ABILITY_COUNT, FireAbility};

/// The most pieces a set may have: one per slot that could carry one, with rings and trinkets
/// counted twice.
pub const MAX_SET_PIECES: usize = EquipSlot::COUNT;

/// The most a single effect may be worth, as a fraction. Sixty per cent more damage is already
/// not a set bonus.
pub const MAX_EFFECT_FRACTION: f32 = 0.6;

/// The most any ability's cooldown can be cut by set effects together, as a fraction.
pub const MAX_COOLDOWN_CUT: f32 = 0.5;

/// What a bonus does to an ability, beyond adding stats.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SetEffect {
    /// `value` more damage (a fraction: `0.1` is +10 %) from `ability`, direct and periodic.
    DamagePct {
        /// Which ability.
        ability: FireAbility,
        /// The extra damage as a fraction.
        value: f32,
    },
    /// `value` shorter cooldown (a fraction: `0.2` is 20 % shorter) for `ability`.
    CooldownPct {
        /// Which ability.
        ability: FireAbility,
        /// The cooldown cut as a fraction.
        value: f32,
    },
    /// `value` more of the class's resource from casting: the FireMage's Heat and the Druid's
    /// Astral Power. The Warrior's Valor is a count of hits, not an amount, and is unchanged.
    ResourceGenPct {
        /// The extra generation as a fraction.
        value: f32,
    },
}

impl SetEffect {
    /// The written names of every kind, as `sets.json` spells them.
    pub const KINDS: [&'static str; 3] = ["damage_pct", "cooldown_pct", "resource_gen_pct"];

    /// An effect from its written parts.
    ///
    /// # Errors
    ///
    /// A sentence naming what is wrong: an unknown kind, an ability the kind needs and lacks (or
    /// one it does not take), an unknown ability, a value that is not a fraction in
    /// `(0, MAX_EFFECT_FRACTION]` (`MAX_COOLDOWN_CUT` for a cooldown).
    pub fn parse(kind: &str, ability: Option<&str>, value: f32) -> Result<Self, String> {
        let ability = match ability {
            Some(name) => Some(
                FireAbility::from_config_name(name)
                    .ok_or_else(|| format!("unknown ability {name:?}"))?,
            ),
            None => None,
        };
        let limit = if kind == "cooldown_pct" {
            MAX_COOLDOWN_CUT
        } else {
            MAX_EFFECT_FRACTION
        };
        if !(value.is_finite() && value > 0.0 && value <= limit) {
            return Err(format!(
                "{kind} value {value} is not a fraction above 0 and at most {limit}"
            ));
        }
        match (kind, ability) {
            ("damage_pct", Some(ability)) => Ok(Self::DamagePct { ability, value }),
            ("cooldown_pct", Some(ability)) => Ok(Self::CooldownPct { ability, value }),
            ("resource_gen_pct", None) => Ok(Self::ResourceGenPct { value }),
            ("damage_pct" | "cooldown_pct", None) => Err(format!("{kind} needs an ability")),
            ("resource_gen_pct", Some(_)) => Err("resource_gen_pct takes no ability".to_owned()),
            (other, _) => Err(format!("unknown effect kind {other:?}")),
        }
    }

    /// The written kind: what [`Self::parse`] reads back.
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Self::DamagePct { .. } => "damage_pct",
            Self::CooldownPct { .. } => "cooldown_pct",
            Self::ResourceGenPct { .. } => "resource_gen_pct",
        }
    }

    /// The ability it names, for the kinds that name one.
    #[must_use]
    pub fn ability(self) -> Option<FireAbility> {
        match self {
            Self::DamagePct { ability, .. } | Self::CooldownPct { ability, .. } => Some(ability),
            Self::ResourceGenPct { .. } => None,
        }
    }

    /// The fraction it is worth.
    #[must_use]
    pub fn value(self) -> f32 {
        match self {
            Self::DamagePct { value, .. }
            | Self::CooldownPct { value, .. }
            | Self::ResourceGenPct { value } => value,
        }
    }
}

/// One tier of a set: what wearing `count` pieces grants.
#[derive(Debug, Clone, PartialEq)]
pub struct SetBonus {
    /// How many distinct pieces switch it on.
    pub count: u8,
    /// Stats it adds, by stat. `Stat::MainStat` becomes the wearer's own main stat.
    pub stats: Vec<(Stat, f32)>,
    /// Changes to abilities it makes.
    pub effects: Vec<SetEffect>,
}

/// One equipment set: its pieces and the bonuses wearing them unlocks.
#[derive(Debug, Clone, PartialEq)]
pub struct SetDef {
    /// Stable identity. What an item's `set` names.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The item keys that belong to it.
    pub pieces: Vec<String>,
    /// Its tiers, by ascending `count`.
    pub bonuses: Vec<SetBonus>,
}

impl SetDef {
    /// Whether the set is internally consistent: a name, two or more distinct pieces, at least
    /// one tier, tiers strictly ascending and none above the piece count, every tier granting
    /// something, every stat value positive and finite.
    ///
    /// Whether the *pieces exist* is the catalog's business: this crate does not see the file.
    ///
    /// # Errors
    ///
    /// A sentence naming the first thing wrong.
    pub fn validate(&self) -> Result<(), String> {
        let id = &self.id;
        if id.trim().is_empty() || self.name.trim().is_empty() {
            return Err("a set has no id or no name".to_owned());
        }
        if self.pieces.len() < 2 || self.pieces.len() > MAX_SET_PIECES {
            return Err(format!(
                "set {id:?} has {} pieces; a set has 2 to {MAX_SET_PIECES}",
                self.pieces.len()
            ));
        }
        for (index, piece) in self.pieces.iter().enumerate() {
            if self.pieces[..index].contains(piece) {
                return Err(format!("set {id:?} lists piece {piece:?} twice"));
            }
        }
        if self.bonuses.is_empty() {
            return Err(format!("set {id:?} has no bonus"));
        }
        let mut previous = 0;
        for bonus in &self.bonuses {
            if bonus.count <= previous {
                return Err(format!(
                    "set {id:?} bonus counts must be ascending and above zero ({} after {previous})",
                    bonus.count
                ));
            }
            if usize::from(bonus.count) > self.pieces.len() {
                return Err(format!(
                    "set {id:?} has a {}-piece bonus but only {} pieces",
                    bonus.count,
                    self.pieces.len()
                ));
            }
            if bonus.stats.is_empty() && bonus.effects.is_empty() {
                return Err(format!(
                    "set {id:?} {}-piece bonus grants nothing",
                    bonus.count
                ));
            }
            for (stat, value) in &bonus.stats {
                if !(value.is_finite() && *value > 0.0) {
                    return Err(format!(
                        "set {id:?} {}-piece bonus has a bad {} value",
                        bonus.count,
                        stat.key()
                    ));
                }
            }
            previous = bonus.count;
        }
        Ok(())
    }

    /// Whether the item with this key is one of the pieces.
    #[must_use]
    pub fn has_piece(&self, key: &str) -> bool {
        self.pieces.iter().any(|piece| piece == key)
    }
}

static SETS: OnceLock<Vec<SetDef>> = OnceLock::new();

/// Registers the data catalog's sets so [`active_sets`] can find them.
///
/// Idempotent: the first call wins and returns `true`; a later one changes nothing and returns
/// `false` (logging a warning if it was given different sets, which means two catalogs disagree
/// inside one process). The caller is expected to have validated the sets against the item
/// catalog; a set that fails [`SetDef::validate`] is dropped here rather than half-trusted.
pub fn register_sets(sets: Vec<SetDef>) -> bool {
    let mut installed = false;
    let rows = SETS.get_or_init(|| {
        installed = true;
        sets.iter()
            .filter(|set| match set.validate() {
                Ok(()) => true,
                Err(error) => {
                    log::error!("dropping set: {error}");
                    false
                }
            })
            .cloned()
            .collect()
    });
    if !installed && rows != &sets {
        log::warn!("sets registered twice with different contents; keeping the first");
    }
    installed
}

/// Every registered set, in catalog order. Empty until [`register_sets`] runs.
#[must_use]
pub fn registered_sets() -> &'static [SetDef] {
    SETS.get().map_or(&[], Vec::as_slice)
}

/// The registered set with this id.
#[must_use]
pub fn set_by_id(id: &str) -> Option<&'static SetDef> {
    registered_sets().iter().find(|set| set.id == id)
}

/// One set as worn: how much of it, and so which tiers are on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveSet {
    /// The set.
    pub set: &'static SetDef,
    /// How many distinct pieces are worn.
    pub equipped: u8,
}

impl ActiveSet {
    /// The tiers the worn pieces switch on, in order.
    pub fn active_bonuses(&self) -> impl Iterator<Item = &'static SetBonus> + use<> {
        let equipped = self.equipped;
        self.set
            .bonuses
            .iter()
            .filter(move |bonus| bonus.count <= equipped)
    }

    /// How many tiers are on.
    #[must_use]
    pub fn tiers_active(&self) -> usize {
        self.active_bonuses().count()
    }

    /// Whether a given tier is on.
    #[must_use]
    pub fn is_active(&self, bonus: &SetBonus) -> bool {
        bonus.count <= self.equipped
    }
}

/// Every set the wearer has at least one piece of.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActiveSets {
    sets: Vec<ActiveSet>,
}

impl ActiveSets {
    /// The sets with a piece worn, in the order the first piece appears in slot order.
    pub fn iter(&self) -> impl Iterator<Item = &ActiveSet> {
        self.sets.iter()
    }

    /// Whether no set has a single piece worn.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// The part of `set` worn, if any of it is.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ActiveSet> {
        self.sets.iter().find(|active| active.set.id == id)
    }

    /// Every active tier of every set, with the set it belongs to.
    pub fn active_bonuses(&self) -> impl Iterator<Item = (&'static SetDef, &'static SetBonus)> {
        self.sets
            .iter()
            .flat_map(|active| active.active_bonuses().map(|bonus| (active.set, bonus)))
    }

    /// The stats every active tier adds together. `Stat::MainStat` is left as the placeholder:
    /// [`super::CharacterSheet::gear_stats`] resolves it with the rest of the gear's.
    #[must_use]
    pub fn stats(&self) -> StatBlock {
        let mut total = StatBlock::zero();
        for (_, bonus) in self.active_bonuses() {
            for (stat, value) in &bonus.stats {
                total.add(*stat, *value);
            }
        }
        total
    }

    /// Every active tier's effects, resolved per ability.
    #[must_use]
    pub fn effects(&self) -> SetEffects {
        let mut effects = SetEffects::NONE;
        for (_, bonus) in self.active_bonuses() {
            for effect in &bonus.effects {
                effects.add(*effect);
            }
        }
        effects
    }
}

/// The distinct set pieces among `slots`, counted per set.
///
/// The one counting rule: a piece is an *item*, so two stacks of the same item (two copies of a
/// ring in the two ring slots) are one piece. An item that names a set which is not registered,
/// or which the set does not list, counts for nothing.
#[must_use]
pub fn active_sets_of(slots: &[Option<ItemStack>]) -> ActiveSets {
    let mut out = ActiveSets::default();
    if registered_sets().is_empty() {
        return out;
    }
    let mut seen: Vec<ItemId> = Vec::new();
    for stack in slots.iter().flatten() {
        let Some(def) = stack.def() else { continue };
        let Some(set_id) = def.set else { continue };
        if seen.contains(&def.id) {
            continue;
        }
        let Some(set) = set_by_id(set_id).filter(|set| set.has_piece(def.key)) else {
            continue;
        };
        seen.push(def.id);
        match out.sets.iter_mut().find(|active| active.set.id == set.id) {
            Some(active) => active.equipped = active.equipped.saturating_add(1),
            None => out.sets.push(ActiveSet { set, equipped: 1 }),
        }
    }
    out
}

/// **Which sets a body is wearing, and how much of each.** Pure: the same equipment always
/// gives the same answer, and the server and the client both ask it.
#[must_use]
pub fn active_sets(equipment: &Equipment) -> ActiveSets {
    let mut slots = [None; EquipSlot::COUNT];
    for (slot, stack) in equipment.worn() {
        slots[slot.index()] = Some(stack);
    }
    active_sets_of(&slots)
}

/// What every active set effect adds up to, per ability. Small, `Copy`, and cheap to ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SetEffects {
    damage: [f32; ABILITY_COUNT],
    cooldown: [f32; ABILITY_COUNT],
    resource_gen: f32,
}

impl SetEffects {
    /// No effects: every factor is one.
    pub const NONE: Self = Self {
        damage: [0.0; ABILITY_COUNT],
        cooldown: [0.0; ABILITY_COUNT],
        resource_gen: 0.0,
    };

    /// The effects of what `equipment` wears.
    #[must_use]
    pub fn of(equipment: &Equipment) -> Self {
        if registered_sets().is_empty() {
            return Self::NONE;
        }
        active_sets(equipment).effects()
    }

    /// Adds one effect to the totals.
    pub fn add(&mut self, effect: SetEffect) {
        match effect {
            SetEffect::DamagePct { ability, value } => self.damage[ability.index()] += value,
            SetEffect::CooldownPct { ability, value } => self.cooldown[ability.index()] += value,
            SetEffect::ResourceGenPct { value } => self.resource_gen += value,
        }
    }

    /// Whether nothing is in force.
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// The extra damage `ability` deals, as a fraction.
    #[must_use]
    pub fn damage_bonus(&self, ability: FireAbility) -> f32 {
        self.damage[ability.index()]
    }

    /// What `ability`'s cooldown is multiplied by: one, less its cooldown cuts, never below
    /// [`MAX_COOLDOWN_CUT`] taken off.
    #[must_use]
    pub fn cooldown_scale(&self, ability: FireAbility) -> f32 {
        1.0 - self.cooldown[ability.index()].min(MAX_COOLDOWN_CUT)
    }

    /// What the class's generated resource is multiplied by.
    #[must_use]
    pub fn resource_gen_scale(&self) -> f32 {
        1.0 + self.resource_gen
    }
}

impl Default for SetEffects {
    fn default() -> Self {
        Self::NONE
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn bonus(count: u8) -> SetBonus {
        SetBonus {
            count,
            stats: vec![(Stat::Haste, 5.0)],
            effects: Vec::new(),
        }
    }

    fn set(pieces: &[&str], bonuses: Vec<SetBonus>) -> SetDef {
        SetDef {
            id: "s".to_owned(),
            name: "S".to_owned(),
            pieces: pieces.iter().map(|piece| (*piece).to_owned()).collect(),
            bonuses,
        }
    }

    #[test]
    fn effects_are_parsed_and_refused_by_the_rules_of_their_kind() {
        let ok = SetEffect::parse("damage_pct", Some("pyroblast"), 0.1).expect("valid");
        assert_eq!(ok.kind(), "damage_pct");
        assert_eq!(ok.ability(), Some(FireAbility::Pyroblast));
        assert_eq!(ok.value(), 0.1);
        assert!(SetEffect::parse("cooldown_pct", Some("meteor"), 0.3).is_ok());
        assert!(SetEffect::parse("resource_gen_pct", None, 0.1).is_ok());
        for (kind, ability, value) in [
            ("damage_pct", Some("not_an_ability"), 0.1),
            ("damage_pct", None, 0.1),
            ("cooldown_pct", None, 0.1),
            ("resource_gen_pct", Some("meteor"), 0.1),
            ("extra_charge", Some("meteor"), 1.0),
            ("damage_pct", Some("meteor"), 0.0),
            ("damage_pct", Some("meteor"), -0.1),
            ("damage_pct", Some("meteor"), 0.7),
            ("damage_pct", Some("meteor"), f32::NAN),
            ("cooldown_pct", Some("meteor"), 0.6),
        ] {
            assert!(
                SetEffect::parse(kind, ability, value).is_err(),
                "{kind} {ability:?} {value}"
            );
        }
        for kind in SetEffect::KINDS {
            let ability = (kind != "resource_gen_pct").then_some("meteor");
            let parsed = SetEffect::parse(kind, ability, 0.1).expect("valid");
            assert_eq!(parsed.kind(), kind);
        }
    }

    #[test]
    fn a_set_must_be_consistent_with_itself() {
        assert!(set(&["a", "b"], vec![bonus(2)]).validate().is_ok());
        assert!(
            set(&["a", "b", "c", "d", "e"], vec![bonus(2), bonus(4), bonus(5)])
                .validate()
                .is_ok()
        );
        for (name, bad) in [
            ("one piece", set(&["a"], vec![bonus(1)])),
            ("duplicate piece", set(&["a", "a"], vec![bonus(2)])),
            ("no bonus", set(&["a", "b"], Vec::new())),
            ("count zero", set(&["a", "b"], vec![bonus(0)])),
            (
                "count above pieces",
                set(&["a", "b"], vec![bonus(2), bonus(3)]),
            ),
            (
                "descending counts",
                set(&["a", "b", "c"], vec![bonus(3), bonus(2)]),
            ),
            (
                "repeated count",
                set(&["a", "b", "c"], vec![bonus(2), bonus(2)]),
            ),
            (
                "empty bonus",
                set(
                    &["a", "b"],
                    vec![SetBonus {
                        count: 2,
                        stats: Vec::new(),
                        effects: Vec::new(),
                    }],
                ),
            ),
            (
                "negative stat",
                set(
                    &["a", "b"],
                    vec![SetBonus {
                        count: 2,
                        stats: vec![(Stat::Haste, -1.0)],
                        effects: Vec::new(),
                    }],
                ),
            ),
        ] {
            assert!(bad.validate().is_err(), "{name} was accepted");
        }
        let mut nameless = set(&["a", "b"], vec![bonus(2)]);
        nameless.name.clear();
        assert!(nameless.validate().is_err());
    }

    #[test]
    fn effects_are_resolved_per_ability_and_cooldown_cuts_are_capped() {
        let mut effects = SetEffects::NONE;
        assert!(effects.is_none());
        effects.add(SetEffect::DamagePct {
            ability: FireAbility::Firebolt,
            value: 0.1,
        });
        effects.add(SetEffect::ResourceGenPct { value: 0.15 });
        assert_eq!(effects.damage_bonus(FireAbility::Firebolt), 0.1);
        assert_eq!(effects.damage_bonus(FireAbility::Pyroblast), 0.0);
        assert_eq!(effects.cooldown_scale(FireAbility::Meteor), 1.0);
        assert!((effects.resource_gen_scale() - 1.15).abs() < 1e-6);
        for _ in 0..4 {
            effects.add(SetEffect::CooldownPct {
                ability: FireAbility::Meteor,
                value: 0.4,
            });
        }
        assert_eq!(effects.cooldown_scale(FireAbility::Meteor), 0.5);
        assert!(!effects.is_none());
    }

    #[test]
    fn without_registered_sets_nothing_is_active() {
        assert!(registered_sets().is_empty());
        assert!(set_by_id("anything").is_none());
        assert!(active_sets(&Equipment::empty()).is_empty());
        assert!(SetEffects::of(&Equipment::empty()).is_none());
    }
}
