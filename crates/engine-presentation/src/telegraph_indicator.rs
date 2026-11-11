//! Monster telegraphs, tracked for drawing as ground indicators.
//!
//! A telegraph is gameplay's promise that a patch of ground will be hit in `delay` seconds.
//! The VFX director used to draw that promise as an animated effect, which had two problems:
//! the effect rebuilt its footprint every frame (it twitched while the caster moved) and it
//! showed no interior, so the player could not read *when* the strike would land.
//!
//! This module keeps only the facts an indicator needs - the shape, where it is anchored,
//! which way it faces, and how far through its delay it is - and nothing about how it is
//! drawn. The app draws a static border with a fill that covers [`progress`] of the shape:
//! nothing at the start of the cast, half when half the time is left, all of it when the
//! strike lands.
//!
//! [`progress`]: TelegraphIndicator::progress
//!
//! # Time
//!
//! Elapsed time is advanced by real frame time, as the VFX layer does, and not read from the
//! simulation. The start and end of the telegraph still come from gameplay's events, so the
//! indicator is removed on the tick the attack resolves; a fill that is a frame ahead or
//! behind the strike is invisible, one that outlives it is not.

use engine_core::math::Vec3;
use engine_game::enemy::TelegraphShape;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::Simulation;

use crate::beam::BeamTracker;
use crate::extract::lerp_position;
use crate::ui::targeting::TargetShape;

/// The most indicators tracked at once. Past it the oldest is dropped.
pub const MAX_INDICATORS: usize = 64;

/// Added to an entity's slot to name the indicator of the beam it is channelling, so a beam
/// can never be mistaken for one of gameplay's own telegraph ids, which count up from zero.
pub const BEAM_INDICATOR_BASE: u32 = 0x8000_0000;

/// How long past its own delay an indicator is kept waiting for its end event, in seconds.
///
/// Gameplay always ends a telegraph, so this only matters if an event is lost: without it
/// the indicator would stay on the floor for good.
const OVERDUE_GRACE: f32 = 2.0;

/// Where the ids of a dungeon modifier's indicators start. Gameplay's telegraph ids count up
/// from zero, so the two cannot meet.
const MODIFIER_KEY_BASE: u32 = 0xA000_0000;
/// Ids of festering corpses.
const FUSE_KEY_BASE: u32 = 0xA000_0000;
/// Ids of plague clouds.
const CLOUD_KEY_BASE: u32 = 0xB000_0000;
/// Ids of beams: the emitter's id and the beam's index.
const BEAM_KEY_BASE: u32 = 0xC000_0000;
/// The part of a modifier id that distinguishes one fuse or cloud from another.
const KEY_MASK: u32 = 0x0FFF_FFFF;
/// The part of an emitter id that fits above a beam's index.
const BEAM_KEY_MASK: u32 = 0x00FF_FFFF;

/// The most beam emitters drawn at once: three beams each.
pub const MAX_BEAM_EMITTERS_DRAWN: usize = 3;
/// The most festering corpses drawn at once. A stampede of kills is more than can be read.
pub const MAX_FUSES_DRAWN: usize = 12;
/// The most plague clouds drawn at once.
pub const MAX_CLOUDS_DRAWN: usize = 6;

/// What kind of telegraph this is, which decides how it is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorKind {
    /// An area on the ground that a monster is about to hit.
    Enemy,
    /// A mark riding a player: stepping out of it is not the answer.
    Mark,
    /// A dungeon modifier's beam that is only a warning yet: an outline, no damage.
    BeamWarning,
    /// A dungeon modifier's beam that burns: filled in the danger colour.
    Beam,
    /// A festering corpse about to burst, filling towards the burst.
    Plague,
    /// Plague on the ground after a burst: a filled, green-tinted disc.
    PlagueCloud,
    /// A Moving Fire Beam's footprint, following its channeller and turning with the beam.
    ///
    /// Not a warning - the player's own colour, never mistaken for a monster's area.
    FireBeam,
}

/// One monster telegraph being shown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TelegraphIndicator {
    /// Gameplay's id for the telegraph, matching its end event.
    pub id: u32,
    /// Enemy area or follower mark.
    pub kind: IndicatorKind,
    /// The footprint, in the shape type the targeting indicator already draws.
    pub shape: TargetShape,
    /// Where the shape is anchored: a circle's centre, a cone's apex, a line's start.
    pub anchor: Vec3,
    /// Yaw about +Y, zero facing -Z.
    pub facing: f32,
    /// The entity the anchor follows, for a mark.
    pub follow: Option<EntityId>,
    /// Seconds from the start of the telegraph to the strike.
    pub delay: f32,
    /// Seconds since the telegraph started.
    pub elapsed: f32,
    /// A fill that is not counted by the clock: `0` for an outline only, `1` for filled. A
    /// dungeon modifier's beams use it, since their state is a fact and not a countdown.
    pub held: Option<f32>,
    /// Whether the shape changes every frame (a turning beam), so it is rebuilt each time
    /// rather than once.
    pub moving: bool,
}

