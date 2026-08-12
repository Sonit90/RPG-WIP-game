//! What happened in combat, as data.
//!
//! Every change combat makes to the world is announced here:
//!
//! ```text
//! Ability system ──┐
//! Status system  ──┼─> CombatEvent ─┬─> health (already applied by the emitter)
//! Death checks   ──┘                ├─> telemetry / the damage meter
//!                                   ├─> future VFX and floating numbers
//!                                   └─> future network replication
//! ```
//!
//! The architectural requirement of Milestone 10 is that damage is **observable without
//! the damage code knowing who is observing**. Nothing in the ability system mentions the
//! meter; the meter reads a slice of events. Adding a second observer — a combat log, a
//! sound cue, a replication layer — costs one more read of the same slice and no change
//! here.
//!
//! Events are values, not callbacks: a callback would run inside the ability system, and an
//! observer that mutated the world from there would make the order of two abilities in one
//! tick depend on who registered first.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::combat::DamageType;
use crate::entity::EntityId;
use crate::firemage::ZoneShape;
use crate::sim::CastEnd;
use crate::status::{StatusEnd, StatusKind};

/// One instance of damage landing.
///
/// Carries everything an observer could need, because an observer cannot ask the world:
/// by the time it reads this, the target may be dead and its slot reused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    /// Who dealt it.
    pub source: EntityId,
    /// Who took it.
    pub target: EntityId,
    /// How much health was actually lost.
    pub amount: f32,
    /// How much of the hit exceeded the health that was there.
    pub overkill: f32,
    /// Which ability is credited. [`AbilityId::NONE`] for damage from no ability.
    pub ability: AbilityId,
    /// What kind of damage it was.
    pub damage_type: DamageType,
    /// Whether this hit killed the target.
    pub killed: bool,
    /// Whether it critically struck.
    ///
    /// Carried rather than derived: the roll happens inside the executor against the crit
    /// chance of that moment, and an observer that tried to infer it from the amount would
    /// have to re-implement every multiplier that produced the number.
    pub critical: bool,
    /// Where the target was standing when it landed, at its feet.
    ///
    /// On the event rather than looked up afterwards, because an observer reading this may
    /// be reading it after the target died and its slot was reused — and because a hit is a
    /// thing that happened at a place, not a thing that happened to a row in a table.
    pub position: Vec3,
    /// The simulation tick it happened on.
    pub tick: u64,
}

