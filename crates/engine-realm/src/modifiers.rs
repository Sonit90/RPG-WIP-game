//! Dungeon key modifiers: what exists, how a key rolls them, how they scale and how a reroll
//! changes them.
//!
//! ```text
//! assets/catalog/modifiers.json ─► ModifierCatalog ─┬─ roll_key_modifiers(level, seed) ─► Vec<KeyModifier>
//!                                                   ├─ resolve(level, &[KeyModifier]) ──► Vec<ActiveModifier> ─► Simulation::set_modifiers
//!                                                   ├─ reroll_outcome / apply_reroll    ─► new magnitude_pct
//!                                                   ├─ downgrade / cleanse              ─► a weaker key
//!                                                   └─ row_for / key_scaling(level)     ─► slots, (health, damage) multipliers
//! ```
//!
//! # Pure functions
//!
//! Nothing here reads a clock or a random source. Randomness arrives as a seed or as numbers
//! in `[0, 1)` that the caller rolled (the realm module's `ctx.random`), the same convention
//! as the rest of the crate, so every rule is deterministic and testable natively. Storing a
//! key's modifiers and reducers that call these are somebody else's job; this is what they
//! call.
//!
//! # Two kinds of strength
//!
//! A modifier's strength is the product of two independent things:
//!
//! * the **key level**: `1 + scaling_per_level × (level − introduced_at_level)`, so the same
//!   modifier hits harder on a higher key without anyone touching it;
//! * the **reroll magnitude** stored on the key, [`KeyModifier::magnitude_pct`]: `0` is the
//!   modifier as designed, `+17` is seventeen per cent stronger, `−12` twelve per cent weaker.
//!
//! The progression table (key levels 1-12) and the `eternal` block that continues it are the
//! single source of a key level's modifier slots and monster health and damage multipliers
//! ([`ModifierCatalog::row_for`], [`ModifierCatalog::key_scaling`]); the dungeon's own
//! `keys.health_per_level` stays only as the fallback for a catalog with no table.
//!
//! # Eternal keys
//!
//! Past the table, [`ModifierCatalog::row_for`] synthesises rows from the `eternal` rules: an
//! odd level adds modifier slots (up to a cap) and carries the multipliers, an even level raises
//! the multipliers by a fixed step and carries the slots. Pure arithmetic on the level, so any
//! key level has a row and none is stored.

use engine_game::modifier::{
    ActiveModifier, BeamParams, BolsteringParams, ModifierKind, ModifierParams, NecroticParams,
    PlagueParams, RagingParams, SpitefulParams,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::catalog::{CatalogError, KeyRules};
use crate::error::RealmError;

/// How a reroll changes a modifier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerollRules {
    /// Chance a reroll strengthens rather than weakens, `0..=1`.
    pub strengthen_chance: f64,
    /// The smallest change, in per cent.
    pub min_pct: u32,
    /// The largest change, in per cent.
    pub max_pct: u32,
}

/// One modifier a key can carry.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifierDefinition {
    /// Stable identifier. What a key stores.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One or two sentences for the key panel.
    pub description: String,
    /// The icon's key, for the interface.
    pub icon: String,
    /// What it does when it runs.
    pub kind: ModifierKind,
    /// The lowest key level it can appear on, and the level at which it is at its base.
    pub introduced_at_level: u32,
    /// Its base numbers.
    pub params: ModifierParams,
    /// Fractional increase in magnitude for every key level above `introduced_at_level`.
    pub scaling_per_level: f32,
    /// Whether a reroll may make it stronger.
    pub amplifiable: bool,
    /// Whether a reroll may make it weaker.
    pub reducible: bool,
    /// How a reroll changes it.
    pub reroll: RerollRules,
}

/// What one key level asks of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressionRow {
    /// The key level this row is for.
    pub level: u32,
    /// How many modifiers a key of this level carries.
    pub modifier_slots: u32,
    /// What monster health is multiplied by.
    pub health_mult: f32,
    /// What monster damage is multiplied by.
    pub damage_mult: f32,
    /// A designer's note on the row.
    pub notes: String,
}

/// How key levels past the progression table continue it (`eternal` in `modifiers.json`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EternalRules {
    /// The first key level the rules apply to: the level after the table's last row.
    pub from_level: u32,
    /// Modifier slots an odd level adds.
    pub odd_add_slots: u32,
    /// Added to the health multiplier by every even level.
    pub even_health_step: f32,
    /// Added to the damage multiplier by every even level.
    pub even_damage_step: f32,
    /// The most modifier slots a key can carry.
    pub max_modifier_slots: u32,
}

/// The bounds a modifier's total magnitude is held to, in per cent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MagnitudeLimits {
    /// The weakest it can be made.
    pub min_pct: i16,
    /// The strongest it can be made.
    pub max_pct: i16,
}

impl Default for MagnitudeLimits {
    fn default() -> Self {
        Self {
            min_pct: -50,
            max_pct: 100,
        }
    }
}

/// The reroll and cleanse tunables (`config` in `modifiers.json`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModifierConfig {
    /// The fraction of the dungeon's placed monsters that must be dead before a reroll is
    /// allowed, `0..=1`.
    pub reroll_unlock_fraction: f32,
    /// The resource a reroll costs.
    pub reroll_resource: String,
    /// How much of it one reroll costs.
    pub reroll_cost: u32,
    /// How many rerolls a run allows, for the whole party together.
    pub max_rerolls_per_run: u32,
    /// The reward bonus, per cent, a run that was rerolled pays on success.
    pub reroll_reward_bonus_pct: u32,
    /// What a cleanse costs, in gold.
    pub cleanse_gold_cost: u32,
}

impl Default for ModifierConfig {
    fn default() -> Self {
        Self {
            reroll_unlock_fraction: 0.5,
            reroll_resource: "aether_shard".to_owned(),
            reroll_cost: 1,
            max_rerolls_per_run: 1,
            reroll_reward_bonus_pct: 25,
            cleanse_gold_cost: 100,
        }
    }
}

/// How much of the dungeon is cleared: `killed / total`, zero for a dungeon with nothing placed.
#[must_use]
pub fn kill_fraction(progress: (u32, u32)) -> f32 {
    let (killed, total) = progress;
    if total == 0 {
        0.0
    } else {
        (killed as f32 / total as f32).clamp(0.0, 1.0)
    }
}

/// Everything a reroll's checks look at, gathered by the caller from its own state.
#[derive(Debug, Clone, Copy)]
pub struct RerollRequest<'a> {
    /// The key's modifiers as they stand.
    pub mods: &'a [KeyModifier],
    /// Which of them the caller asked to reroll.
    pub index: usize,
    /// The fight's `(killed, total)` placed monsters.
    pub progress: (u32, u32),
    /// Rerolls the run has used.
    pub rerolls_done: u32,
    /// The caller's balance of the reroll resource when the run began.
    pub balance: u64,
    /// What the caller has already spent of it this run.
    pub spent: u64,
}

/// A reroll that passed every check, with what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reroll {
    /// Which modifier.
    pub index: usize,
    /// Its catalog id.
    pub id: String,
    /// The change the dice made, per cent (positive: stronger).
    pub delta_pct: i16,
    /// The modifier's magnitude after, per cent, held to the catalog's limits.
    pub magnitude_pct: i16,
    /// The cost, in the reroll resource.
    pub cost: u32,
}

impl Reroll {
    /// Whether the dice strengthened the modifier.
    #[must_use]
    pub fn strengthened(&self) -> bool {
        self.delta_pct > 0
    }

    /// `mods` with this reroll applied.
    #[must_use]
    pub fn apply(&self, mods: &[KeyModifier]) -> Vec<KeyModifier> {
        let mut out = mods.to_vec();
        if let Some(target) = out.get_mut(self.index) {
            target.magnitude_pct = self.magnitude_pct;
        }
        out
    }
}

