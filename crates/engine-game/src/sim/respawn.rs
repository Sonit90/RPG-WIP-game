//! The player lifecycle after the health bar reaches zero: dying, coming back, and the
//! twenty seconds during which they cannot come back again.
//!
//! ```text
//!   alive ──► death ──► waiting (3 s) ──► respawn ──► protected (20 s) ──► alive
//!                          ▲                                │
//!                          │      death while protected     │
//!                          └────────────────────────────────┘
//!                          the respawn is *deferred*, not refused: it happens
//!                          the moment the protection runs out
//! ```
//!
//! # What is here and what is not
//!
//! Here: *when* a dead player comes back, and whether they are allowed to yet. Not here:
//! *where* — that is [`EnvironmentScene::player_spawn`](engine_environment::EnvironmentScene)
//! and the level author's business — and not *how the player is restored*, which is
//! `Simulation::revive_player` and has been since long before this module.
//!
//! That split is the whole design. The editor says where; the runtime says when and how; and a
//! level with no spawn point in it still respawns a player, at the place the layout has always
//! started them.
//!
//! # Why the clock is here rather than in the status
//!
//! [`StatusKind::RespawnProtection`] is what the rest of the game and the interface read, and
//! it is the only thing they read — nothing asks this module a question it can ask the status.
//! But a status does not tick on a corpse (see `Simulation::tick_statuses`, which skips the
//! dead), and this clock has to run *while the player is dead* or a player who died during
//! their protection would wait for a timer that had stopped.
//!
//! So the clock is here and the status is its published view: this module writes it every tick
//! and nothing else ever does. One authority, one number, and the thing everybody reads is the
//! ordinary status every other timed effect in the game uses.
//!
//! # Why a death has a pause in it
//!
//! [`RESPAWN_DELAY`] exists so that dying is a thing that happens rather than a frame nobody
//! sees. A respawn on the same tick as the death is indistinguishable from not having died,
//! which makes the feature impossible to test by playing it and impossible to notice by
//! accident.

use crate::status::StatusKind;

/// How long the player lies dead before coming back, in seconds.
///
/// Long enough to see, short enough not to be a punishment. Not a balance decision — nothing
/// in this game costs anything on death — so it is a readability number and it lives here.
pub const RESPAWN_DELAY: f32 = 3.0;

/// How long after a respawn the player cannot be respawned again, in seconds.
///
/// Twenty, exactly, from the specification. It begins when the player *has* respawned, not
/// when they died: the point of the effect is to stop a respawn from being immediately
/// undone, so it can only be measured from the respawn it is protecting.
pub const RESPAWN_PROTECTION: f32 = 20.0;

/// Where the player is in the cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespawnState {
    /// Alive, or at least not being handled: nothing is pending.
    Idle,
    /// Dead, and a respawn is coming.
    Waiting,
    /// Dead, and a respawn is owed but cannot happen until the protection expires.
    ///
    /// The state the twenty seconds exist to produce. It is not a refusal — the player is not
    /// left dead for ever — it is a deferral, and the respawn fires the moment the clock runs
    /// out.
    Deferred,
    /// A respawn has been asked for and the world has not shown it yet.
    ///
    /// The state that stops one death from producing a stream of respawns. The clock cannot
    /// see the player being put back — it is told `alive` once a tick — so between asking for
    /// a respawn and observing a living player there is a window in which the player is still
    /// dead and nothing is wrong. Without this state that window reads as a fresh death, and
    /// a player who somehow stayed dead would be handed a respawn every three seconds for
    /// ever.
    Respawned,
}

/// The player's death-and-return clock.
///
/// One per simulation. Stepped once a tick, after the world has been updated, so it sees the
/// death the tick produced rather than the one before it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Respawn {
    state: RespawnState,
    /// Seconds until the respawn, while [`RespawnState::Waiting`].
    countdown: f32,
    /// Seconds of protection left. Zero when there is none.
    protection: f32,
    /// How many times the player has come back this session, for the log and for a test.
    count: u32,
}

impl Default for Respawn {
    fn default() -> Self {
        Self::new()
    }
}

/// What the simulation should do about this tick of the lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespawnAction {
    /// Nothing to do.
    None,
    /// The player just died and the clock has started.
    Died,
    /// Put the player back now.
    Respawn,
}