/// Something that happened in combat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CombatEvent {
    /// An ability was successfully used. Emitted once per use, before its damage.
    AbilityUsed {
        /// Who used it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// How many entities it affected.
        targets: u32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// Damage landed.
    Damage(DamageEvent),
    /// Health was restored.
    Healing {
        /// Who healed.
        source: EntityId,
        /// Who was healed.
        target: EntityId,
        /// How much health was actually restored.
        amount: f32,
        /// Which ability is credited.
        ability: AbilityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A status was applied to an entity.
    StatusApplied {
        /// Who applied it.
        source: EntityId,
        /// Who now has it.
        target: EntityId,
        /// Which status.
        status: StatusKind,
        /// How long it will last, seconds.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A status ended.
    StatusRemoved {
        /// Who had it.
        target: EntityId,
        /// Which status.
        status: StatusKind,
        /// Why it ended.
        reason: StatusEnd,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A cast started. Its effect lands when `CastEnded { end: Completed }` follows.
    ///
    /// Announced rather than exposed as state so a cast bar, a future animation selector and
    /// a network client all read the same stream everything else does.
    CastStarted {
        /// Who is casting.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// How long the cast takes, seconds.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A cast ended, whether or not it landed.
    CastEnded {
        /// Who was casting.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Why it ended.
        end: CastEnd,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// An entity was moved somewhere it did not travel to.
    ///
    /// Blink is the one that emits it today. It exists because presentation cannot tell a
    /// teleport from a very fast run by watching positions: both are a large change between
    /// two ticks, and the camera has to lag one and not the other. Carrying both ends means
    /// an observer can measure the jump without having kept last tick's world.
    Teleported {
        /// Who moved.
        entity: EntityId,
        /// Where they were.
        from: Vec3,
        /// Where they are now, after the geometry had its say.
        to: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// An entity's health reached zero.
    Death {
        /// Who died.
        entity: EntityId,
        /// Who killed them, or [`EntityId::INVALID`] if nobody did.
        killer: EntityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A single-target ability reached its victim.
    ///
    /// **The damage has already been applied when this is emitted.** Every targeted ability
    /// in the class is hitscan and stays hitscan — making a bolt a real travelling object
    /// would change what "in range" means, when a target can dodge and what a dead target
    /// does to a cast in flight, and all three are gameplay. What this announces is the
    /// *line the effect travelled along*, so presentation can fly something down it and play
    /// an impact at the far end. A missing bolt costs a picture, never a hit.
    ProjectileLaunched {
        /// Who cast it.
        source: EntityId,
        /// Who it hit.
        target: EntityId,
        /// Where the caster was standing.
        from: Vec3,
        /// Where the target was standing.
        to: Vec3,
        /// Which ability.
        ability: AbilityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// Something is falling and will land at a known place after a known delay.
    ///
    /// Meteor, and the Ultimate when it is enabled. Announced when the cast lands rather
    /// than when the rock does, which is what lets the interface show an anticipation ring
    /// for exactly the delay the gameplay is counting down.
    ImpactScheduled {
        /// Who cast it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Where it will land.
        position: Vec3,
        /// The radius it will cover, in metres.
        radius: f32,
        /// Seconds until it lands.
        delay: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A scheduled impact arrived.
    ImpactLanded {
        /// Who cast it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Where it landed.
        position: Vec3,
        /// The radius it covered, in metres.
        radius: f32,
        /// How many entities were inside that radius.
        targets: u32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A lasting area of effect started burning.
    ///
    /// Carries the *gameplay* shape — the same [`ZoneShape`] the containment test uses — so
    /// a visual built from it cannot claim a different footprint from the one that deals
    /// the damage.
    AreaEffectStarted {
        /// Names this area, so its end can be matched to its start.
        id: u32,
        /// Who created it.
        source: EntityId,
        /// Which ability is credited.
        ability: AbilityId,
        /// Where it is anchored: a circle's centre, a line's near end.
        position: Vec3,
        /// The ground it covers.
        shape: ZoneShape,
        /// Seconds it will last.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A lasting area of effect burned out.
    AreaEffectEnded {
        /// Which area, matching an earlier [`CombatEvent::AreaEffectStarted`].
        id: u32,
        /// Where it was anchored.
        position: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A character left the ground under its own power.
    Jumped {
        /// Who jumped.
        entity: EntityId,
        /// Where they were standing.
        position: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A character touched the ground again.
    Landed {
        /// Who landed.
        entity: EntityId,
        /// Where they landed.
        position: Vec3,
        /// How fast they were falling, in metres per second. Always positive.
        speed: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
}

impl CombatEvent {
    /// The tick this event was emitted on.
    #[must_use]
    pub fn tick(&self) -> u64 {
        match *self {
            Self::AbilityUsed { tick, .. }
            | Self::Healing { tick, .. }
            | Self::StatusApplied { tick, .. }
            | Self::StatusRemoved { tick, .. }
            | Self::CastStarted { tick, .. }
            | Self::CastEnded { tick, .. }
            | Self::Teleported { tick, .. }
            | Self::Death { tick, .. }
            | Self::ProjectileLaunched { tick, .. }
            | Self::ImpactScheduled { tick, .. }
            | Self::ImpactLanded { tick, .. }
            | Self::AreaEffectStarted { tick, .. }
            | Self::AreaEffectEnded { tick, .. }
            | Self::Jumped { tick, .. }
            | Self::Landed { tick, .. } => tick,
            Self::Damage(damage) => damage.tick,
        }
    }

    /// The damage this event carries, if it is a damage event.
    #[must_use]
    pub fn as_damage(&self) -> Option<&DamageEvent> {
        match self {
            Self::Damage(damage) => Some(damage),
            _ => None,
        }
    }
}

/// The events emitted since the stream was last cleared.
///
/// A growable buffer that is cleared, never freed: after the first few frames the capacity
/// covers the busiest tick and emitting an event is a `push` into memory that already
/// exists. That is the whole allocation strategy for combat, and Milestone 25 measures it.
#[derive(Debug, Default, Clone)]
pub struct EventStream {
    events: Vec<CombatEvent>,
    recorded: u64,
}

impl EventStream {
    /// An empty stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty stream with room for `capacity` events before it grows.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            recorded: 0,
        }
    }

    /// Appends an event.
    pub fn push(&mut self, event: CombatEvent) {
        self.events.push(event);
        self.recorded += 1;
    }

    /// The events currently held, in emission order.
    #[must_use]
    pub fn events(&self) -> &[CombatEvent] {
        &self.events
    }

    /// The events emitted after `cursor`, which is a length previously read from
    /// [`EventStream::len`].
    ///
    /// This is how a per-tick observer reads only what that tick produced without the
    /// stream having to be cleared between ticks — the frame's whole event list stays
    /// available to the application afterwards.
    #[must_use]
    pub fn events_after(&self, cursor: usize) -> &[CombatEvent] {
        self.events.get(cursor..).unwrap_or(&[])
    }

    /// How many events are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing has been emitted since the last clear.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// How many events this stream has ever carried.
    ///
    /// Survives [`EventStream::clear`], so a test can assert that a frame emitted anything
    /// at all without holding on to the events.
    #[must_use]
    pub fn total_recorded(&self) -> u64 {
        self.recorded
    }

    /// How many events the buffer can hold before it grows.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.events.capacity()
    }

    /// Drops the held events, keeping the allocation.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Drops the held events and the lifetime counter.
    pub fn reset(&mut self) {
        self.events.clear();
        self.recorded = 0;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn damage(amount: f32, tick: u64) -> CombatEvent {
        CombatEvent::Damage(DamageEvent {
            source: EntityId::new(0, 1),
            target: EntityId::new(1, 1),
            amount,
            overkill: 0.0,
            ability: AbilityId::from_slot(0).expect("slot 0 exists"),
            damage_type: DamageType::Fire,
            killed: false,
            critical: false,
            position: Vec3::ZERO,
            tick,
        })
    }

    #[test]
    fn events_are_held_in_emission_order() {
        let mut stream = EventStream::new();
        stream.push(damage(10.0, 1));
        stream.push(damage(20.0, 1));

        let amounts: Vec<f32> = stream
            .events()
            .iter()
            .filter_map(|event| event.as_damage().map(|damage| damage.amount))
            .collect();
        assert_eq!(amounts, vec![10.0, 20.0]);
        assert_eq!(stream.len(), 2);
        assert_eq!(stream.total_recorded(), 2);
    }

    #[test]
    fn a_cursor_reads_only_what_came_after_it() {
        // How the meter observes one tick's events without the frame's list being cleared.
        let mut stream = EventStream::new();
        stream.push(damage(10.0, 1));
        let cursor = stream.len();
        stream.push(damage(20.0, 2));
        stream.push(damage(30.0, 2));

        let recent = stream.events_after(cursor);
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|event| event.tick() == 2));
        assert!(
            stream.events_after(99).is_empty(),
            "a stale cursor is empty"
        );
    }

    #[test]
    fn clearing_keeps_the_allocation_and_the_lifetime_count() {
        let mut stream = EventStream::with_capacity(64);
        for tick in 0..64 {
            stream.push(damage(1.0, tick));
        }
        let capacity = stream.capacity();
        stream.clear();

        assert!(stream.is_empty());
        assert_eq!(stream.total_recorded(), 64, "the counter survives a clear");
        assert_eq!(stream.capacity(), capacity, "the buffer was not released");

        for tick in 0..64 {
            stream.push(damage(1.0, tick));
        }
        assert_eq!(
            stream.capacity(),
            capacity,
            "a steady state must not reallocate"
        );
    }

    #[test]
    fn resetting_forgets_the_lifetime_count_too() {
        let mut stream = EventStream::new();
        stream.push(damage(1.0, 0));
        stream.reset();
        assert_eq!(stream.total_recorded(), 0);
        assert!(stream.is_empty());
    }

    #[test]
    fn every_event_reports_the_tick_it_happened_on() {
        let entity = EntityId::new(3, 1);
        let ability = AbilityId::from_slot(2).expect("slot 2 exists");
        let events = [
            CombatEvent::AbilityUsed {
                source: entity,
                ability,
                targets: 4,
                tick: 7,
            },
            damage(1.0, 7),
            CombatEvent::Healing {
                source: entity,
                target: entity,
                amount: 5.0,
                ability,
                tick: 7,
            },
            CombatEvent::StatusApplied {
                source: entity,
                target: entity,
                status: StatusKind::Burning,
                duration: 6.0,
                tick: 7,
            },
            CombatEvent::StatusRemoved {
                target: entity,
                status: StatusKind::Burning,
                reason: StatusEnd::Consumed,
                tick: 7,
            },
            CombatEvent::Death {
                entity,
                killer: entity,
                tick: 7,
            },
        ];
        for event in events {
            assert_eq!(event.tick(), 7, "{event:?}");
        }
    }
}
