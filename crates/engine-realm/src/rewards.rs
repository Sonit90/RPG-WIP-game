//! What a finished run pays: experience and resources scaled by the key level, and the key's
//! next level.
//!
//! ```text
//! dungeon reward table + key level + outcome ─▶ [`run_rewards`] ─▶ [`RunRewards`]
//! ```
//!
//! Pure data in, pure data out: the module decides *who* is paid (once, per player) and the
//! persistence API applies the result; the numbers come from here.

use crate::catalog::DungeonDefinition;
use crate::key;

/// What one player is paid for one run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunRewards {
    /// Experience granted.
    pub experience: u32,
    /// Resources granted, as `(resource id, amount)`.
    pub resources: Vec<(String, u32)>,
    /// Talent points granted: one per success, one more at a high enough key level.
    pub talent_points: u32,
    /// The key owner's new key level after a success; `None` for everyone else and after a
    /// failure (the key is unchanged).
    pub key_level_after: Option<u32>,
}

impl RunRewards {
    /// These rewards raised by a key modifier's `bonus_pct` per cent: every resource amount
    /// becomes `amount x (100 + bonus) / 100`, rounded to nearest. Experience and the key's
    /// progress are not touched. A bonus of zero changes nothing.
    ///
    /// The simulation applies the same number to what it rolls (`Simulation::set_loot_rules`),
    /// so a bonus reads as more of everything the run pays.
    #[must_use]
    pub fn with_bonus(mut self, bonus_pct: u32) -> Self {
        for (_, amount) in &mut self.resources {
            let scaled = (u64::from(*amount) * u64::from(100 + bonus_pct) + 50) / 100;
            *amount = u32::try_from(scaled).unwrap_or(u32::MAX);
        }
        self
    }
}

/// The rewards for a run of `dungeon` at `key_level`.
///
/// On success, experience and every resource grow linearly with the levels above the first. On
/// failure nothing is paid and the key stays as it was. Only the key's owner learns a new key
/// level.
#[must_use]
pub fn run_rewards(
    dungeon: &DungeonDefinition,
    key_level: u32,
    success: bool,
    is_key_owner: bool,
) -> RunRewards {
    if !success {
        return RunRewards::default();
    }
    let steps = key_level.saturating_sub(1);
    let table = &dungeon.reward_table;
    RunRewards {
        talent_points: crate::talents::points_for_run(success, key_level),
        experience: table
            .experience
            .saturating_add(table.experience_per_level.saturating_mul(steps)),
        resources: table
            .resources
            .iter()
            .map(|reward| {
                (
                    reward.id.clone(),
                    reward
                        .amount
                        .saturating_add(reward.per_level.saturating_mul(steps)),
                )
            })
            .collect(),
        key_level_after: is_key_owner.then(|| key::progress(&dungeon.keys, key_level, true)),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn crypt() -> DungeonDefinition {
        Catalog::builtin()
            .expect("the shipped catalog parses")
            .dungeon("graveyard_crypt")
            .expect("the crypt")
            .clone()
    }

    #[test]
    fn success_scales_with_the_key_level() {
        let dungeon = crypt();
        let first = run_rewards(&dungeon, 1, true, false);
        assert_eq!(first.experience, 200);
        assert_eq!(first.resources, vec![("crypt_dust".to_owned(), 5)]);
        let tenth = run_rewards(&dungeon, 10, true, false);
        assert_eq!(tenth.experience, 200 + 50 * 9);
        assert_eq!(tenth.resources, vec![("crypt_dust".to_owned(), 5 + 2 * 9)]);
    }

    #[test]
    fn a_reward_bonus_raises_resources_only() {
        let dungeon = crypt();
        let plain = run_rewards(&dungeon, 3, true, true);
        let bonus = plain.clone().with_bonus(25);
        assert_eq!(bonus.experience, plain.experience);
        assert_eq!(bonus.key_level_after, plain.key_level_after);
        // 9 crypt dust at key level 3, +25 per cent, rounded to nearest.
        assert_eq!(plain.resources, vec![("crypt_dust".to_owned(), 9)]);
        assert_eq!(bonus.resources, vec![("crypt_dust".to_owned(), 11)]);
        assert_eq!(plain.clone().with_bonus(0), plain);
    }

    #[test]
    fn failure_pays_nothing_and_keeps_the_key() {
        let dungeon = crypt();
        for owner in [true, false] {
            let rewards = run_rewards(&dungeon, 5, false, owner);
            assert_eq!(rewards.experience, 0);
            assert!(rewards.resources.is_empty());
            assert_eq!(rewards.key_level_after, None);
        }
    }

    #[test]
    fn only_the_owner_learns_a_new_key_level() {
        let dungeon = crypt();
        assert_eq!(
            run_rewards(&dungeon, 4, true, true).key_level_after,
            Some(5)
        );
        assert_eq!(run_rewards(&dungeon, 4, true, false).key_level_after, None);
        let max = dungeon.keys.max_level;
        assert_eq!(
            run_rewards(&dungeon, max, true, true).key_level_after,
            Some(max)
        );
    }
}