impl Respawn {
    /// A player who is alive and unprotected.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: RespawnState::Idle,
            countdown: 0.0,
            protection: 0.0,
            count: 0,
        }
    }

    /// Where in the cycle the player is.
    #[must_use]
    pub fn state(&self) -> RespawnState {
        self.state
    }

    /// Seconds of respawn protection left, or zero.
    #[must_use]
    pub fn protection_remaining(&self) -> f32 {
        self.protection
    }

    /// Whether the player is inside their protection.
    #[must_use]
    pub fn is_protected(&self) -> bool {
        self.protection > 0.0
    }

    /// Whether a respawn is owed and being held back by the protection.
    #[must_use]
    pub fn is_deferred(&self) -> bool {
        self.state == RespawnState::Deferred
    }

    /// How many times the player has respawned.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Advances the clock by one step and says what the simulation should do.
    ///
    /// `alive` is whether the player is alive *now*, after the tick's damage has been applied.
    /// Everything this type knows about death comes through that one argument, which is what
    /// keeps it independent of how the player died — a fall, a fight, or the F7 that exists to
    /// test it are the same event here.
    pub fn step(&mut self, seconds: f32, alive: bool) -> RespawnAction {
        // The protection runs whatever else is happening, including while the player is dead.
        // That is the reason this clock is here rather than on the status: statuses do not
        // tick on a corpse, and a player who died inside their protection would otherwise wait
        // for a timer that had stopped.
        if self.protection > 0.0 {
            self.protection = (self.protection - seconds).max(0.0);
        }

        if alive {
            // A player who is alive again by any route — the respawn this clock asked for, a
            // reset, a revive, a level change — is not owed one. Their protection is
            // untouched, because it is a property of the player and not of this machine.
            if self.state != RespawnState::Idle {
                self.state = RespawnState::Idle;
                self.countdown = 0.0;
            }
            return RespawnAction::None;
        }

        match self.state {
            RespawnState::Idle => {
                // The transition into death. Recorded once, however many ticks the player
                // spends dead, which is what stops a repeated kill from stacking respawns.
                self.state = RespawnState::Waiting;
                self.countdown = RESPAWN_DELAY;
                RespawnAction::Died
            }
            RespawnState::Waiting => {
                self.countdown = (self.countdown - seconds).max(0.0);
                if self.countdown > 0.0 {
                    return RespawnAction::None;
                }
                if self.is_protected() {
                    // Died inside the protection. The respawn is owed and held, not thrown
                    // away: this is the state the twenty seconds exist to produce.
                    self.state = RespawnState::Deferred;
                    return RespawnAction::None;
                }
                self.finish();
                RespawnAction::Respawn
            }
            RespawnState::Deferred => {
                if self.is_protected() {
                    return RespawnAction::None;
                }
                self.finish();
                RespawnAction::Respawn
            }
            // The respawn was asked for on the tick before this one. Still dead, so either
            // the world has not applied it yet or the player has died again — and after one
            // tick those are the same thing as far as this clock can tell. Treated as a fresh
            // death: it goes back to waiting rather than sitting here, because a state that
            // only ever ends when the player is alive is a state a dead player never leaves.
            //
            // It cannot become a stream of respawns, because the death that follows a respawn
            // always falls inside the protection and is deferred by it.
            RespawnState::Respawned => {
                self.state = RespawnState::Waiting;
                self.countdown = RESPAWN_DELAY;
                RespawnAction::Died
            }
        }
    }

    /// Books the respawn that is about to happen and starts the protection.
    ///
    /// The twenty seconds start here — at the respawn, not at the death — because what they
    /// protect is the respawn.
    fn finish(&mut self) {
        self.state = RespawnState::Respawned;
        self.countdown = 0.0;
        self.protection = RESPAWN_PROTECTION;
        self.count = self.count.saturating_add(1);
    }

    /// Forgets everything: no respawn owed, no protection, no history.
    ///
    /// What a level change and an encounter reset call. A protection that survived a reload
    /// would be twenty seconds of a previous level leaking into this one.
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// The status this clock publishes, if there is one to show.
    ///
    /// The interface and any gameplay that cares read the *status*, never this type — see the
    /// module documentation. This is the one place the two are joined.
    #[must_use]
    pub fn status_kind() -> StatusKind {
        StatusKind::RespawnProtection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the simulation: a player who is alive or not, and who is actually put
    /// back when the clock asks.
    ///
    /// The clock is told `alive` once a tick and can see nothing else, so a test that holds
    /// `alive` at `false` for ever is testing a caller that ignores every respawn it is
    /// handed — which no real one does. Driving it through this instead means the states
    /// asserted below are states the simulation can actually be in.
    struct Player {
        alive: bool,
        respawns: u32,
        deaths: u32,
    }

    impl Player {
        fn new() -> Self {
            Self {
                alive: true,
                respawns: 0,
                deaths: 0,
            }
        }

        /// Runs `seconds` of ticks, applying whatever the clock asks for.
        fn run(&mut self, clock: &mut Respawn, seconds: f32) {
            let step = 1.0 / 60.0;
            for _ in 0..(seconds / step).round() as u32 {
                match clock.step(step, self.alive) {
                    RespawnAction::Died => self.deaths += 1,
                    RespawnAction::Respawn => {
                        self.alive = true;
                        self.respawns += 1;
                    }
                    RespawnAction::None => {}
                }
            }
        }

        fn kill(&mut self) {
            self.alive = false;
        }
    }

    #[test]
    fn a_death_becomes_a_respawn_after_the_delay_and_the_protection_follows_it() {
        let mut clock = Respawn::new();
        let mut player = Player::new();
        player.run(&mut clock, 1.0);
        assert_eq!(player.deaths, 0);

        player.kill();
        player.run(&mut clock, RESPAWN_DELAY * 0.5);
        assert_eq!(player.deaths, 1, "the death was not noticed");
        assert_eq!(clock.state(), RespawnState::Waiting);
        assert!(!clock.is_protected(), "protection starts at the respawn");
        assert!(!player.alive, "it came back before the delay was up");

        player.run(&mut clock, RESPAWN_DELAY);
        assert!(player.alive, "the respawn never came");
        assert_eq!(player.respawns, 1);
        assert_eq!(clock.count(), 1);
        // Twenty seconds, from the respawn and not from the death.
        assert!(
            clock.protection_remaining() > RESPAWN_PROTECTION - RESPAWN_DELAY,
            "{}",
            clock.protection_remaining()
        );
    }

    #[test]
    fn a_death_inside_the_protection_does_not_respawn_until_it_expires() {
        let mut clock = Respawn::new();
        let mut player = Player::new();
        player.kill();
        player.run(&mut clock, RESPAWN_DELAY + 0.1);
        assert_eq!(player.respawns, 1);

        // Killed again well inside the twenty seconds.
        player.kill();
        player.run(&mut clock, RESPAWN_DELAY + 1.0);
        assert!(!player.alive, "it respawned inside the protection");
        assert!(clock.is_deferred(), "{:?}", clock.state());
        assert_eq!(player.respawns, 1, "a second respawn happened");

        // And the moment the protection runs out, the owed respawn happens — once.
        player.run(&mut clock, RESPAWN_PROTECTION);
        assert!(player.alive, "the owed respawn never came");
        assert_eq!(player.respawns, 2);
        assert_eq!(clock.count(), 2);
    }

    #[test]
    fn being_killed_over_and_over_produces_one_respawn_per_protection_window() {
        // Repeated F7, held down. Every death after the first falls inside the protection the
        // first respawn started, so the player comes back on a twenty-second cadence rather
        // than once per press — and the state never runs away.
        let mut clock = Respawn::new();
        let mut player = Player::new();
        for _ in 0..(10.0 * 60.0) as u32 {
            player.kill();
            player.run(&mut clock, 1.0 / 60.0);
        }
        assert_eq!(
            player.respawns, 1,
            "ten seconds of being killed produced {} respawns",
            player.respawns
        );
        assert!(clock.is_protected());

        // Let go of the key and everything settles: the owed respawn arrives and the player
        // is alive and unprotected a little later.
        player.run(&mut clock, RESPAWN_PROTECTION + RESPAWN_DELAY + 1.0);
        assert!(player.alive, "the player never recovered");
        assert_eq!(player.respawns, 2);
    }

    #[test]
    fn a_death_on_the_boundary_of_the_protection_resolves_the_same_way_every_time() {
        // The specification asks for this case to be deterministic. It is: whichever side of
        // the boundary the death falls on, the player is alive again within one delay of the
        // protection ending, and exactly one respawn was spent on it.
        for offset in [-0.25_f32, -1.0 / 60.0, 0.0, 1.0 / 60.0, 0.25] {
            let mut clock = Respawn::new();
            let mut player = Player::new();
            player.kill();
            player.run(&mut clock, RESPAWN_DELAY + 0.1);
            assert_eq!(player.respawns, 1);

            // Wait until the protection is about to end, then die right on the edge.
            let wait = (clock.protection_remaining() + offset).max(0.0);
            player.run(&mut clock, wait);
            player.kill();
            player.run(&mut clock, RESPAWN_DELAY + RESPAWN_PROTECTION + 1.0);

            assert!(player.alive, "offset {offset}: the player stayed dead");
            assert_eq!(
                player.respawns, 2,
                "offset {offset}: {} respawns",
                player.respawns
            );
        }
    }

    #[test]
    fn coming_back_by_some_other_route_cancels_the_owed_respawn() {
        // An encounter reset revives the player where they stand. The clock must notice, or it
        // would respawn a living player three seconds later and teleport them away.
        let mut clock = Respawn::new();
        let mut player = Player::new();
        player.kill();
        player.run(&mut clock, 1.0);
        assert_eq!(clock.state(), RespawnState::Waiting);

        // Revived by something else.
        player.alive = true;
        player.run(&mut clock, 0.1);
        assert_eq!(clock.state(), RespawnState::Idle);
        player.run(&mut clock, 10.0);
        assert_eq!(player.respawns, 0, "it respawned a living player");
    }

    #[test]
    fn clearing_it_takes_the_protection_with_it() {
        // A level change. Twenty seconds of a previous level leaking into this one would be a
        // player who cannot respawn in a level they have only just arrived in.
        let mut clock = Respawn::new();
        let mut player = Player::new();
        player.kill();
        player.run(&mut clock, RESPAWN_DELAY + 0.1);
        assert!(clock.is_protected());

        clock.clear();
        assert!(!clock.is_protected());
        assert_eq!(clock.state(), RespawnState::Idle);
        assert_eq!(clock.count(), 0);
    }
}