/// The last reroll of a run, as the instance row carries it for the clients' reveal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerollEvent {
    /// Counts the run's rerolls from 1; a client plays the reveal when it grows.
    pub seq: u32,
    /// The character that paid for it.
    pub by: String,
    /// The modifier's catalog id.
    pub modifier: String,
    /// The change, per cent (positive: stronger).
    pub delta_pct: i16,
    /// The magnitude after, per cent.
    pub magnitude_pct: i16,
}

impl RerollEvent {
    /// Whether the reroll strengthened the modifier.
    #[must_use]
    pub fn strengthened(&self) -> bool {
        self.delta_pct > 0
    }

    /// The JSON the instance row stores.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The record in `text`; `None` for empty or unreadable text.
    #[must_use]
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
}

/// A key's modifiers as the JSON the tables and the API store.
#[must_use]
pub fn encode_modifiers(mods: &[KeyModifier]) -> String {
    serde_json::to_string(mods).unwrap_or_else(|_| "[]".to_owned())
}

/// The modifiers in stored JSON. Empty or unreadable text is a key with none: a key written by
/// a build that did not know modifiers still opens its dungeon.
#[must_use]
pub fn decode_modifiers(text: &str) -> Vec<KeyModifier> {
    serde_json::from_str(text).unwrap_or_default()
}

/// The signed per-cent text of a magnitude: `+17%`, `-12%`, `+0%`.
#[must_use]
pub fn magnitude_text(pct: i16) -> String {
    format!("{pct:+}%")
}

/// A modifier as a key stores it. Serialises into the `dungeon_keys.modifiers` JSON column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyModifier {
    /// The modifier's id in the catalog.
    pub id: String,
    /// Reroll magnitude in per cent: `0` is base, `+17` seventeen per cent stronger.
    #[serde(default)]
    pub magnitude_pct: i16,
}

impl KeyModifier {
    /// A modifier at its base.
    #[must_use]
    pub fn base(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            magnitude_pct: 0,
        }
    }
}

/// Every modifier and the progression table.
#[derive(Debug, Clone, PartialEq)]
pub struct ModifierCatalog {
    modifiers: Vec<ModifierDefinition>,
    progression: Vec<ProgressionRow>,
    eternal: Option<EternalRules>,
    limits: MagnitudeLimits,
    config: ModifierConfig,
}

impl ModifierCatalog {
    /// The reroll and cleanse tunables.
    #[must_use]
    pub fn config(&self) -> &ModifierConfig {
        &self.config
    }

    /// Every modifier, in catalog order.
    #[must_use]
    pub fn modifiers(&self) -> &[ModifierDefinition] {
        &self.modifiers
    }

    /// The modifier with this id.
    #[must_use]
    pub fn modifier(&self, id: &str) -> Option<&ModifierDefinition> {
        self.modifiers.iter().find(|definition| definition.id == id)
    }

    /// The progression table, by level.
    #[must_use]
    pub fn progression(&self) -> &[ProgressionRow] {
        &self.progression
    }

    /// The bounds a reroll holds a magnitude to.
    #[must_use]
    pub fn limits(&self) -> MagnitudeLimits {
        self.limits
    }

    /// The eternal rules that continue the table, if the catalog has them.
    #[must_use]
    pub fn eternal(&self) -> Option<&EternalRules> {
        self.eternal.as_ref()
    }

    /// The row for key `level`. `None` only for a catalog with no table at all.
    ///
    /// The table's own row for a level it covers, the first row for a level before it. Past the
    /// table, with `eternal` rules, a row synthesised from the last table row: every odd level
    /// from `from_level` adds the rules' slots (capped at `max_modifier_slots`), every even level
    /// adds the health and damage steps. Without eternal rules, the last row holds.
    #[must_use]
    pub fn row_for(&self, level: u32) -> Option<ProgressionRow> {
        let last = self.progression.last()?;
        if level < 1 {
            return self.progression.first().cloned();
        }
        if let Some(row) = self.progression.iter().find(|row| row.level == level) {
            return Some(row.clone());
        }
        let Some(rules) = self.eternal.filter(|rules| level >= rules.from_level) else {
            return Some(last.clone());
        };
        // Odd and even levels in from_level..=level, in closed form so a key level of any size
        // costs the same. u64 so `level + 1` cannot overflow.
        let (from, to) = (u64::from(rules.from_level), u64::from(level));
        let odd = (to + 1) / 2 - from / 2;
        let even = to / 2 - (from - 1) / 2;
        let slots = (u64::from(last.modifier_slots) + odd * u64::from(rules.odd_add_slots))
            .min(u64::from(rules.max_modifier_slots));
        Some(ProgressionRow {
            level,
            modifier_slots: u32::try_from(slots).unwrap_or(rules.max_modifier_slots),
            health_mult: last.health_mult + even as f32 * rules.even_health_step,
            damage_mult: last.damage_mult + even as f32 * rules.even_damage_step,
            notes: if level % 2 == 1 {
                "Eternal: another modifier.".to_owned()
            } else {
                "Eternal: stronger monsters.".to_owned()
            },
        })
    }

    /// How many modifiers a key of `level` carries.
    #[must_use]
    pub fn slots(&self, level: u32) -> usize {
        self.row_for(level)
            .map_or(0, |row| row.modifier_slots as usize)
    }

    /// What a key of `level` multiplies monster `(health, damage)` by.
    ///
    /// [`ModifierCatalog::row_for`] when the catalog has a table, otherwise the dungeon's own
    /// linear `keys` rule (see [`crate::key::scaling`]). The realm module calls this in place of
    /// `key::scaling` when it builds the fight.
    #[must_use]
    pub fn key_scaling(&self, rules: &KeyRules, level: u32) -> (f32, f32) {
        match self.row_for(level) {
            Some(row) => (row.health_mult, row.damage_mult),
            None => crate::key::scaling(rules, level),
        }
    }