impl TelegraphIndicator {
    /// How far through its delay this is, 0 at the start and 1 when the strike lands.
    ///
    /// A telegraph with no delay is already at the strike.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if let Some(held) = self.held {
            return held.clamp(0.0, 1.0);
        }
        if self.delay <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.delay).clamp(0.0, 1.0)
    }
}

/// The footprint and facing of a telegraph, or `None` for one that covers no ground.
#[must_use]
pub fn shape_of(shape: TelegraphShape) -> Option<(TargetShape, f32)> {
    match shape {
        TelegraphShape::Single => None,
        TelegraphShape::Circle { radius } => Some((TargetShape::Circle { radius }, 0.0)),
        TelegraphShape::Cone {
            yaw,
            range,
            half_angle,
        } => Some((TargetShape::Cone { range, half_angle }, yaw)),
        TelegraphShape::Line { yaw, length, width } => {
            Some((TargetShape::Line { width, length }, yaw))
        }
    }
}

/// Every monster telegraph that is currently on the floor.
#[derive(Debug, Clone)]
pub struct TelegraphIndicators {
    active: Vec<TelegraphIndicator>,
}

impl Default for TelegraphIndicators {
    fn default() -> Self {
        Self::new()
    }
}

impl TelegraphIndicators {
    /// No indicators, with room for [`MAX_INDICATORS`] so nothing allocates later.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: Vec::with_capacity(MAX_INDICATORS),
        }
    }

    /// Reads this frame's telegraph events: a start adds an indicator, an end removes it.
    pub fn observe(&mut self, simulation: &Simulation) {
        for event in simulation.events() {
            self.handle(event);
        }
    }

    /// Applies one event.
    pub fn handle(&mut self, event: &CombatEvent) {
        match *event {
            CombatEvent::TelegraphStarted {
                id,
                anchor,
                shape,
                delay,
                follow,
                ..
            } => {
                let Some((shape, facing)) = shape_of(shape) else {
                    return;
                };
                // A repeated id replaces rather than duplicates.
                self.active.retain(|indicator| indicator.id != id);
                if self.active.len() >= MAX_INDICATORS {
                    self.active.remove(0);
                }
                self.active.push(TelegraphIndicator {
                    id,
                    kind: if follow.is_some() {
                        IndicatorKind::Mark
                    } else {
                        IndicatorKind::Enemy
                    },
                    shape,
                    anchor,
                    facing,
                    follow,
                    delay,
                    elapsed: 0.0,
                    held: None,
                    moving: false,
                });
            }
            CombatEvent::TelegraphEnded { id, .. } => {
                self.active.retain(|indicator| indicator.id != id);
            }
            _ => {}
        }
    }

    /// Moves every indicator `delta` seconds on, and follows a mark to its victim.
    ///
    /// A mark's anchor is the victim's interpolated position, the same one the character is
    /// drawn at, so the ring sits under the feet rather than a tick behind them. A mark whose
    /// victim is gone or dead is dropped. Anything long past its strike with no end event is
    /// dropped too.
    pub fn advance(&mut self, delta: f32, simulation: &Simulation) {
        let delta = delta.max(0.0);
        let world = simulation.world();
        let alpha = simulation.timestep().alpha();
        self.active.retain_mut(|indicator| {
            indicator.elapsed += delta;
            if indicator.elapsed > indicator.delay + OVERDUE_GRACE {
                return false;
            }
            let Some(victim) = indicator.follow else {
                return true;
            };
            if !world.is_alive(victim) {
                return false;
            }
            let Some(body) = world.body(victim) else {
                return false;
            };
            let previous = world.previous_body(victim).unwrap_or(body);
            indicator.anchor = lerp_position(previous.position, body.position, alpha);
            true
        });
    }

    /// Lays the floor under every beam in `beams`: a rectangle as long and as wide as the
    /// beam hits, anchored at its channeller and turned to the beam's yaw.
    ///
    /// Rebuilt from the tracker every frame rather than kept: a beam moves every frame, and
    /// the tracker is already the one place that knows where each one points. The ids are
    /// stable per channeller, so the app's slot for a beam is the same slot next frame.
    pub fn sync_beams(&mut self, beams: &BeamTracker, simulation: &Simulation) {
        self.active
            .retain(|indicator| indicator.kind != IndicatorKind::FireBeam);
        for beam in beams.iter() {
            if self.active.len() >= MAX_INDICATORS {
                break;
            }
            let Some(anchor) = beams.origin_of(beam, simulation) else {
                continue;
            };
            self.active.push(TelegraphIndicator {
                id: BEAM_INDICATOR_BASE | beam.entity.index() as u32,
                kind: IndicatorKind::FireBeam,
                shape: TargetShape::Line {
                    width: engine_game::firemage::BEAM_WIDTH,
                    length: engine_game::firemage::BEAM_LENGTH,
                },
                anchor,
                facing: beam.yaw,
                // Followed, so the app rebuilds the footprint every frame as it does a mark.
                follow: Some(beam.entity),
                delay: 0.0,
                elapsed: 0.0,
                // Always fully drawn, and turned every frame.
                held: Some(1.0),
                moving: true,
            });
        }
    }

    /// The indicators, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &TelegraphIndicator> {
        self.active.iter()
    }

    /// How many indicators are showing.
    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Whether nothing is showing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// Replaces the dungeon modifiers' indicators with what the simulation holds right now.
    ///
    /// Not event-driven: a modifier's ground is a pure function of the state it keeps and the
    /// clock (see `engine_game::modifier`), so the indicators are derived each frame from
    /// that state, which is the same on a client that only mirrors the server's events and on
    /// an offline simulation that runs the modifiers itself. The turning beams are therefore
    /// exact without any per-frame message, and a lost end event cannot leave one on the floor.
    ///
    /// Bounded: the newest [`MAX_BEAM_EMITTERS_DRAWN`] emitters, [`MAX_FUSES_DRAWN`] corpses and
    /// [`MAX_CLOUDS_DRAWN`] clouds. Nothing is allocated once the list has grown to its
    /// largest.
    pub fn sync_modifiers(&mut self, simulation: &Simulation) {
        self.active
            .retain(|indicator| indicator.id < MODIFIER_KEY_BASE);
        let runtime = simulation.modifiers();
        if runtime.beams().is_empty() && runtime.fuses().is_empty() && runtime.clouds().is_empty() {
            return;
        }
        // Where the simulation's bodies are drawn: between the last tick and the one before.
        let step = f64::from(simulation.timestep().step_seconds());
        let now = simulation.time() - (1.0 - f64::from(simulation.timestep().alpha())) * step;

        let skip = runtime
            .beams()
            .len()
            .saturating_sub(MAX_BEAM_EMITTERS_DRAWN);
        for emitter in runtime.beams().iter().skip(skip) {
            let warming = emitter.is_warming(now);
            for index in 0..emitter.beams {
                self.push_modifier(TelegraphIndicator {
                    id: BEAM_KEY_BASE
                        | ((emitter.id & BEAM_KEY_MASK) << 4)
                        | u32::from(index & 0x0f),
                    kind: if warming {
                        IndicatorKind::BeamWarning
                    } else {
                        IndicatorKind::Beam
                    },
                    shape: TargetShape::Line {
                        width: emitter.width,
                        length: emitter.radius,
                    },
                    anchor: emitter.center,
                    facing: emitter.yaw(index, now),
                    follow: None,
                    delay: 0.0,
                    elapsed: 0.0,
                    held: Some(if warming { 0.0 } else { 1.0 }),
                    moving: true,
                });
            }
        }

        let skip = runtime.fuses().len().saturating_sub(MAX_FUSES_DRAWN);
        for fuse in runtime.fuses().iter().skip(skip) {
            self.push_modifier(TelegraphIndicator {
                id: FUSE_KEY_BASE | (fuse.id & KEY_MASK),
                kind: IndicatorKind::Plague,
                shape: TargetShape::Circle {
                    radius: fuse.radius,
                },
                anchor: fuse.position,
                facing: 0.0,
                follow: None,
                delay: 1.0,
                elapsed: fuse.progress(now),
                held: None,
                moving: false,
            });
        }

        let skip = runtime.clouds().len().saturating_sub(MAX_CLOUDS_DRAWN);
        for cloud in runtime.clouds().iter().skip(skip) {
            self.push_modifier(TelegraphIndicator {
                id: CLOUD_KEY_BASE | (cloud.id & KEY_MASK),
                kind: IndicatorKind::PlagueCloud,
                shape: TargetShape::Circle {
                    radius: cloud.radius,
                },
                anchor: cloud.position,
                facing: 0.0,
                follow: None,
                delay: 0.0,
                elapsed: 0.0,
                held: Some(1.0),
                moving: false,
            });
        }
    }

    fn push_modifier(&mut self, indicator: TelegraphIndicator) {
        if self.active.len() < MAX_INDICATORS {
            self.active.push(indicator);
        }
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
    use engine_game::ability::AbilityId;

    fn started(
        id: u32,
        shape: TelegraphShape,
        delay: f32,
        follow: Option<EntityId>,
    ) -> CombatEvent {
        CombatEvent::TelegraphStarted {
            id,
            source: EntityId::default(),
            ability: AbilityId::NONE,
            anchor: Vec3::new(1.0, 0.0, 2.0),
            shape,
            delay,
            follow,
            tick: 0,
        }
    }

    fn ended(id: u32) -> CombatEvent {
        CombatEvent::TelegraphEnded {
            id,
            anchor: Vec3::ZERO,
            resolved: true,
            tick: 0,
        }
    }

    const CIRCLE: TelegraphShape = TelegraphShape::Circle { radius: 4.0 };

    #[test]
    fn progress_runs_from_zero_to_one_over_the_delay() {
        let mut indicators = TelegraphIndicators::new();
        indicators.handle(&started(1, CIRCLE, 2.0, None));
        let mut indicator = *indicators.iter().next().expect("one indicator");
        assert_eq!(indicator.progress(), 0.0);
        indicator.elapsed = 1.0;
        assert!((indicator.progress() - 0.5).abs() < 1e-6);
        indicator.elapsed = 2.0;
        assert_eq!(indicator.progress(), 1.0);
        indicator.elapsed = 5.0;
        assert_eq!(indicator.progress(), 1.0);
        indicator.delay = 0.0;
        assert_eq!(indicator.progress(), 1.0);
    }

    #[test]
    fn an_end_event_removes_only_its_own_indicator() {
        let mut indicators = TelegraphIndicators::new();
        indicators.handle(&started(1, CIRCLE, 2.0, None));
        indicators.handle(&started(2, CIRCLE, 2.0, None));
        indicators.handle(&ended(1));
        let ids: Vec<u32> = indicators.iter().map(|indicator| indicator.id).collect();
        assert_eq!(ids, [2]);
        indicators.handle(&ended(9));
        assert_eq!(indicators.len(), 1);
        indicators.clear();
        assert!(indicators.is_empty());
    }

    #[test]
    fn a_single_target_telegraph_is_not_shown() {
        let mut indicators = TelegraphIndicators::new();
        indicators.handle(&started(1, TelegraphShape::Single, 1.0, None));
        assert!(indicators.is_empty());
    }

    #[test]
    fn shapes_convert_with_their_own_facing_and_angle() {
        assert_eq!(
            shape_of(TelegraphShape::Circle { radius: 3.0 }),
            Some((TargetShape::Circle { radius: 3.0 }, 0.0)),
        );
        assert_eq!(
            shape_of(TelegraphShape::Cone {
                yaw: 1.5,
                range: 7.0,
                half_angle: 0.4,
            }),
            Some((
                TargetShape::Cone {
                    range: 7.0,
                    half_angle: 0.4,
                },
                1.5,
            )),
        );
        assert_eq!(
            shape_of(TelegraphShape::Line {
                yaw: -0.5,
                length: 12.0,
                width: 2.0,
            }),
            Some((
                TargetShape::Line {
                    width: 2.0,
                    length: 12.0,
                },
                -0.5,
            )),
        );
        assert_eq!(shape_of(TelegraphShape::Single), None);
    }

    #[test]
    fn a_follow_makes_a_mark() {
        let mut indicators = TelegraphIndicators::new();
        indicators.handle(&started(1, CIRCLE, 2.0, Some(EntityId::default())));
        indicators.handle(&started(2, CIRCLE, 2.0, None));
        let kinds: Vec<IndicatorKind> = indicators.iter().map(|indicator| indicator.kind).collect();
        assert_eq!(kinds, [IndicatorKind::Mark, IndicatorKind::Enemy]);
    }

    /// A client simulation that only mirrors the server's modifier events.
    fn mirror() -> Simulation {
        let mut simulation = Simulation::new(engine_game::sim::SimConfig::default());
        simulation.set_remote_authority(true);
        simulation
    }

    fn beams_started(id: u32) -> CombatEvent {
        CombatEvent::ModifierBeamsStarted {
            id,
            center: Vec3::new(3.0, 0.0, 4.0),
            start_angle: 0.0,
            angular_speed: 1.0,
            radius: 9.0,
            width: 0.9,
            beams: 3,
            warmup_end_time: 2.0,
            start_time: 0.0,
            tick: 0,
        }
    }

    #[test]
    fn beams_are_three_outlines_that_fill_and_turn_when_the_warm_up_ends() {
        let mut simulation = mirror();
        let mut indicators = TelegraphIndicators::new();
        simulation.inject_event(beams_started(1));

        simulation.step_many(30);
        indicators.sync_modifiers(&simulation);
        let warming: Vec<TelegraphIndicator> = indicators.iter().copied().collect();
        assert_eq!(warming.len(), 3);
        assert!(warming.iter().all(|i| i.kind == IndicatorKind::BeamWarning));
        assert!(warming.iter().all(|i| i.progress() == 0.0 && i.moving));
        let gap = warming[1].facing - warming[0].facing;
        assert!(
            (gap - std::f32::consts::TAU / 3.0).abs() < 1e-4,
            "120 degrees apart"
        );

        simulation.step_many(120);
        indicators.sync_modifiers(&simulation);
        let active: Vec<TelegraphIndicator> = indicators.iter().copied().collect();
        assert_eq!(active.len(), 3, "replaced, not accumulated");
        assert!(
            active
                .iter()
                .all(|i| i.kind == IndicatorKind::Beam && i.progress() == 1.0)
        );
        assert!(active[0].facing > warming[0].facing, "the beams turned");
        assert_eq!(
            active[0].shape,
            TargetShape::Line {
                width: 0.9,
                length: 9.0
            }
        );
    }

    #[test]
    fn a_fuse_fills_towards_its_burst_and_a_cloud_is_a_filled_disc() {
        let mut simulation = mirror();
        let mut indicators = TelegraphIndicators::new();
        simulation.inject_event(CombatEvent::PlagueArmed {
            id: 9,
            position: Vec3::new(1.0, 0.0, 1.0),
            radius: 4.0,
            armed_time: 0.0,
            explode_time: 5.0,
            tick: 0,
        });
        simulation.step_many(150);
        indicators.sync_modifiers(&simulation);
        let fuse = *indicators.iter().next().expect("a fuse");
        assert_eq!(fuse.kind, IndicatorKind::Plague);
        assert!((fuse.progress() - 0.5).abs() < 0.05, "{}", fuse.progress());

        simulation.inject_event(CombatEvent::PlagueExploded {
            id: 9,
            position: Vec3::new(1.0, 0.0, 1.0),
            radius: 4.0,
            tick: 1,
        });
        indicators.sync_modifiers(&simulation);
        let cloud = *indicators.iter().next().expect("a cloud");
        assert_eq!(indicators.len(), 1, "the fuse is gone");
        assert_eq!(cloud.kind, IndicatorKind::PlagueCloud);
        assert_eq!(cloud.progress(), 1.0);

        simulation.inject_event(CombatEvent::PlagueCloudEnded { id: 9, tick: 2 });
        indicators.sync_modifiers(&simulation);
        assert!(indicators.is_empty());
    }

    #[test]
    fn modifier_indicators_never_disturb_gameplays_telegraphs_and_are_bounded() {
        let mut simulation = mirror();
        let mut indicators = TelegraphIndicators::new();
        indicators.handle(&started(1, CIRCLE, 2.0, None));
        for id in 0..40 {
            simulation.inject_event(CombatEvent::PlagueArmed {
                id: 100 + id,
                position: Vec3::ZERO,
                radius: 4.0,
                armed_time: 0.0,
                explode_time: 5.0,
                tick: 0,
            });
        }
        for id in 0..6 {
            simulation.inject_event(beams_started(id));
        }
        for _ in 0..3 {
            indicators.sync_modifiers(&simulation);
        }
        let drawn = indicators.len();
        assert_eq!(
            drawn,
            1 + MAX_FUSES_DRAWN + 3 * MAX_BEAM_EMITTERS_DRAWN,
            "the newest few of each, and gameplay's own telegraph kept"
        );
        assert!(indicators.iter().any(|i| i.id == 1));
        let mut ids: Vec<u32> = indicators.iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), drawn, "every indicator has its own id");
    }

    #[test]
    fn the_oldest_is_dropped_past_capacity() {
        let mut indicators = TelegraphIndicators::new();
        for id in 0..(MAX_INDICATORS as u32 + 5) {
            indicators.handle(&started(id, CIRCLE, 2.0, None));
        }
        assert_eq!(indicators.len(), MAX_INDICATORS);
        assert_eq!(indicators.iter().next().map(|i| i.id), Some(5));
    }
}
