//! The developer readout for targeted-ability line of sight.
//!
//! ```text
//! LOS       FIREBOLT -> #3   AT CAST
//! RANGE     OK        REQUIRED YES
//! RESULT    BLOCKED   DIST 12.4 M
//! BLOCKER   DUNGEON WALL
//! ```
//!
//! Developer tooling and nothing else. It is written by the application behind its
//! `debug-overlay` feature, beside the collision debug drawing and switched by the same key,
//! and it never appears on the combat HUD: a player who is refused a cast is told by the
//! refusal message, which is [`engine_game::AbilityError`]'s job.
//!
//! Every number here is read from the check the simulation actually ran
//! ([`Simulation::last_line_of_sight`]), so the readout cannot describe a query gameplay
//! did not make.

use std::fmt::Write as _;

use engine_game::sim::{LineOfSightPhase, Simulation};

/// Appends the line-of-sight readout to `lines`, or nothing if no check has been run.
///
/// Appends rather than rewrites, because it is a section of the developer overlay rather
/// than an overlay of its own. Returns how many lines it added.
pub fn append_lines(lines: &mut Vec<String>, simulation: &Simulation) -> usize {
    let Some(record) = simulation.last_line_of_sight() else {
        return 0;
    };
    let before = lines.len();

    let mut line = String::new();
    let _ = write!(
        line,
        "LOS       {} -> #{}   {}",
        record.ability.definition().name,
        record.target.index(),
        match record.phase {
            LineOfSightPhase::Activation => "AT CAST",
            LineOfSightPhase::CastCompletion => "AT LANDING",
        }
    );
    lines.push(line);

    let mut line = String::new();
    let _ = write!(
        line,
        "RANGE     {}        REQUIRED {}",
        if record.in_range { "OK" } else { "TOO FAR" },
        if record.required { "YES" } else { "NO" }
    );
    lines.push(line);

    let mut line = String::new();
    match record.check {
        Some(check) => {
            let _ = write!(
                line,
                "RESULT    {}   DIST {:.1} M",
                if check.visible { "CLEAR" } else { "BLOCKED" },
                (check.to - check.from).length()
            );
        }
        // Neither endpoint had a position: the target was gone before the ray was cast.
        None => {
            let _ = write!(line, "RESULT    NO TARGET");
        }
    }
    lines.push(line);

    let blocker = record.blocker_label();
    if !blocker.is_empty() {
        let mut line = String::new();
        let _ = write!(line, "BLOCKER   {blocker}");
        lines.push(line);
    }

    lines.len() - before
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::math::Vec3;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::firemage::FireAbility;

    /// The training arena with a wall between the player and the selected dummy.
    fn arena_with_a_wall() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        let player = simulation.player_position();
        let target = simulation
            .player()
            .target
            .and_then(|id| simulation.world().position(id))
            .expect("a dummy is selected");
        let middle = (player + target) * 0.5;
        let world = simulation.collision_world_mut();
        world.add_box(
            middle - Vec3::new(0.5, 0.0, 6.0),
            middle + Vec3::new(0.5, 5.0, 6.0),
            "TEST WALL",
        );
        world.rebuild_broad_phase();
        simulation
    }

    #[test]
    fn nothing_is_written_before_any_ability_has_asked() {
        let simulation = arena::training_arena(&ArenaConfig::default());
        let mut lines = Vec::new();
        assert_eq!(append_lines(&mut lines, &simulation), 0);
        assert!(lines.is_empty());
    }

    #[test]
    fn a_blocked_cast_names_the_ability_the_result_and_the_blocker() {
        let mut simulation = arena_with_a_wall();
        let _ = simulation.cast(FireAbility::Firebolt);

        let mut lines = Vec::new();
        let written = append_lines(&mut lines, &simulation);
        assert_eq!(written, lines.len());
        assert!(lines[0].starts_with("LOS       FIREBOLT ->"));
        assert!(lines[0].ends_with("AT CAST"));
        assert!(lines[1].contains("REQUIRED YES"));
        assert!(lines[2].contains("BLOCKED"), "{}", lines[2]);
        assert!(lines[3].contains("TEST WALL"), "{}", lines[3]);
    }

    #[test]
    fn a_clear_cast_reports_clear_and_names_no_blocker() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        let _ = simulation.cast(FireAbility::Firebolt);

        let mut lines = Vec::new();
        let written = append_lines(&mut lines, &simulation);
        assert_eq!(written, 3, "a clear check has no blocker line");
        assert!(lines[2].contains("CLEAR"), "{}", lines[2]);
    }
}
