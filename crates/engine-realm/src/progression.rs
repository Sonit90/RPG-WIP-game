//! Levels and experience.
//!
//! The persistence API applies a dungeon result with [`gain`]; the client draws the
//! experience bar with [`experience_to_next`]. One curve, compiled into both.

/// The highest level a character can reach.
pub const MAX_LEVEL: u32 = 60;

/// Experience needed to go from `level` to `level + 1`. Zero at the cap.
#[must_use]
pub fn experience_to_next(level: u32) -> u64 {
    if level >= MAX_LEVEL || level == 0 {
        return 0;
    }
    // Quadratic: 400 to reach level 2, a little under 1.5 million for the last level.
    let level = u64::from(level);
    400 * level * level
}

/// A character's level and experience after gaining `gained`.
///
/// Experience carries over level boundaries; at the cap it stops accumulating.
#[must_use]
pub fn gain(level: u32, experience: u64, gained: u64) -> (u32, u64) {
    let mut level = level.clamp(1, MAX_LEVEL);
    let mut experience = experience.saturating_add(gained);
    while level < MAX_LEVEL {
        let need = experience_to_next(level);
        if experience < need {
            break;
        }
        experience -= need;
        level += 1;
    }
    if level >= MAX_LEVEL {
        experience = 0;
    }
    (level, experience)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_up_and_carries_over() {
        assert_eq!(gain(1, 0, 399), (1, 399));
        assert_eq!(gain(1, 0, 400), (2, 0));
        assert_eq!(gain(1, 300, 200), (2, 100));
        // Two levels in one go.
        assert_eq!(gain(1, 0, 400 + 1600 + 5), (3, 5));
    }

    #[test]
    fn stops_at_the_cap() {
        assert_eq!(gain(MAX_LEVEL, 0, 1_000_000), (MAX_LEVEL, 0));
        assert_eq!(gain(MAX_LEVEL - 1, 0, u64::MAX), (MAX_LEVEL, 0));
        assert_eq!(experience_to_next(MAX_LEVEL), 0);
    }

    #[test]
    fn nonsense_levels_are_clamped() {
        assert_eq!(gain(0, 0, 0), (1, 0));
        assert_eq!(gain(500, 0, 0), (MAX_LEVEL, 0));
    }
}