    /// Parses the catalog's `modifiers.json`.
    ///
    /// # Errors
    ///
    /// Malformed JSON, an unknown version or kind or parameter name, a duplicate id, a
    /// reroll that cannot work, or a progression table that is not levels 1, 2, 3 … in order.
    pub fn parse(json: &str) -> Result<Self, CatalogError> {
        let file: File = serde_json::from_str(json)
            .map_err(|error| CatalogError(format!("modifiers.json: {error}")))?;
        if file.version != crate::catalog::CATALOG_VERSION {
            return Err(CatalogError(format!(
                "this build reads catalog version {}",
                crate::catalog::CATALOG_VERSION
            )));
        }
        let limits = MagnitudeLimits {
            min_pct: file.magnitude_limits.min_pct,
            max_pct: file.magnitude_limits.max_pct,
        };
        if limits.min_pct > 0 || limits.max_pct < 0 || limits.min_pct <= -100 {
            return Err(CatalogError(
                "modifiers.json: magnitude limits must straddle zero and stay above -100".into(),
            ));
        }

        let mut modifiers: Vec<ModifierDefinition> = Vec::with_capacity(file.modifiers.len());
        for record in file.modifiers {
            let bad = |what: &str| CatalogError(format!("modifier {:?} {what}", record.id));
            if modifiers.iter().any(|existing| existing.id == record.id) {
                return Err(bad("is listed twice"));
            }
            let kind = ModifierKind::from_id(&record.kind).ok_or_else(|| {
                bad(&format!(
                    "names kind {:?}, which this build does not have",
                    record.kind
                ))
            })?;
            let params = parse_params(kind, record.params).map_err(|error| bad(&error))?;
            if record.introduced_at_level < 1 {
                return Err(bad("is introduced below level 1"));
            }
            if !(record.scaling_per_level >= 0.0 && record.scaling_per_level.is_finite()) {
                return Err(bad("has a negative scaling"));
            }
            let reroll = record.reroll;
            if !(0.0..=1.0).contains(&reroll.strengthen_chance)
                || reroll.min_pct == 0
                || reroll.min_pct > reroll.max_pct
            {
                return Err(bad("has a reroll that cannot work"));
            }
            modifiers.push(ModifierDefinition {
                id: record.id,
                name: record.name,
                description: record.description,
                icon: record.icon,
                kind,
                introduced_at_level: record.introduced_at_level,
                params,
                scaling_per_level: record.scaling_per_level,
                amplifiable: record.amplifiable,
                reducible: record.reducible,
                reroll: RerollRules {
                    strengthen_chance: reroll.strengthen_chance,
                    min_pct: reroll.min_pct,
                    max_pct: reroll.max_pct,
                },
            });
        }

        let mut progression: Vec<ProgressionRow> = Vec::with_capacity(file.progression.len());
        for (index, row) in file.progression.into_iter().enumerate() {
            if row.level as usize != index + 1 {
                return Err(CatalogError(format!(
                    "modifiers.json: progression row {} is for level {}; levels must run 1, 2, 3 …",
                    index + 1,
                    row.level
                )));
            }
            if !(row.health_mult > 0.0 && row.damage_mult > 0.0) {
                return Err(CatalogError(format!(
                    "modifiers.json: progression level {} has a non-positive multiplier",
                    row.level
                )));
            }
            if let Some(previous) = progression.last()
                && (row.health_mult < previous.health_mult
                    || row.damage_mult < previous.damage_mult)
            {
                return Err(CatalogError(format!(
                    "modifiers.json: progression level {} lowers a multiplier; they may only rise",
                    row.level
                )));
            }
            progression.push(ProgressionRow {
                level: row.level,
                modifier_slots: row.modifier_slots,
                health_mult: row.health_mult,
                damage_mult: row.damage_mult,
                notes: row.notes,
            });
        }
        let eternal = file
            .eternal
            .map(|record| {
                let rules = EternalRules {
                    from_level: record.from_level,
                    odd_add_slots: record.odd.add_modifier_slots,
                    even_health_step: record.even.health_mult_step,
                    even_damage_step: record.even.damage_mult_step,
                    max_modifier_slots: record.max_modifier_slots,
                };
                let next = progression.len() as u32 + 1;
                let last_slots = progression.last().map_or(0, |row| row.modifier_slots);
                if progression.is_empty() || rules.from_level != next {
                    return Err(CatalogError(format!(
                        "modifiers.json: eternal must start at level {next}, right after the table"
                    )));
                }
                if !(rules.even_health_step > 0.0
                    && rules.even_health_step.is_finite()
                    && rules.even_damage_step > 0.0
                    && rules.even_damage_step.is_finite())
                {
                    return Err(CatalogError(
                        "modifiers.json: eternal steps must be above zero".into(),
                    ));
                }
                if rules.max_modifier_slots < last_slots {
                    return Err(CatalogError(
                        "modifiers.json: eternal max_modifier_slots is below the table's last row"
                            .into(),
                    ));
                }
                Ok(rules)
            })
            .transpose()?;
        let config = ModifierConfig {
            reroll_unlock_fraction: file.config.reroll_unlock_fraction,
            reroll_resource: file.config.reroll_resource,
            reroll_cost: file.config.reroll_cost,
            max_rerolls_per_run: file.config.max_rerolls_per_run,
            reroll_reward_bonus_pct: file.config.reroll_reward_bonus_pct,
            cleanse_gold_cost: file.config.cleanse_gold_cost,
        };
        if !(0.0..=1.0).contains(&config.reroll_unlock_fraction)
            || config.reroll_resource.trim().is_empty()
            || config.reroll_cost == 0
        {
            return Err(CatalogError(
                "modifiers.json: config needs an unlock fraction in 0..1, a reroll resource and a cost above zero"
                    .into(),
            ));
        }
        Ok(Self {
            modifiers,
            progression,
            eternal,
            limits,
            config,
        })
    }

    /// `mods` as a key of `level` may carry them: unknown and repeated modifiers dropped,
    /// magnitudes held to the limits, no more than the level has slots for.
    ///
    /// What the realm runs on a snapshot the API handed it; the API's own writes are already
    /// in this shape, so on a healthy system it changes nothing.
    #[must_use]
    pub fn sanitize(&self, level: u32, mods: &[KeyModifier]) -> Vec<KeyModifier> {
        let mut out: Vec<KeyModifier> = Vec::with_capacity(mods.len());
        for modifier in mods {
            if self.modifier(&modifier.id).is_none() || out.iter().any(|m| m.id == modifier.id) {
                continue;
            }
            out.push(KeyModifier {
                id: modifier.id.clone(),
                magnitude_pct: modifier
                    .magnitude_pct
                    .clamp(self.limits.min_pct, self.limits.max_pct),
            });
        }
        out.truncate(self.slots(level));
        out
    }

    /// Whether `progress` (`(killed, total)`) has reached the reroll's unlock fraction.
    #[must_use]
    pub fn reroll_unlocked(&self, progress: (u32, u32)) -> bool {
        progress.1 > 0 && kill_fraction(progress) >= self.config.reroll_unlock_fraction
    }

    /// Whether a reroll is allowed, and if so what the dice made of it.
    ///
    /// `roll_a` and `roll_b` are the server's numbers in `[0, 1)` (see [`reroll_outcome`]).
    /// Checked in the order a player would fix them: the dungeon is cleared enough, the run
    /// has a reroll left, the modifier exists and can move, the caller can pay.
    /// `resource_name` is what the cost is called in the refusal.
    ///
    /// # Errors
    ///
    /// A dungeon refusal with a sentence a player can read.
    pub fn plan_reroll(
        &self,
        request: &RerollRequest<'_>,
        resource_name: &str,
        roll_a: f64,
        roll_b: f64,
    ) -> Result<Reroll, RealmError> {
        let config = &self.config;
        if !self.reroll_unlocked(request.progress) {
            let percent = (config.reroll_unlock_fraction * 100.0).round();
            return Err(RealmError::dungeon(format!(
                "A reroll unlocks once {percent:.0}% of the dungeon is cleared."
            )));
        }
        if request.rerolls_done >= config.max_rerolls_per_run {
            return Err(RealmError::dungeon(
                "This key has already been rerolled as often as a run allows.",
            ));
        }
        let key_modifier = request
            .mods
            .get(request.index)
            .ok_or_else(|| RealmError::dungeon("That modifier is not on this key."))?;
        let definition = self
            .modifier(&key_modifier.id)
            .ok_or_else(|| RealmError::dungeon("That modifier is not on this key."))?;
        if !definition.amplifiable && !definition.reducible {
            return Err(RealmError::dungeon("That modifier cannot be rerolled."));
        }
        let available = request.balance.saturating_sub(request.spent);
        if available < u64::from(config.reroll_cost) {
            return Err(RealmError::dungeon(format!(
                "A reroll costs {} {resource_name}, and you have {available} to spend.",
                config.reroll_cost
            )));
        }
        let delta = reroll_outcome(definition, roll_a, roll_b);
        Ok(Reroll {
            index: request.index,
            id: key_modifier.id.clone(),
            delta_pct: delta,
            magnitude_pct: self.apply_reroll(key_modifier.magnitude_pct, delta),
            cost: config.reroll_cost,
        })
    }

    /// Whether a key carrying `mods` may be cleansed by a character holding `gold`.
    ///
    /// # Errors
    ///
    /// A dungeon refusal: nothing to remove (not charged), or too little gold.
    pub fn check_cleanse(&self, mods: &[KeyModifier], gold: u64) -> Result<(), RealmError> {
        if mods.is_empty() {
            return Err(RealmError::dungeon("This key has no modifiers to cleanse."));
        }
        if gold < u64::from(self.config.cleanse_gold_cost) {
            return Err(RealmError::dungeon(format!(
                "Cleansing a key costs {} gold.",
                self.config.cleanse_gold_cost
            )));
        }
        Ok(())
    }

