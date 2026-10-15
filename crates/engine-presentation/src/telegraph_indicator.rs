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

/// What kind of telegraph this is, which decides how it is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorKind {
    /// An area on the ground that a monster is about to hit.
    Enemy,
    /// A mark riding a player: stepping out of it is not the answer.
    Mark,
    /// A Moving Fire Beam's footprint, following its channeller and turning with the beam.
    ///
    /// Not a warning - the player's own colour, never mistaken for a monster's area.
    Beam,
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
}

impl TelegraphIndicator {
    /// How far through its delay this is, 0 at the start and 1 when the strike lands.
    ///
    /// A telegraph with no delay is already at the strike.
    #[must_use]
    pub fn progress(&self) -> f32 {
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
            .retain(|indicator| indicator.kind != IndicatorKind::Beam);
        for beam in beams.iter() {
            if self.active.len() >= MAX_INDICATORS {
                break;
            }
            let Some(anchor) = beams.origin_of(beam, simulation) else {
                continue;
            };
            self.active.push(TelegraphIndicator {
                id: BEAM_INDICATOR_BASE | beam.entity.index() as u32,
                kind: IndicatorKind::Beam,
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
