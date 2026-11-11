//! Casting the Moving Fire Beam: the channel, its damage, its steering and its endings.
//!
//! ```text
//! cast -> begin_beam -> every tick: turn, tick due? -> hit list -> strike + Ignite
//!                                                   -> yaw report (<= 10 / s)
//!          ChannelAim ─► steer                      -> ended: completed, cancelled,
//!          stun / silence / death / another cast ───►          broken, reset
//! ```
//!
//! A child module of [`super`] for the reason the other class executors are: everything it
//! touches is the simulation's own state, and keeping it out of `sim/mod.rs` is what stops
//! the loop from being buried in fire.
//!
//! ## Who decides what
//!
//! **The authority.** The server (or an offline game) runs [`Simulation::tick_beam`]: it turns
//! the beam, finds who stands inside it and deals the damage through
//! [`Simulation::strike`] like every other point of fire damage. A client attached to a
//! remote authority never runs that; it only *presents* the channel
//! ([`Simulation::tick_presented_beam`]) — turning the beam towards the player's own aim so
//! it can be drawn where they are steering — and is corrected by the events the server
//! sends. Damage on a remote client is impossible twice over: nothing here runs, and
//! `deal_damage` refuses under a remote authority.
//!
//! **Steering is an intent.** [`PlayerCommand::ChannelAim`] says where the player is
//! pointing; the beam turns towards it at [`BEAM_TURN_RATE`] at most, whatever was asked.
//!
//! [`BEAM_TURN_RATE`]: crate::firemage::BEAM_TURN_RATE
//! [`PlayerCommand::ChannelAim`]: crate::command::PlayerCommand::ChannelAim

use engine_core::math::Vec3;

use crate::event::{CombatEvent, ControlKind};
use crate::firemage::{
    BEAM_DURATION, BEAM_IGNITE_STACKS, BEAM_MOVE_SPEED_MULTIPLIER, BEAM_TICK_COEFFICIENT,
    BeamAdvance, BeamChannel, BeamEnd, FireAbility, yaw_to,
};

use super::Simulation;
use super::firemage_exec::CastAim;

/// How long past its full length a presented channel waits for the authority's ending before
/// it gives up and stops, in seconds. Only matters if an event is lost.
const PRESENTED_GRACE: f32 = 1.0;

impl Simulation {
    /// The channel the player is running, or idle.
    #[must_use]
    pub fn beam(&self) -> &BeamChannel {
        &self.mage.beam
    }

    /// Whether the player is channelling the Moving Fire Beam.
    #[must_use]
    pub fn is_channelling_beam(&self) -> bool {
        self.mage.beam.is_active()
    }

    /// What the player's movement speed is multiplied by while they channel: the beam's
    /// forty per cent, or one when nothing is being channelled.
    #[must_use]
    pub fn channel_move_multiplier(&self) -> f32 {
        if self.mage.beam.is_active() {
            BEAM_MOVE_SPEED_MULTIPLIER
        } else {
            1.0
        }
    }

    /// Starts the channel the Ultimate cast just paid for.
    ///
    /// The initial yaw is the bearing to the aim point, so the beam is born pointing where
    /// the player pointed rather than where the character happened to face.
    pub(super) fn begin_beam(&mut self, aim: CastAim, origin: Vec3) {
        let caster = self.player.entity;
        let point = match aim {
            CastAim::Ground(position) => Some(position),
            CastAim::Target => None,
        };
        let yaw = point
            .and_then(|point| yaw_to(origin, point))
            .unwrap_or_else(|| self.facing_yaw());
        self.mage.beam.start(yaw, point);
        self.face_beam();
        let (tick, start_time) = (self.tick, self.time);
        self.events.push(CombatEvent::BeamChannelStarted {
            entity: caster,
            yaw,
            start_time,
            duration: BEAM_DURATION,
            tick,
        });
    }

    /// Points the steering at a ground point. Does nothing when no channel is running or the
    /// point is not a number.
    pub fn steer_beam(&mut self, aim: Vec3) {
        if aim.x.is_finite() && aim.y.is_finite() && aim.z.is_finite() {
            self.mage.beam.steer(aim);
        }
    }

    /// Ends the channel because of a crowd-control effect on the player.
    ///
    /// The hook for stun and silence: nothing in the game can put either on the player yet,
    /// and the day something does it calls this rather than reaching into the beam. A taunt
    /// does not interrupt a channel. Returns whether a channel was broken.
    pub fn break_channel(&mut self, control: ControlKind) -> bool {
        match control {
            ControlKind::Stun => self.end_beam(BeamEnd::Stunned),
            ControlKind::Silence => self.end_beam(BeamEnd::Silenced),
            ControlKind::Taunt => false,
        }
    }