    /// The modifiers a key of `level` starts with, rolled from `seed`.
    ///
    /// Exactly `min(slots, eligible)` of them: the eligible are the modifiers introduced at
    /// or below `level`, each at most once, chosen by a partial shuffle of the catalog order
    /// driven by `seed`. The same seed, level and catalog give the same key.
    #[must_use]
    pub fn roll_key_modifiers(&self, level: u32, seed: u64) -> Vec<KeyModifier> {
        let mut eligible: Vec<&ModifierDefinition> = self
            .modifiers
            .iter()
            .filter(|definition| definition.introduced_at_level <= level)
            .collect();
        let count = self.slots(level).min(eligible.len());
        let mut random = SplitMix64(seed);
        let mut picked = Vec::with_capacity(count);
        for taken in 0..count {
            let remaining = eligible.len() - taken;
            let choice = taken + (random.next() % remaining as u64) as usize;
            eligible.swap(taken, choice);
            picked.push(KeyModifier::base(&eligible[taken].id));
        }
        picked
    }

    /// The modifiers in force for a run of key `level` carrying `mods`, ready to hand to
    /// `Simulation::set_modifiers`.
    ///
    /// Strength is the level's scaling times the reroll magnitude (see the module docs).
    /// A modifier the catalog does not know, or that appears twice, is skipped rather than
    /// failing the run: a key written by an older catalog must still open a dungeon.
    #[must_use]
    pub fn resolve(&self, level: u32, mods: &[KeyModifier]) -> Vec<ActiveModifier> {
        let mut active: Vec<ActiveModifier> = Vec::with_capacity(mods.len());
        for key_modifier in mods {
            let Some(definition) = self.modifier(&key_modifier.id) else {
                continue;
            };
            if active
                .iter()
                .any(|existing| existing.kind == definition.kind)
            {
                continue;
            }
            let steps = level.saturating_sub(definition.introduced_at_level) as f32;
            let level_scale = 1.0 + definition.scaling_per_level * steps;
            let pct = key_modifier
                .magnitude_pct
                .clamp(self.limits.min_pct, self.limits.max_pct);
            let magnitude = level_scale * (1.0 + f32::from(pct) / 100.0);
            active.push(
                ActiveModifier::new(definition.kind, magnitude).with_params(definition.params),
            );
        }
        active
    }

    /// `current_pct` after a reroll that changed it by `delta`, held to the catalog's limits.
    #[must_use]
    pub fn apply_reroll(&self, current_pct: i16, delta: i16) -> i16 {
        current_pct
            .saturating_add(delta)
            .clamp(self.limits.min_pct, self.limits.max_pct)
    }

    /// A key one level lower, keeping only as many modifiers as that level carries.
    ///
    /// Never below `min_level`. Modifiers are kept in order, so the ones the key was rolled
    /// with first survive.
    #[must_use]
    pub fn downgrade(
        &self,
        min_level: u32,
        level: u32,
        mods: &[KeyModifier],
    ) -> (u32, Vec<KeyModifier>) {
        let lowered = level.saturating_sub(1).max(min_level.max(1));
        let keep = self.slots(lowered).min(mods.len());
        (lowered, mods[..keep].to_vec())
    }
}

/// What one reroll of `definition` does to a modifier: a signed change in per cent.
///
/// `roll_a` in `[0, 1)` decides the direction (strengthen below the definition's chance,
/// 60 % by default) and `roll_b` in `[0, 1)` the size, uniform over `min_pct..=max_pct`.
/// A direction the modifier does not allow becomes the other one; a modifier that allows
/// neither does not change (`0`). The result is a *change*: hold the total to the catalog's
/// limits with [`ModifierCatalog::apply_reroll`].
#[must_use]
pub fn reroll_outcome(definition: &ModifierDefinition, roll_a: f64, roll_b: f64) -> i16 {
    let wants_stronger = roll_a < definition.reroll.strengthen_chance;
    let stronger = match (wants_stronger, definition.amplifiable, definition.reducible) {
        (_, false, false) => return 0,
        (true, true, _) | (false, _, false) => true,
        (true, false, _) | (false, _, true) => false,
    };
    let (min, max) = (definition.reroll.min_pct, definition.reroll.max_pct);
    let span = f64::from(max - min + 1);
    let size = min + (roll_b.clamp(0.0, 1.0 - f64::EPSILON) * span).floor() as u32;
    let size = i16::try_from(size.min(max)).unwrap_or(i16::MAX);
    if stronger { size } else { -size }
}

/// A key with every modifier removed.
#[must_use]
pub fn cleanse(_mods: &[KeyModifier]) -> Vec<KeyModifier> {
    Vec::new()
}

/// A small deterministic generator: enough to shuffle a handful of ids from a seed.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn parse_params(kind: ModifierKind, mut map: Map<String, Value>) -> Result<ModifierParams, String> {
    let mut number = |key: &str, default: f32| -> Result<f32, String> {
        match map.remove(key) {
            None => Ok(default),
            Some(Value::Number(n)) => n
                .as_f64()
                .map(|v| v as f32)
                .filter(|v| v.is_finite() && *v >= 0.0)
                .ok_or_else(|| format!("param {key:?} must be a non-negative number")),
            Some(_) => Err(format!("param {key:?} must be a number")),
        }
    };
    let params = match kind {
        ModifierKind::RotatingBeams => {
            let d = BeamParams::default();
            let beams = number("beams", f32::from(d.beams))?;
            if !(1.0..=12.0).contains(&beams) {
                return Err("param \"beams\" must be 1 to 12".into());
            }
            ModifierParams::RotatingBeams(BeamParams {
                beams: beams as u8,
                radius: number("radius", d.radius)?,
                width: number("width", d.width)?,
                angular_speed_deg: number("angular_speed_deg", d.angular_speed_deg)?,
                warmup: number("warmup", d.warmup)?,
                damage_pct_max_hp: number("damage_pct_max_hp", d.damage_pct_max_hp)?,
                max_hit_pct: number("max_hit_pct", d.max_hit_pct)?,
            })
        }
        ModifierKind::PlagueExplosion => {
            let d = PlagueParams::default();
            ModifierParams::PlagueExplosion(PlagueParams {
                fuse: number("fuse", d.fuse)?,
                explosion_radius: number("explosion_radius", d.explosion_radius)?,
                explosion_pct_max_hp: number("explosion_pct_max_hp", d.explosion_pct_max_hp)?,
                cloud_radius: number("cloud_radius", d.cloud_radius)?,
                cloud_duration: number("cloud_duration", d.cloud_duration)?,
                cloud_pct_max_hp_per_sec: number(
                    "cloud_pct_max_hp_per_sec",
                    d.cloud_pct_max_hp_per_sec,
                )?,
                heal_reduction: number("heal_reduction", d.heal_reduction)?.min(1.0),
                debuff_linger: number("debuff_linger", d.debuff_linger)?,
                max_hit_pct: number("max_hit_pct", d.max_hit_pct)?,
            })
        }
        ModifierKind::Bolstering => {
            let d = BolsteringParams::default();
            let max_stacks = number("max_stacks", f32::from(d.max_stacks))?;
            if !(1.0..=20.0).contains(&max_stacks) {
                return Err("param \"max_stacks\" must be 1 to 20".into());
            }
            ModifierParams::Bolstering(BolsteringParams {
                radius: number("radius", d.radius)?,
                health_pct: number("health_pct", d.health_pct)?,
                damage_pct: number("damage_pct", d.damage_pct)?,
                max_stacks: max_stacks as u8,
            })
        }
        ModifierKind::NecroticPools => {
            let d = NecroticParams::default();
            let radius = number("radius", d.radius)?;
            if radius <= 0.0 {
                return Err("param \"radius\" must be above zero".into());
            }
            ModifierParams::NecroticPools(NecroticParams {
                warning: number("warning", d.warning)?,
                radius,
                duration: number("duration", d.duration)?,
                pct_max_hp_per_sec: number("pct_max_hp_per_sec", d.pct_max_hp_per_sec)?,
                max_hit_pct: number("max_hit_pct", d.max_hit_pct)?,
            })
        }
        ModifierKind::Raging => {
            let d = RagingParams::default();
            let threshold = number("threshold", d.threshold)?;
            if !(threshold > 0.0 && threshold < 1.0) {
                return Err("param \"threshold\" must be between 0 and 1".into());
            }
            ModifierParams::Raging(RagingParams {
                threshold,
                damage_pct: number("damage_pct", d.damage_pct)?,
            })
        }
        ModifierKind::SpitefulCasters => {
            let d = SpitefulParams::default();
            ModifierParams::SpitefulCasters(SpitefulParams {
                cast_speed_pct: number("cast_speed_pct", d.cast_speed_pct)?,
                debuff_duration: number("debuff_duration", d.debuff_duration)?,
                heal_reduction: number("heal_reduction", d.heal_reduction)?.min(1.0),
            })
        }
    };
    if let Some(unknown) = map.keys().next() {
        return Err(format!("has an unknown param {unknown:?}"));
    }
    Ok(params)
}

