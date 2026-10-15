//! Every Moving Fire Beam being channelled, tracked for drawing.
//!
//! ```text
//! BeamChannelStarted ─┐                      ┌─► the beam's particles   (vfx)
//! BeamYaw ────────────┼─► BeamTracker ───────┼─► the rectangle on the floor (telegraph indicators)
//! BeamChannelEnded ───┘     │                └─► the roar               (audio)
//!                           └─ turns every frame at the rate the authority turns it
//! ```
//!
//! The authority reports a beam when it starts, when its steering target moves (at most ten
//! times a second) and when it ends. In between, every observer **turns the beam itself** at
//! the beam's own turn rate, towards the last target it was told about — which is exactly
//! what the authority is doing, so a picture drawn from three messages a second is smooth at
//! any frame rate and never ahead of the truth by more than the steering it has not heard.
//!
//! The local player is the exception that makes it feel right: their own channel is real
//! state in the simulation (offline it *is* the beam; online it is the prediction the client
//! steers with its cursor), so this tracker reads the yaw from there instead of extrapolating
//! it. Everyone else's is extrapolated from the events.
//!
//! Like everything in this crate it observes and never writes: deleting it would leave a game
//! that plays identically and shows no beam.

use engine_core::math::Vec3;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::{BEAM_DURATION, BEAM_TURN_RATE, turn_towards, wrap_angle};
use engine_game::sim::Simulation;

use crate::extract::lerp_position;

/// The most beams tracked at once: a party of three each with one, with room to spare.
pub const MAX_BEAMS: usize = 8;

/// How long past its full length a beam is kept waiting for its end event, in seconds.
///
/// The authority always ends a channel; this only matters if the event is lost, which would
/// otherwise leave a beam burning on the floor for good.
const OVERDUE_GRACE: f32 = 2.0;

/// How far an extrapolated beam may drift from a report before it is pulled to it, in
/// radians. Three degrees: below it the report is the same beam seen a round trip later.
const SNAP: f32 = 3.0 * (std::f32::consts::PI / 180.0);

/// One beam being drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamView {
    /// The channeller.
    pub entity: EntityId,
    /// Which way the beam points now, radians about +Y. Zero faces -Z.
    pub yaw: f32,
    /// The yaw it is turning towards.
    pub target_yaw: f32,
    /// Seconds since it started, in real time.
    pub age: f32,
    /// Seconds it lasts unless it is broken.
    pub duration: f32,
}

impl BeamView {
    /// How far through the channel it is, `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.age / self.duration).clamp(0.0, 1.0)
    }
}

/// Every beam currently burning.
#[derive(Debug, Clone)]
pub struct BeamTracker {
    active: Vec<BeamView>,
}

