//! Eternal keys against a fully geared player: where the ladder runs out (report section 11).
//!
//! ```text
//! ModifierCatalog::row_for(level)  -> health_mult, damage_mult of a key       (monsters)
//! scaling (Default build, st60)    -> DPS and effective health per gear tier  (the player)
//!     wall = the key level at which the monsters have grown by what the tier gained
//!            over the reference (`high` gear on a key 10)
//! ```
//!
//! Three walls per tier, all against the same reference so they are comparable:
//!
//! * **offence**: monster health outgrows the tier's DPS, so a kill takes as long as it took the
//!   reference player on the reference key (`health_mult(L) = health_mult(ref) x DPS / DPS_ref`);
//! * **defence**: monster damage outgrows the tier's effective health
//!   (`damage_mult(L) = damage_mult(ref) x EHP / EHP_ref`);
//! * **combined**: both together, as the geometric mean of the two growths, which is the level at
//!   which the fight is as hard as the reference fight (`sqrt(health x damage)`).
//!
//! `high` on key 10 is the reference because it is the strongest gear the game offered before
//! P2 and key 10 is the middle of the 1-20 ladder, a key such a player clears without strain.
//! The numbers say where gear stops *paying for itself*; skill, a group of several players, and
//! the modifiers' own mechanics move the real wall either way, which is why the table lists the
//! growth by level too.

use std::fmt::Write as _;

use engine_realm::modifiers::ModifierCatalog;

use crate::gear::GearTier;
use crate::scaling::{ClassScaling, Scaling, TierMeasure};

/// The key the reference player is comfortable on.
pub const REFERENCE_KEY: u32 = 10;
/// The gear the reference player wears.
pub const REFERENCE_TIER: GearTier = GearTier::High;
/// The tiers whose wall is reported.
const TIERS: [GearTier; 4] = [
    GearTier::P2Max,
    GearTier::P2Set,
    GearTier::P2Gems,
    GearTier::P2Books,
];
/// The levels the growth table lists.
const LEVELS: [u32; 11] = [10, 12, 15, 20, 25, 30, 40, 50, 60, 80, 99];
/// The highest key level (`dungeons.json` `keys.max_level`).
pub const MAX_KEY: u32 = 99;

/// The monsters' `(health, damage)` multipliers at `level`.
#[must_use]
pub fn growth(catalog: &ModifierCatalog, level: u32) -> (f64, f64) {
    catalog.row_for(level).map_or((1.0, 1.0), |row| {
        (f64::from(row.health_mult), f64::from(row.damage_mult))
    })
}

/// The lowest level in `1..=MAX_KEY` where `reached(health, damage)` holds, if any.
fn first_level(catalog: &ModifierCatalog, reached: impl Fn(f64, f64) -> bool) -> Option<u32> {
    (1..=MAX_KEY).find(|level| {
        let (health, damage) = growth(catalog, *level);
        reached(health, damage)
    })
}

/// A tier's three walls against a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Walls {
    /// Where monster health catches the tier's damage.
    pub offence: Option<u32>,
    /// Where monster damage catches the tier's effective health.
    pub defence: Option<u32>,
    /// Where the fight is as hard as the reference fight.
    pub combined: Option<u32>,
}

/// The walls of `tier` against `reference`.
#[must_use]
pub fn walls(catalog: &ModifierCatalog, tier: &TierMeasure, reference: &TierMeasure) -> Walls {
    let (ref_health, ref_damage) = growth(catalog, REFERENCE_KEY);
    let dps_gain = tier.dps / reference.dps.max(1e-9);
    let ehp_gain = tier.survivability / reference.survivability.max(1e-9);
    let health_target = ref_health * dps_gain;
    let damage_target = ref_damage * ehp_gain;
    Walls {
        offence: first_level(catalog, |health, _| health >= health_target),
        defence: first_level(catalog, |_, damage| damage >= damage_target),
        combined: first_level(catalog, |health, damage| {
            (health * damage).sqrt() >= (health_target * damage_target).sqrt()
        }),
    }
}

fn find(class: &ClassScaling, tier: GearTier) -> Option<&TierMeasure> {
    class.tiers.iter().find(|measure| measure.tier == tier)
}

fn level_text(level: Option<u32>) -> String {
    level.map_or_else(|| format!("> {MAX_KEY}"), |level| level.to_string())
}

/// The class's wall rows, for the report and the summary.
#[must_use]
pub fn class_walls(catalog: &ModifierCatalog, class: &ClassScaling) -> Vec<(GearTier, Walls)> {
    let Some(reference) = find(class, REFERENCE_TIER) else {
        return Vec::new();
    };
    TIERS
        .into_iter()
        .filter_map(|tier| {
            find(class, tier).map(|measure| (tier, walls(catalog, measure, reference)))
        })
        .collect()
}