#[derive(Deserialize)]
struct File {
    version: u32,
    #[serde(default)]
    magnitude_limits: LimitsRecord,
    #[serde(default)]
    config: ConfigRecord,
    modifiers: Vec<ModifierRecord>,
    progression: Vec<RowRecord>,
    #[serde(default)]
    eternal: Option<EternalRecord>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EternalRecord {
    from_level: u32,
    odd: EternalOddRecord,
    even: EternalEvenRecord,
    max_modifier_slots: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EternalOddRecord {
    add_modifier_slots: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EternalEvenRecord {
    health_mult_step: f32,
    damage_mult_step: f32,
}

#[derive(Deserialize)]
struct LimitsRecord {
    min_pct: i16,
    max_pct: i16,
}

impl Default for LimitsRecord {
    fn default() -> Self {
        let limits = MagnitudeLimits::default();
        Self {
            min_pct: limits.min_pct,
            max_pct: limits.max_pct,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigRecord {
    reroll_unlock_fraction: f32,
    reroll_resource: String,
    reroll_cost: u32,
    max_rerolls_per_run: u32,
    reroll_reward_bonus_pct: u32,
    cleanse_gold_cost: u32,
}

impl Default for ConfigRecord {
    fn default() -> Self {
        let config = ModifierConfig::default();
        Self {
            reroll_unlock_fraction: config.reroll_unlock_fraction,
            reroll_resource: config.reroll_resource,
            reroll_cost: config.reroll_cost,
            max_rerolls_per_run: config.max_rerolls_per_run,
            reroll_reward_bonus_pct: config.reroll_reward_bonus_pct,
            cleanse_gold_cost: config.cleanse_gold_cost,
        }
    }
}

#[derive(Deserialize)]
struct ModifierRecord {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    icon: String,
    kind: String,
    introduced_at_level: u32,
    #[serde(default)]
    params: Map<String, Value>,
    #[serde(default)]
    scaling_per_level: f32,
    #[serde(default = "yes")]
    amplifiable: bool,
    #[serde(default = "yes")]
    reducible: bool,
    reroll: RerollRecord,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Clone, Copy)]
struct RerollRecord {
    strengthen_chance: f64,
    min_pct: u32,
    max_pct: u32,
}

#[derive(Deserialize)]
struct RowRecord {
    level: u32,
    modifier_slots: u32,
    health_mult: f32,
    damage_mult: f32,
    #[serde(default)]
    notes: String,
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn catalog() -> ModifierCatalog {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .modifiers()
            .clone()
    }

    fn beams(catalog: &ModifierCatalog) -> &ModifierDefinition {
        catalog.modifier("rotating_beams").expect("beams exist")
    }

    /// A deterministic stream of numbers in `[0, 1)`, for distribution tests.
    fn unit_stream(seed: u64) -> impl FnMut() -> f64 {
        let mut random = SplitMix64(seed);
        move || (random.next() >> 11) as f64 / (1_u64 << 53) as f64
    }

    #[test]
    fn the_shipped_modifiers_read() {
        let catalog = catalog();
        assert_eq!(catalog.modifiers().len(), 6);
        for (id, introduced) in [
            ("rotating_beams", 3),
            ("plague_explosion", 3),
            ("bolstering", 5),
            ("raging", 7),
            ("necrotic_pools", 11),
            ("spiteful_casters", 13),
        ] {
            let definition = catalog.modifier(id).expect("a shipped modifier");
            assert_eq!(definition.introduced_at_level, introduced, "{id}");
            assert_eq!(definition.kind.id(), id);
            assert!(
                definition.scaling_per_level > 0.0,
                "{id} scales with the key"
            );
        }
        let beams = beams(&catalog);
        assert_eq!(beams.kind, ModifierKind::RotatingBeams);
        assert_eq!(beams.reroll.min_pct, 10);
        assert_eq!(beams.reroll.max_pct, 25);
        assert!((beams.reroll.strengthen_chance - 0.60).abs() < f64::EPSILON);
        assert_eq!(
            catalog.limits(),
            MagnitudeLimits {
                min_pct: -50,
                max_pct: 100
            }
        );
        let ModifierParams::RotatingBeams(params) = beams.params else {
            panic!("beams carry beam params");
        };
        assert_eq!((params.radius, params.width, params.beams), (9.0, 0.9, 3));
    }

    #[test]
    fn the_progression_follows_the_design() {
        let catalog = catalog();
        let slots: Vec<usize> = (1..=20).map(|level| catalog.slots(level)).collect();
        assert_eq!(&slots[..9], &[0, 0, 1, 1, 2, 2, 2, 2, 3]);
        assert_eq!(
            catalog.progression().len(),
            12,
            "the table is the normal keys"
        );
        assert!(
            slots.windows(2).all(|pair| pair[0] <= pair[1]),
            "never fewer as keys rise"
        );
        let health: Vec<f32> = (1..=20)
            .map(|level| catalog.row_for(level).expect("a row").health_mult)
            .collect();
        assert_eq!(health[0], 1.0);
        assert!(health.windows(2).all(|pair| pair[0] <= pair[1]));
        // Before the table the first row holds.
        assert_eq!(catalog.row_for(0), catalog.row_for(1));
    }

    #[test]
    fn eternal_levels_alternate_a_slot_and_a_step() {
        let catalog = catalog();
        let rows: Vec<ProgressionRow> = (10..=16)
            .map(|level| catalog.row_for(level).expect("a row"))
            .collect();
        let slots: Vec<u32> = rows.iter().map(|row| row.modifier_slots).collect();
        // 10-12 are the table; 13 and 15 (odd) add a slot; 14 and 16 (even) do not.
        assert_eq!(slots, [3, 3, 3, 4, 4, 5, 5]);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let (twelve, thirteen, fourteen, fifteen, sixteen) =
            (&rows[2], &rows[3], &rows[4], &rows[5], &rows[6]);
        // An odd level carries the multipliers; an even level raises them by the steps.
        assert!(near(thirteen.health_mult, twelve.health_mult));
        assert!(near(thirteen.damage_mult, twelve.damage_mult));
        assert!(near(fourteen.health_mult, twelve.health_mult + 0.12));
        assert!(near(fourteen.damage_mult, twelve.damage_mult + 0.06));
        assert!(near(fifteen.health_mult, fourteen.health_mult));
        assert!(near(sixteen.health_mult, twelve.health_mult + 0.24));
        assert!(near(sixteen.damage_mult, twelve.damage_mult + 0.12));
        // Rolling a key takes the slot count from the same rows.
        assert_eq!(catalog.slots(13), 4);
        let rules = KeyRules {
            min_level: 1,
            max_level: 99,
            health_per_level: 0.10,
            damage_per_level: 0.05,
        };
        assert_eq!(
            catalog.key_scaling(&rules, 14),
            (fourteen.health_mult, fourteen.damage_mult)
        );
    }

    #[test]
    fn eternal_rows_never_fall_and_slots_are_capped() {
        let catalog = catalog();
        let cap = catalog.eternal().expect("eternal rules").max_modifier_slots;
        let mut previous = catalog.row_for(12).expect("a row");
        for level in 13..=120 {
            let row = catalog.row_for(level).expect("a row");
            assert_eq!(row.level, level);
            assert!(row.health_mult >= previous.health_mult);
            assert!(row.damage_mult >= previous.damage_mult);
            assert!(row.modifier_slots >= previous.modifier_slots);
            assert!(row.modifier_slots <= cap);
            previous = row;
        }
        let ninety_nine = catalog.row_for(99).expect("a row");
        assert_eq!(ninety_nine.modifier_slots, cap);
        // 43 even levels from 14 to 98: 2.02 + 43 * 0.12 and 1.51 + 43 * 0.06.
        assert!(
            (ninety_nine.health_mult - 7.18).abs() < 1e-3,
            "{ninety_nine:?}"
        );
        assert!(
            (ninety_nine.damage_mult - 4.09).abs() < 1e-3,
            "{ninety_nine:?}"
        );
        // The largest level is arithmetic, not a loop.
        let huge = catalog.row_for(u32::MAX).expect("a row");
        assert_eq!(huge.modifier_slots, cap);
        assert!(huge.health_mult.is_finite());
    }

    #[test]
    fn eternal_rules_are_validated() {
        let good = include_str!("../../../assets/catalog/modifiers.json");
        assert!(ModifierCatalog::parse(good).is_ok());
        for (from, to, why) in [
            (
                "\"from_level\": 13",
                "\"from_level\": 14",
                "a gap after the table",
            ),
            (
                "\"from_level\": 13",
                "\"from_level\": 12",
                "overlapping the table",
            ),
            (
                "\"health_mult_step\": 0.12",
                "\"health_mult_step\": 0.0",
                "a zero step",
            ),
            (
                "\"damage_mult_step\": 0.06",
                "\"damage_mult_step\": -0.1",
                "a negative step",
            ),
            (
                "\"max_modifier_slots\": 6",
                "\"max_modifier_slots\": 2",
                "a cap below the table",
            ),
            (
                "\"level\": 12, \"modifier_slots\": 3, \"health_mult\": 2.02",
                "\"level\": 12, \"modifier_slots\": 3, \"health_mult\": 1.0",
                "a multiplier that falls",
            ),
        ] {
            assert!(
                ModifierCatalog::parse(&good.replacen(from, to, 1)).is_err(),
                "{why}"
            );
        }
    }

    #[test]
    fn key_scaling_reads_the_table_and_falls_back_to_the_dungeon_rule() {
        let catalog = catalog();
        let rules = KeyRules {
            min_level: 1,
            max_level: 20,
            health_per_level: 0.10,
            damage_per_level: 0.05,
        };
        assert_eq!(catalog.key_scaling(&rules, 1), (1.0, 1.0));
        let row = catalog.row_for(10).expect("a row");
        assert_eq!(
            catalog.key_scaling(&rules, 10),
            (row.health_mult, row.damage_mult)
        );
        let empty = ModifierCatalog {
            progression: Vec::new(),
            ..catalog
        };
        assert_eq!(
            empty.key_scaling(&rules, 11),
            crate::key::scaling(&rules, 11)
        );
    }

    #[test]
    fn a_roll_is_deterministic_unique_eligible_and_as_long_as_the_slots() {
        let catalog = catalog();
        for level in 1..=20 {
            for seed in 0..50 {
                let rolled = catalog.roll_key_modifiers(level, seed);
                assert_eq!(
                    rolled,
                    catalog.roll_key_modifiers(level, seed),
                    "same seed, same key"
                );
                let eligible = catalog
                    .modifiers()
                    .iter()
                    .filter(|m| m.introduced_at_level <= level)
                    .count();
                assert_eq!(rolled.len(), catalog.slots(level).min(eligible));
                let mut ids: Vec<&str> = rolled.iter().map(|m| m.id.as_str()).collect();
                ids.sort_unstable();
                ids.dedup();
                assert_eq!(ids.len(), rolled.len(), "no duplicates");
                assert!(rolled.iter().all(|m| m.magnitude_pct == 0));
            }
        }
        assert!(catalog.roll_key_modifiers(1, 1).is_empty());
        assert!(catalog.roll_key_modifiers(2, 1).is_empty());
        assert_eq!(catalog.roll_key_modifiers(3, 1).len(), 1);
        assert_eq!(catalog.roll_key_modifiers(5, 1).len(), 2);
    }

    #[test]
    fn every_shipped_modifier_resolves_to_its_own_kind_and_numbers() {
        let catalog = catalog();
        for definition in catalog.modifiers() {
            let level = definition.introduced_at_level + 2;
            let active = catalog.resolve(level, &[KeyModifier::base(&definition.id)]);
            assert_eq!(active.len(), 1, "{}", definition.id);
            assert_eq!(active[0].kind, definition.kind);
            assert_eq!(active[0].params, definition.params);
            // Two levels above its introduction it is stronger than at its base.
            assert!(active[0].magnitude > 1.0, "{}", definition.id);
        }
    }

    #[test]
    fn a_roll_respects_when_a_modifier_is_introduced() {
        let mut catalog = catalog();
        for definition in catalog.modifiers.iter_mut().skip(1) {
            definition.introduced_at_level = 7;
        }
        for seed in 0..200 {
            let rolled = catalog.roll_key_modifiers(5, seed);
            assert_eq!(rolled, vec![KeyModifier::base("rotating_beams")]);
        }
        assert_eq!(catalog.roll_key_modifiers(7, 1).len(), 2);
    }

    #[test]
    fn rolls_across_seeds_choose_each_modifier_about_equally() {
        let catalog = catalog();
        let mut beams = 0;
        for seed in 0..2_000 {
            if catalog.roll_key_modifiers(3, seed)[0].id == "rotating_beams" {
                beams += 1;
            }
        }
        assert!((850..=1_150).contains(&beams), "{beams} of 2000");
    }

    #[test]
    fn resolve_applies_level_scaling_and_reroll_magnitude() {
        let catalog = catalog();
        let at_base = catalog.resolve(3, &[KeyModifier::base("rotating_beams")]);
        assert_eq!(at_base.len(), 1);
        assert!(
            (at_base[0].magnitude - 1.0).abs() < 1e-6,
            "base at its introduction level"
        );

        // Level 11 is eight levels above 3 at five per cent: 1.4; a +20 % reroll on top.
        let stronger = catalog.resolve(
            11,
            &[KeyModifier {
                id: "rotating_beams".into(),
                magnitude_pct: 20,
            }],
        );
        assert!((stronger[0].magnitude - 1.4 * 1.2).abs() < 1e-5);

        let weaker = catalog.resolve(
            3,
            &[KeyModifier {
                id: "plague_explosion".into(),
                magnitude_pct: -30,
            }],
        );
        assert!((weaker[0].magnitude - 0.7).abs() < 1e-6);
        assert_eq!(weaker[0].kind, ModifierKind::PlagueExplosion);
    }

    #[test]
    fn resolve_skips_what_it_does_not_know_and_what_repeats() {
        let catalog = catalog();
        let active = catalog.resolve(
            5,
            &[
                KeyModifier::base("rotating_beams"),
                KeyModifier::base("rotating_beams"),
                KeyModifier::base("from_the_future"),
            ],
        );
        assert_eq!(active.len(), 1);
        // And a stored magnitude outside the limits is held to them.
        let capped = catalog.resolve(
            3,
            &[KeyModifier {
                id: "rotating_beams".into(),
                magnitude_pct: 9_999,
            }],
        );
        assert!((capped[0].magnitude - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_reroll_is_sixty_forty_and_ten_to_twenty_five() {
        let catalog = catalog();
        let definition = beams(&catalog);
        let mut roll = unit_stream(0xC0FFEE);
        let (mut up, mut down) = (0, 0);
        let mut seen = [false; 26];
        for _ in 0..10_000 {
            let delta = reroll_outcome(definition, roll(), roll());
            assert!((10..=25).contains(&delta.abs()), "{delta}");
            seen[usize::try_from(delta.abs()).expect("small")] = true;
            if delta > 0 {
                up += 1;
            } else {
                down += 1;
            }
        }
        let share = f64::from(up) / f64::from(up + down);
        assert!(
            (0.57..=0.63).contains(&share),
            "strengthened {share:.3} of the time"
        );
        assert!(
            seen[10..=25].iter().all(|hit| *hit),
            "every size from 10 to 25 comes up"
        );
    }

    #[test]
    fn a_reroll_obeys_the_amplifiable_and_reducible_flags() {
        let catalog = catalog();
        let mut definition = beams(&catalog).clone();
        definition.amplifiable = false;
        assert!(
            (0..100).all(|i| reroll_outcome(&definition, f64::from(i) / 100.0, 0.5) < 0),
            "cannot be strengthened, so it is weakened"
        );
        definition.amplifiable = true;
        definition.reducible = false;
        assert!(
            (0..100).all(|i| reroll_outcome(&definition, f64::from(i) / 100.0, 0.5) > 0),
            "cannot be weakened, so it is strengthened"
        );
        definition.amplifiable = false;
        assert_eq!(
            reroll_outcome(&definition, 0.1, 0.5),
            0,
            "it cannot change at all"
        );
    }

    #[test]
    fn the_direction_and_size_follow_the_rolls() {
        let catalog = catalog();
        let definition = beams(&catalog);
        assert_eq!(reroll_outcome(definition, 0.0, 0.0), 10);
        assert_eq!(reroll_outcome(definition, 0.59, 0.999_999), 25);
        assert_eq!(reroll_outcome(definition, 0.60, 0.0), -10);
        assert_eq!(reroll_outcome(definition, 0.99, 0.999_999), -25);
        // A roll at or past one never indexes past the range.
        assert_eq!(reroll_outcome(definition, 0.0, 1.0), 25);
    }

    #[test]
    fn the_total_magnitude_is_clamped_to_the_limits() {
        let catalog = catalog();
        assert_eq!(catalog.apply_reroll(0, 17), 17);
        assert_eq!(catalog.apply_reroll(95, 25), 100);
        assert_eq!(catalog.apply_reroll(-45, -25), -50);
        assert_eq!(catalog.apply_reroll(i16::MAX, 25), 100);
        assert_eq!(catalog.apply_reroll(i16::MIN, -25), -50);
    }

    #[test]
    fn downgrading_lowers_the_level_and_trims_the_modifiers_to_fit() {
        let catalog = catalog();
        let mods = vec![
            KeyModifier::base("rotating_beams"),
            KeyModifier::base("plague_explosion"),
        ];
        // Level 5 carries two; level 4 carries one.
        let (level, kept) = catalog.downgrade(1, 5, &mods);
        assert_eq!(level, 4);
        assert_eq!(kept, vec![KeyModifier::base("rotating_beams")]);
        // Level 6 -> 5 keeps both.
        let (level, kept) = catalog.downgrade(1, 6, &mods);
        assert_eq!((level, kept.len()), (5, 2));
        // Level 3 -> 2 carries none.
        assert_eq!(catalog.downgrade(1, 3, &mods), (2, Vec::new()));
        // Never below the minimum level.
        assert_eq!(catalog.downgrade(1, 1, &mods).0, 1);
        assert_eq!(catalog.downgrade(1, 0, &mods).0, 1);
        assert_eq!(catalog.downgrade(4, 4, &mods).0, 4);
    }

    #[test]
    fn cleansing_removes_everything() {
        let mods = vec![KeyModifier::base("rotating_beams")];
        assert!(cleanse(&mods).is_empty());
        assert!(cleanse(&[]).is_empty());
    }

    #[test]
    fn key_modifiers_are_json_for_the_database_column() {
        let mods = vec![KeyModifier {
            id: "plague_explosion".into(),
            magnitude_pct: -12,
        }];
        let json = serde_json::to_string(&mods).expect("serializes");
        assert_eq!(json, r#"[{"id":"plague_explosion","magnitude_pct":-12}]"#);
        let back: Vec<KeyModifier> = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, mods);
        // The reserved, always-empty column and a bare id both still read.
        assert!(
            serde_json::from_str::<Vec<KeyModifier>>("[]")
                .expect("empty")
                .is_empty()
        );
        let bare: Vec<KeyModifier> =
            serde_json::from_str(r#"[{"id":"rotating_beams"}]"#).expect("bare");
        assert_eq!(bare[0].magnitude_pct, 0);
    }

    #[test]
    fn broken_catalogs_are_refused() {
        let good = include_str!("../../../assets/catalog/modifiers.json");
        assert!(ModifierCatalog::parse(good).is_ok());
        for (from, to, why) in [
            (
                "\"kind\": \"plague_explosion\"",
                "\"kind\": \"nope\"",
                "unknown kind",
            ),
            ("\"fuse\": 5.0", "\"fuze\": 5.0", "unknown param"),
            (
                "\"id\": \"plague_explosion\"",
                "\"id\": \"rotating_beams\"",
                "duplicate id",
            ),
            (
                "\"min_pct\": 10, \"max_pct\": 25",
                "\"min_pct\": 30, \"max_pct\": 25",
                "inverted reroll",
            ),
            (
                "{ \"level\": 2,",
                "{ \"level\": 9,",
                "progression out of order",
            ),
            ("\"version\": 1", "\"version\": 9", "unknown version"),
        ] {
            assert!(
                ModifierCatalog::parse(&good.replacen(from, to, 1)).is_err(),
                "{why}"
            );
        }
    }

    // --- reroll gating, cleanse, config ---------------------------------------------------

    fn request(mods: &[KeyModifier]) -> RerollRequest<'_> {
        RerollRequest {
            mods,
            index: 0,
            progress: (5, 10),
            rerolls_done: 0,
            balance: 3,
            spent: 0,
        }
    }

    fn two_mods() -> Vec<KeyModifier> {
        vec![
            KeyModifier::base("rotating_beams"),
            KeyModifier::base("plague_explosion"),
        ]
    }

    #[test]
    fn the_shipped_config_is_what_the_design_says() {
        let catalog = catalog();
        let config = catalog.config();
        assert!((config.reroll_unlock_fraction - 0.5).abs() < 1e-6);
        assert_eq!(config.reroll_resource, "aether_shard");
        assert_eq!(config.reroll_cost, 1);
        assert_eq!(config.max_rerolls_per_run, 1);
        assert_eq!(config.reroll_reward_bonus_pct, 25);
        assert_eq!(config.cleanse_gold_cost, 100);
    }

    #[test]
    fn a_bad_config_is_refused() {
        let json = include_str!("../../../assets/catalog/modifiers.json");
        for (from, to) in [
            (
                "\"reroll_unlock_fraction\": 0.5",
                "\"reroll_unlock_fraction\": 1.5",
            ),
            ("\"reroll_cost\": 1", "\"reroll_cost\": 0"),
            (
                "\"reroll_resource\": \"aether_shard\"",
                "\"reroll_resource\": \"\"",
            ),
            ("\"cleanse_gold_cost\": 100", "\"cleanse_gold\": 100"),
        ] {
            assert!(json.contains(from), "{from}");
            assert!(
                ModifierCatalog::parse(&json.replace(from, to)).is_err(),
                "{to}"
            );
        }
    }

    #[test]
    fn a_reroll_is_refused_before_half_the_dungeon_is_dead() {
        let catalog = catalog();
        let mods = two_mods();
        for progress in [(0, 10), (4, 10), (49, 100), (0, 0)] {
            let error = catalog
                .plan_reroll(
                    &RerollRequest {
                        progress,
                        ..request(&mods)
                    },
                    "Aether Shards",
                    0.1,
                    0.5,
                )
                .expect_err("locked");
            assert!(
                error.message.contains("unlocks once 50%"),
                "{progress:?}: {error}"
            );
        }
        let ok = catalog
            .plan_reroll(&request(&mods), "Aether Shards", 0.1, 0.5)
            .expect("half is enough");
        assert_eq!(ok.id, "rotating_beams");
        assert!(ok.strengthened(), "0.1 is below the 60% strengthen chance");
        assert!((10..=25).contains(&ok.magnitude_pct));
    }

    #[test]
    fn a_reroll_needs_the_currency_the_caller_has_not_already_spent() {
        let catalog = catalog();
        let mods = two_mods();
        let broke = RerollRequest {
            balance: 0,
            ..request(&mods)
        };
        let error = catalog
            .plan_reroll(&broke, "Aether Shards", 0.1, 0.5)
            .expect_err("broke");
        assert!(error.message.contains("1 Aether Shards"), "{error}");
        let spent = RerollRequest {
            balance: 2,
            spent: 2,
            ..request(&mods)
        };
        assert!(
            catalog
                .plan_reroll(&spent, "Aether Shards", 0.1, 0.5)
                .is_err()
        );
        let last = RerollRequest {
            balance: 2,
            spent: 1,
            ..request(&mods)
        };
        assert!(
            catalog
                .plan_reroll(&last, "Aether Shards", 0.1, 0.5)
                .is_ok()
        );
        // A balance smaller than what was spent (it changed elsewhere) is just "nothing".
        let odd = RerollRequest {
            balance: 1,
            spent: 5,
            ..request(&mods)
        };
        assert!(
            catalog
                .plan_reroll(&odd, "Aether Shards", 0.1, 0.5)
                .is_err()
        );
    }

    #[test]
    fn a_run_allows_only_the_configured_number_of_rerolls() {
        let catalog = catalog();
        let mods = two_mods();
        let used = RerollRequest {
            rerolls_done: 1,
            ..request(&mods)
        };
        let error = catalog
            .plan_reroll(&used, "Aether Shards", 0.1, 0.5)
            .expect_err("used up");
        assert!(
            error.message.contains("as often as a run allows"),
            "{error}"
        );
    }

    #[test]
    fn a_reroll_names_a_real_modifier() {
        let catalog = catalog();
        let mods = two_mods();
        let missing = RerollRequest {
            index: 2,
            ..request(&mods)
        };
        assert!(catalog.plan_reroll(&missing, "x", 0.1, 0.5).is_err());
        let none: Vec<KeyModifier> = Vec::new();
        assert!(catalog.plan_reroll(&request(&none), "x", 0.1, 0.5).is_err());
        let unknown = vec![KeyModifier::base("no_such_modifier")];
        assert!(
            catalog
                .plan_reroll(&request(&unknown), "x", 0.1, 0.5)
                .is_err()
        );
    }

    #[test]
    fn a_weakening_reroll_lowers_the_magnitude_and_apply_touches_only_that_modifier() {
        let catalog = catalog();
        let mods = two_mods();
        let plan = catalog
            .plan_reroll(
                &RerollRequest {
                    index: 1,
                    ..request(&mods)
                },
                "x",
                0.9,
                0.0,
            )
            .expect("allowed");
        assert_eq!(plan.id, "plague_explosion");
        assert!(!plan.strengthened());
        assert_eq!(plan.delta_pct, -10);
        let after = plan.apply(&mods);
        assert_eq!(after[0], mods[0]);
        assert_eq!(after[1].magnitude_pct, -10);
        assert_eq!(plan.cost, 1);
    }

    #[test]
    fn rerolls_stay_within_the_magnitude_limits() {
        let catalog = catalog();
        let mods = vec![KeyModifier {
            id: "rotating_beams".into(),
            magnitude_pct: 95,
        }];
        let plan = catalog
            .plan_reroll(&request(&mods), "x", 0.0, 0.99)
            .expect("allowed");
        assert_eq!(plan.magnitude_pct, 100);
    }

    #[test]
    fn unlock_is_exactly_at_the_fraction() {
        let catalog = catalog();
        assert!(!catalog.reroll_unlocked((4, 9)));
        assert!(catalog.reroll_unlocked((5, 10)));
        assert!(catalog.reroll_unlocked((10, 10)));
        assert!(!catalog.reroll_unlocked((0, 0)));
        assert!((kill_fraction((1, 4)) - 0.25).abs() < 1e-6);
        assert_eq!(kill_fraction((3, 0)), 0.0);
    }

    #[test]
    fn a_cleanse_is_all_or_nothing_and_never_charges_for_a_clean_key() {
        let catalog = catalog();
        let mods = two_mods();
        assert!(catalog.check_cleanse(&mods, 100).is_ok());
        assert!(catalog.check_cleanse(&mods, 5_000).is_ok());
        let poor = catalog
            .check_cleanse(&mods, 99)
            .expect_err("99 gold is too little");
        assert!(poor.message.contains("100 gold"), "{poor}");
        let clean = catalog
            .check_cleanse(&[], 1_000)
            .expect_err("nothing to remove");
        assert!(clean.message.contains("no modifiers"), "{clean}");
        assert!(cleanse(&mods).is_empty());
    }

    #[test]
    fn a_downgrade_never_goes_below_the_minimum_and_drops_the_extra_modifiers() {
        let catalog = catalog();
        let mods = two_mods();
        // Level 5 carries two slots; 4 carries one.
        let (level, kept) = catalog.downgrade(1, 5, &mods);
        assert_eq!(level, 4);
        assert_eq!(kept, vec![mods[0].clone()]);
        // Level 3 -> 2 has no slots left.
        let (level, kept) = catalog.downgrade(1, 3, &mods[..1]);
        assert_eq!((level, kept.len()), (2, 0));
        // At the minimum it stays (the caller refuses the action; the key is never deleted).
        let (level, _) = catalog.downgrade(1, 1, &[]);
        assert_eq!(level, 1);
        let (level, _) = catalog.downgrade(3, 3, &mods);
        assert_eq!(level, 3);
    }

    #[test]
    fn a_snapshot_is_made_safe_before_the_fight_trusts_it() {
        let catalog = catalog();
        let dirty = vec![
            KeyModifier {
                id: "plague_explosion".into(),
                magnitude_pct: 900,
            },
            KeyModifier::base("nonsense"),
            KeyModifier::base("plague_explosion"),
            KeyModifier {
                id: "rotating_beams".into(),
                magnitude_pct: -900,
            },
        ];
        let clean = catalog.sanitize(5, &dirty);
        assert_eq!(
            clean,
            vec![
                KeyModifier {
                    id: "plague_explosion".into(),
                    magnitude_pct: 100
                },
                KeyModifier {
                    id: "rotating_beams".into(),
                    magnitude_pct: -50
                },
            ]
        );
        // Level 3 has one slot; level 1 none.
        assert_eq!(catalog.sanitize(3, &dirty).len(), 1);
        assert!(catalog.sanitize(1, &dirty).is_empty());
    }

    #[test]
    fn modifiers_survive_their_stored_json_and_bad_text_is_a_key_with_none() {
        let mods = vec![KeyModifier {
            id: "rotating_beams".into(),
            magnitude_pct: -12,
        }];
        assert_eq!(decode_modifiers(&encode_modifiers(&mods)), mods);
        assert!(decode_modifiers("").is_empty());
        assert!(decode_modifiers("{not json").is_empty());
        assert_eq!(
            decode_modifiers(r#"[{"id":"rotating_beams"}]"#),
            vec![KeyModifier::base("rotating_beams")]
        );
        assert_eq!(magnitude_text(17), "+17%");
        assert_eq!(magnitude_text(-12), "-12%");
        assert_eq!(magnitude_text(0), "+0%");
    }

    #[test]
    fn the_reveal_record_round_trips() {
        let event = RerollEvent {
            seq: 1,
            by: "Ash".into(),
            modifier: "plague_explosion".into(),
            delta_pct: 17,
            magnitude_pct: 17,
        };
        assert!(event.strengthened());
        assert_eq!(RerollEvent::from_json(&event.to_json()), Some(event));
        assert_eq!(RerollEvent::from_json(""), None);
    }
}