impl Default for BeamTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BeamTracker {
    /// No beams, with room for [`MAX_BEAMS`] so nothing allocates later.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: Vec::with_capacity(MAX_BEAMS),
        }
    }

    /// Reads this frame's beam events.
    pub fn observe(&mut self, simulation: &Simulation) {
        for event in simulation.events() {
            self.handle(event);
        }
    }

    /// Applies one event.
    pub fn handle(&mut self, event: &CombatEvent) {
        match *event {
            CombatEvent::BeamChannelStarted {
                entity,
                yaw,
                duration,
                ..
            } => {
                self.active.retain(|beam| beam.entity != entity);
                if self.active.len() >= MAX_BEAMS {
                    self.active.remove(0);
                }
                self.active.push(BeamView {
                    entity,
                    yaw,
                    target_yaw: yaw,
                    age: 0.0,
                    duration,
                });
            }
            CombatEvent::BeamYaw {
                entity,
                yaw,
                turn_target_yaw,
                ..
            } => {
                if let Some(beam) = self.active.iter_mut().find(|beam| beam.entity == entity) {
                    beam.target_yaw = turn_target_yaw;
                    // Trust our own extrapolation unless it has wandered: re-laying the
                    // report every time would make the beam twitch with the network.
                    if wrap_angle(yaw - beam.yaw).abs() > SNAP {
                        beam.yaw = yaw;
                    }
                }
            }
            CombatEvent::BeamChannelEnded { entity, .. } => {
                self.active.retain(|beam| beam.entity != entity);
            }
            _ => {}
        }
    }

    /// Moves every beam `delta` real seconds on: turns it towards its target, and drops the
    /// ones whose channeller has gone or whose ending never came.
    pub fn advance(&mut self, delta: f32, simulation: &Simulation) {
        let delta = delta.max(0.0);
        let world = simulation.world();
        let local = simulation.player().entity;
        self.active.retain_mut(|beam| {
            beam.age += delta;
            if beam.age > beam.duration.max(BEAM_DURATION) + OVERDUE_GRACE
                || !world.is_alive(beam.entity)
            {
                return false;
            }
            if beam.entity == local && simulation.beam().is_active() {
                // The simulation's own channel: offline it is the truth, online it is the
                // prediction the cursor steers.
                beam.yaw = simulation.beam().yaw();
                let origin = world.position(local).unwrap_or(Vec3::ZERO);
                beam.target_yaw = simulation.beam().target_yaw(origin);
            } else {
                beam.yaw = wrap_angle(turn_towards(
                    beam.yaw,
                    beam.target_yaw,
                    BEAM_TURN_RATE * delta,
                ));
            }
            true
        });
    }

    /// Where `beam` starts: its channeller's feet, at the position they are drawn at.
    #[must_use]
    pub fn origin_of(&self, beam: &BeamView, simulation: &Simulation) -> Option<Vec3> {
        let world = simulation.world();
        let body = world.body(beam.entity)?;
        let previous = world.previous_body(beam.entity).unwrap_or(body);
        Some(lerp_position(
            previous.position,
            body.position,
            simulation.timestep().alpha(),
        ))
    }

    /// The beams, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &BeamView> {
        self.active.iter()
    }

    /// The beam `entity` is channelling, if it is.
    #[must_use]
    pub fn of(&self, entity: EntityId) -> Option<&BeamView> {
        self.active.iter().find(|beam| beam.entity == entity)
    }

    /// How many beams are burning.
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether nothing is burning.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Removes everything, for a scene change or a combat reset.
    pub fn clear(&mut self) {
        self.active.clear();
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::firemage::BeamEnd;

    fn arena() -> Simulation {
        arena::training_arena(&ArenaConfig::default())
    }

    fn started(entity: EntityId, yaw: f32) -> CombatEvent {
        CombatEvent::BeamChannelStarted {
            entity,
            yaw,
            start_time: 0.0,
            duration: BEAM_DURATION,
            tick: 0,
        }
    }

    fn steered(entity: EntityId, yaw: f32, target: f32) -> CombatEvent {
        CombatEvent::BeamYaw {
            entity,
            yaw,
            turn_target_yaw: target,
            time: 0.0,
            tick: 0,
        }
    }

    /// Somebody who is not the local player.
    fn stranger(simulation: &mut Simulation) -> EntityId {
        simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(5.0, 0.0, 5.0),
                1_000.0,
            ))
    }

    #[test]
    fn a_start_adds_a_beam_and_an_end_takes_only_its_own_away() {
        let mut simulation = arena();
        let a = stranger(&mut simulation);
        let b = stranger(&mut simulation);
        let mut beams = BeamTracker::new();
        beams.handle(&started(a, 0.5));
        beams.handle(&started(b, -0.5));
        assert_eq!(beams.len(), 2);
        beams.handle(&CombatEvent::BeamChannelEnded {
            entity: a,
            reason: BeamEnd::Completed,
            tick: 0,
        });
        assert_eq!(beams.len(), 1);
        assert!(beams.of(a).is_none());
        assert!(beams.of(b).is_some());
        beams.clear();
        assert!(beams.is_empty());
    }

    #[test]
    fn a_second_start_replaces_rather_than_duplicates() {
        let mut simulation = arena();
        let a = stranger(&mut simulation);
        let mut beams = BeamTracker::new();
        beams.handle(&started(a, 0.0));
        beams.handle(&started(a, 1.0));
        assert_eq!(beams.len(), 1);
        assert_eq!(beams.of(a).expect("one").yaw, 1.0);
    }

    #[test]
    fn an_observer_turns_a_remote_beam_at_the_turn_rate_towards_the_reported_target() {
        let mut simulation = arena();
        let a = stranger(&mut simulation);
        let mut beams = BeamTracker::new();
        beams.handle(&started(a, 0.0));
        beams.handle(&steered(a, 0.0, 3.0));
        for _ in 0..60 {
            beams.advance(1.0 / 60.0, &simulation);
        }
        let yaw = beams.of(a).expect("one").yaw;
        assert!((yaw - BEAM_TURN_RATE).abs() < 0.02, "{yaw}");
        // And it stops at the target rather than overshooting.
        for _ in 0..200 {
            beams.advance(1.0 / 60.0, &simulation);
        }
        assert!((beams.of(a).expect("one").yaw - 3.0).abs() < 1e-4);
    }

    #[test]
    fn a_report_close_to_the_extrapolation_is_not_laid_over_it() {
        let mut simulation = arena();
        let a = stranger(&mut simulation);
        let mut beams = BeamTracker::new();
        beams.handle(&started(a, 1.0));
        beams.handle(&steered(a, 1.02, 2.0));
        assert_eq!(beams.of(a).expect("one").yaw, 1.0, "twitched for a degree");
        beams.handle(&steered(a, 1.5, 2.0));
        assert_eq!(
            beams.of(a).expect("one").yaw,
            1.5,
            "ignored a real correction"
        );
    }

    #[test]
    fn a_dead_channeller_or_a_lost_ending_does_not_leave_a_beam_burning() {
        let mut simulation = arena();
        let a = stranger(&mut simulation);
        let mut beams = BeamTracker::new();
        beams.handle(&started(a, 0.0));
        beams.advance(BEAM_DURATION + OVERDUE_GRACE + 0.1, &simulation);
        assert!(beams.is_empty(), "outlived its own ending");

        let b = stranger(&mut simulation);
        beams.handle(&started(b, 0.0));
        simulation.slay(b);
        beams.advance(0.016, &simulation);
        assert!(beams.is_empty(), "burning from a corpse");
    }

    #[test]
    fn the_local_players_beam_is_the_simulations_own() {
        use engine_core::math::Vec3;
        use engine_game::command::PlayerCommand;
        use engine_game::firemage::FireAbility;

        let mut simulation = arena();
        let player = simulation.player().entity;
        simulation.mage_mut().gain_energy(100.0);
        simulation
            .cast_at(FireAbility::FireBeam, Vec3::new(0.0, 0.0, -10.0))
            .expect("cast");
        let mut beams = BeamTracker::new();
        beams.observe(&simulation);
        assert!(beams.of(player).is_some(), "the start event was seen");

        simulation.push_command(PlayerCommand::ChannelAim {
            position: Vec3::new(10.0, 0.0, 0.0),
        });
        for _ in 0..30 {
            simulation.step_once();
            beams.advance(1.0 / 60.0, &simulation);
        }
        let drawn = beams.of(player).expect("still burning").yaw;
        assert_eq!(drawn, simulation.beam().yaw());
        assert!(
            drawn.abs() > 0.3,
            "the cursor's steering reached the picture"
        );
    }
}