    /// Ends the channel and announces why. Returns whether there was one to end.
    pub(super) fn end_beam(&mut self, reason: BeamEnd) -> bool {
        if !self.mage.beam.is_active() {
            return false;
        }
        self.mage.beam.stop();
        let (entity, tick) = (self.player.entity, self.tick);
        self.events.push(CombatEvent::BeamChannelEnded {
            entity,
            reason,
            tick,
        });
        true
    }

    /// Turns the character to face where the beam points.
    fn face_beam(&mut self) {
        let yaw = self.mage.beam.yaw();
        let entity = self.player.entity;
        if let Some(body) = self.world.body_mut(entity) {
            body.facing_yaw = yaw;
        }
    }

    /// Advances the channel one tick: turns it, pays the damage that came due, reports the
    /// steering and ends it when its time is up. Run only by the authority.
    pub(super) fn tick_beam(&mut self, step: f32) {
        if !self.mage.beam.is_active() {
            return;
        }
        let caster = self.player.entity;
        if !self.world.is_alive(caster) {
            self.end_beam(BeamEnd::Died);
            return;
        }
        let origin = self.world.position(caster).unwrap_or(Vec3::ZERO);
        self.mage.beam.turn(origin, step);
        self.face_beam();
        let advance = self.mage.beam.advance(step);
        if advance.ticks > 0 {
            self.pay_beam_ticks(origin, advance);
        }
        if let Some((yaw, turn_target_yaw)) = self.mage.beam.take_report(origin) {
            let (time, tick) = (self.time, self.tick);
            self.events.push(CombatEvent::BeamYaw {
                entity: caster,
                yaw,
                turn_target_yaw,
                time,
                tick,
            });
        }
        if advance.finished {
            self.end_beam(BeamEnd::Completed);
        }
    }

    /// Hits everything inside the beam once for each damage tick that came due.
    ///
    /// The hit list is taken once for the batch: a step is a sixtieth of a second and the beam
    /// turns 1.25 degrees in it, so ticks that fall in the same step cannot disagree about who
    /// was inside. Stats are read per tick, so Heat gained between ticks counts.
    fn pay_beam_ticks(&mut self, origin: Vec3, advance: BeamAdvance) {
        let shape = self.mage.beam.shape();
        self.hits.clear();
        self.collect_in_shape(origin, shape);
        // Announced as damage over time: a tick every quarter second on a whole pack is a
        // stream of small numbers, not a stream of hits.
        self.periodic_damage = true;
        for offset in 0..advance.ticks {
            let index = advance.first_tick + offset;
            let base = BEAM_TICK_COEFFICIENT * self.mage.stats().spell_power;
            for target_index in 0..self.hits.len() {
                let target = self.hits[target_index];
                if !self.world.is_alive(target) {
                    continue;
                }
                self.strike(target, base, FireAbility::FireBeam, true, 0.0);
                if BeamChannel::applies_ignite(index) {
                    self.apply_ignite(target, BEAM_IGNITE_STACKS);
                }
            }
        }
        self.periodic_damage = false;
    }

    /// One tick of a channel under a remote authority: turn it towards the player's aim so
    /// it is drawn where they are steering, and keep its clock for the bar. Deals nothing.
    pub(super) fn tick_presented_beam(&mut self, step: f32) {
        if !self.mage.beam.is_active() {
            return;
        }
        let origin = self
            .world
            .position(self.player.entity)
            .unwrap_or(Vec3::ZERO);
        self.mage.beam.turn(origin, step);
        self.face_beam();
        self.mage.beam.advance(step);
        if self.mage.beam.elapsed() > BEAM_DURATION + PRESENTED_GRACE {
            self.mage.beam.stop();
        }
    }

    /// Lays what the authority reported over the presented channel.
    ///
    /// Only the local player's own channel is state here; everybody else's beam is drawn from
    /// the events by presentation, which has no use for a simulation of it. A no-op unless a
    /// remote authority runs the fight, because an offline game's events are its own.
    pub(super) fn present_beam_event(&mut self, event: &CombatEvent) {
        if !self.remote_authority {
            return;
        }
        let player = self.player.entity;
        match *event {
            CombatEvent::BeamChannelStarted { entity, yaw, .. } if entity == player => {
                self.mage.beam.present(yaw, 0.0);
            }
            CombatEvent::BeamYaw { entity, yaw, .. } if entity == player => {
                self.mage.beam.correct(yaw);
            }
            CombatEvent::BeamChannelEnded { entity, .. } if entity == player => {
                self.mage.beam.stop();
            }
            _ => {}
        }
    }
}
