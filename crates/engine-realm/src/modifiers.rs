//! Dungeon key modifiers: what exists, how a key rolls them, how they scale and how a reroll
//! changes them.
//!
//! ```text
//! assets/catalog/modifiers.json ─► ModifierCatalog ─┬─ roll_key_modifiers(level, seed) ─► Vec<KeyModifier>
//!                                                   ├─ resolve(level, &[KeyModifier]) ──► Vec<ActiveModifier> ─► Simulation::set_modifiers
//!                                                   ├─ reroll_outcome / apply_reroll    ─► new magnitude_pct
//!                                                   ├─ downgrade / cleanse              ─► a weaker key
//!                                                   └─ key_scaling(level)               ─► (health, damage) multipliers
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
//! The progression table is also the source of the key level's monster health and damage
//! multipliers ([`ModifierCatalog::key_scaling`]); the dungeon's own `keys.health_per_level`
//! stays as the fallback for a catalog with no table.

use engine_game::modifier::{
    ActiveModifier, BeamParams, ModifierKind, ModifierParams, PlagueParams,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::catalog::{CatalogError, KeyRules};

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
    limits: MagnitudeLimits,
}

impl ModifierCatalog {
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

    /// The row for key `level`: the last row for a level past the table, the first for one
    /// before it. `None` only for a catalog with no table at all.
    #[must_use]
    pub fn row(&self, level: u32) -> Option<&ProgressionRow> {
        self.progression
            .iter()
            .find(|row| row.level == level)
            .or_else(|| {
                if level < 1 {
                    self.progression.first()
                } else {
                    self.progression.last()
                }
            })
    }

    /// How many modifiers a key of `level` carries.
    #[must_use]
    pub fn slots(&self, level: u32) -> usize {
        self.row(level).map_or(0, |row| row.modifier_slots as usize)
    }

    /// What a key of `level` multiplies monster `(health, damage)` by.
    ///
    /// The progression table when the catalog has one, otherwise the dungeon's own linear
    /// `keys` rule (see [`crate::key::scaling`]). The realm module calls this in place of
    /// `key::scaling` when it builds the fight.
    #[must_use]
    pub fn key_scaling(&self, rules: &KeyRules, level: u32) -> (f32, f32) {
        match self.row(level) {
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

        let mut progression = Vec::with_capacity(file.progression.len());
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
            progression.push(ProgressionRow {
                level: row.level,
                modifier_slots: row.modifier_slots,
                health_mult: row.health_mult,
                damage_mult: row.damage_mult,
                notes: row.notes,
            });
        }
        Ok(Self {
            modifiers,
            progression,
            limits,
        })
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
    modifiers: Vec<ModifierRecord>,
    progression: Vec<RowRecord>,
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
        assert_eq!(catalog.modifiers().len(), 2);
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
        assert!(
            slots.windows(2).all(|pair| pair[0] <= pair[1]),
            "never fewer as keys rise"
        );
        let health: Vec<f32> = (1..=20)
            .map(|level| catalog.row(level).expect("a row").health_mult)
            .collect();
        assert_eq!(health[0], 1.0);
        assert!(health.windows(2).all(|pair| pair[0] <= pair[1]));
        // Past the table the last row holds; before it the first.
        assert_eq!(catalog.row(99), catalog.row(20));
        assert_eq!(catalog.row(0), catalog.row(1));
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
        let row = catalog.row(10).expect("a row");
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
    fn a_roll_respects_when_a_modifier_is_introduced() {
        let mut catalog = catalog();
        catalog.modifiers[1].introduced_at_level = 7;
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
}