/// The report section.
pub fn write_section(out: &mut String, scaling: &Scaling) {
    let _ = writeln!(out, "## 11. Eternal keys\n");
    let Some(world) = crate::economy_sim::world() else {
        let _ = writeln!(out, "Not run: the shipped catalogs did not build.\n");
        return;
    };
    let catalog = world.modifiers();
    let _ = writeln!(
        out,
        "How far does a fully geared player get? Monsters grow with the key level by \
         `ModifierCatalog::row_for` (health and damage multipliers; from key {} every odd level \
         adds a modifier slot and every even level raises health by {:.2} and damage by {:.2}). \
         The player grows with gear (section 7). The **wall** of a tier is the key level where \
         the monsters have grown by what that tier gained over `{}` gear on a key {REFERENCE_KEY} \
         (the strongest pre-P2 gear on the middle of the ladder), so a player at the wall meets a \
         fight as demanding as that comfortable one. Three versions: *offence* (monster health \
         against DPS), *defence* (monster damage against effective health) and *combined* \
         (the geometric mean of the two).\n",
        catalog.eternal().map_or(13, |rules| rules.from_level),
        catalog
            .eternal()
            .map_or(0.0, |rules| rules.even_health_step),
        catalog
            .eternal()
            .map_or(0.0, |rules| rules.even_damage_step),
        REFERENCE_TIER.as_str(),
    );
    let _ = writeln!(
        out,
        "| Class | Tier | DPS vs `{}` | Effective health vs `{}` | Offence wall | Defence wall | Combined wall |",
        REFERENCE_TIER.as_str(),
        REFERENCE_TIER.as_str()
    );
    let _ = writeln!(out, "|---|---|---:|---:|---:|---:|---:|");
    for class in &scaling.classes {
        let Some(reference) = find(class, REFERENCE_TIER) else {
            continue;
        };
        for (tier, wall) in class_walls(catalog, class) {
            let Some(measure) = find(class, tier) else {
                continue;
            };
            let _ = writeln!(
                out,
                "| {} | {} | {:.2}x | {:.2}x | {} | {} | {} |",
                class.class.title(),
                tier.as_str(),
                measure.dps / reference.dps.max(1e-9),
                measure.survivability / reference.survivability.max(1e-9),
                level_text(wall.offence),
                level_text(wall.defence),
                level_text(wall.combined),
            );
        }
    }

    let _ = writeln!(out, "\n**Monster growth by key level**\n");
    let _ = writeln!(
        out,
        "| Key level | Health x | Damage x | Modifier slots | sqrt(health x damage) | Per cent of the key {REFERENCE_KEY} fight |"
    );
    let _ = writeln!(out, "|---:|---:|---:|---:|---:|---:|");
    let (ref_health, ref_damage) = growth(catalog, REFERENCE_KEY);
    let reference_difficulty = (ref_health * ref_damage).sqrt();
    for level in LEVELS {
        let (health, damage) = growth(catalog, level);
        let _ = writeln!(
            out,
            "| {} | {:.2} | {:.2} | {} | {:.2} | {:.0} % |",
            level,
            health,
            damage,
            catalog.slots(level),
            (health * damage).sqrt(),
            (health * damage).sqrt() / reference_difficulty * 100.0
        );
    }

    let _ = writeln!(out, "\n**Class spread of the endgame stack**\n");
    let _ = writeln!(
        out,
        "| Class | `high` DPS | `p2_books` DPS | Stack gain | `high` effective health | `p2_books` effective health | Stack gain |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|");
    let mut gains = Vec::new();
    for class in &scaling.classes {
        let (Some(high), Some(books)) =
            (find(class, GearTier::High), find(class, GearTier::P2Books))
        else {
            continue;
        };
        let (dps_gain, ehp_gain) = (
            books.dps / high.dps.max(1e-9),
            books.survivability / high.survivability.max(1e-9),
        );
        gains.push(dps_gain);
        let _ = writeln!(
            out,
            "| {} | {:.0} | {:.0} | {:.3}x | {:.0} | {:.0} | {:.3}x |",
            class.class.title(),
            high.dps,
            books.dps,
            dps_gain,
            high.survivability,
            books.survivability,
            ehp_gain
        );
    }
    if let (Some(low), Some(high)) = (
        gains.iter().copied().reduce(f64::min),
        gains.iter().copied().reduce(f64::max),
    ) {
        let mean = gains.iter().sum::<f64>() / gains.len() as f64;
        let _ = writeln!(
            out,
            "\nThe three classes' DPS gain from `high` to `p2_books` spans {:.3}x to {:.3}x, within {:.1} % of the mean ({:.3}x) (target: within 10 %).\n",
            low,
            high,
            ((high - low) / 2.0 / mean) * 100.0,
            mean
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn measure(tier: GearTier, dps: f64, survivability: f64) -> TierMeasure {
        TierMeasure {
            tier,
            dps,
            survivability,
            duration: 10.0,
        }
    }

    fn catalog() -> &'static ModifierCatalog {
        crate::economy_sim::world()
            .expect("the shipped catalogs build")
            .modifiers()
    }

    #[test]
    fn a_stronger_tier_has_a_later_wall_and_the_reference_sits_at_its_own_key() {
        let reference = measure(GearTier::High, 100.0, 1000.0);
        let same = walls(catalog(), &reference, &reference);
        assert_eq!(same.combined, Some(REFERENCE_KEY));
        let stronger = walls(
            catalog(),
            &measure(GearTier::P2Books, 150.0, 1400.0),
            &reference,
        );
        assert!(stronger.combined > same.combined, "{stronger:?}");
        assert!(stronger.offence > same.offence && stronger.defence > same.defence);
    }

    #[test]
    fn monsters_never_get_weaker_with_the_key() {
        let mut last = (0.0, 0.0);
        for level in 1..=MAX_KEY {
            let now = growth(catalog(), level);
            assert!(now.0 >= last.0 && now.1 >= last.1, "level {level}");
            last = now;
        }
    }
}
