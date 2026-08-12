//! Fire left on the ground, and rocks still falling towards it.
//!
//! ```text
//! cast -> PendingImpact (delay) -> impact -> Zone (ticks until it burns out)
//! ```
//!
//! Firestorm's line, Molten Meteor's burning ground and the Ultimate's infernal ground are
//! one type with different numbers and different shapes.
//!
//! Like [`super::ignite`], nothing here deals damage, and nothing here stores a stat. A zone
//! carries a *coefficient* of spell power and reports that it ticked; the simulation decides
//! what the tick is worth against the stats of the moment. That is what keeps every damage
//! instance in the game going through one function, and it is the same no-snapshot rule the
//! burns follow.

use engine_core::math::Vec3;

use super::ability::FireAbility;

/// The most zones that can burn at once.
pub const MAX_ZONES: usize = 8;

/// The most impacts that can be in flight at once.
pub const MAX_PENDING_IMPACTS: usize = 8;

/// What a zone is, which decides how it behaves and how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    /// Firestorm: a placed line of fire that can ignite what it burns.
    Firestorm,
    /// What Molten Meteor leaves behind: burns, and makes its victims take more fire damage.
    BurningGround,
    /// The Ultimate's second phase. Unused while the Ultimate is disabled.
    InfernalGround,
}

impl ZoneKind {
    /// Uppercase name, for the UI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Firestorm => "FIRESTORM",
            Self::BurningGround => "BURNING GROUND",
            Self::InfernalGround => "INFERNAL GROUND",
        }
    }

    /// Whether standing in it makes fire hurt more.
    #[must_use]
    pub fn applies_vulnerability(self) -> bool {
        matches!(self, Self::BurningGround)
    }

    /// Whether standing in it slows you down.
    #[must_use]
    pub fn slows(self) -> bool {
        matches!(self, Self::InfernalGround)
    }
}

/// The ground a zone covers.
///
/// The shape belongs to the zone rather than to the code that tests it, so a line-shaped
/// Firestorm and a circular burning ground are one system with one containment test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoneShape {
    /// A circle centred on the zone's position.
    Circle {
        /// Metres from the centre.
        radius: f32,
    },
    /// A rectangle starting at the zone's position and running along `yaw`.
    Line {
        /// Which way it points, radians about +Y. Zero faces -Z.
        yaw: f32,
        /// How far it runs, in metres.
        length: f32,
        /// How wide it is, in metres.
        width: f32,
    },
}

impl ZoneShape {
    /// How far the shape reaches from its anchor, for a bounding test and for a mesh.
    #[must_use]
    pub fn extent(self) -> f32 {
        match self {
            Self::Circle { radius } => radius,
            Self::Line { length, .. } => length,
        }
    }

    /// Whether a body of `body_radius` standing at `point` is inside the shape.
    ///
    /// Ground distance only: every area in the class is a footprint on the floor, so a dummy
    /// standing on a step is inside the fire it is standing in.
    #[must_use]
    pub fn contains(self, anchor: Vec3, point: Vec3, body_radius: f32) -> bool {
        let (dx, dz) = (point.x - anchor.x, point.z - anchor.z);
        match self {
            Self::Circle { radius } => (dx * dx + dz * dz).sqrt() <= radius + body_radius,
            Self::Line { yaw, length, width } => {
                let (fx, fz) = (-yaw.sin(), -yaw.cos());
                // Along the line, and across it: the rectangle's own axes.
                let along = dx * fx + dz * fz;
                let across = dx * -fz + dz * fx;
                along >= -body_radius
                    && along <= length + body_radius
                    && across.abs() <= width * 0.5 + body_radius
            }
        }
    }
}

/// A patch of fire on the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    /// What it is.
    pub kind: ZoneKind,
    /// Which ability made it, so damage is credited to the right row of the meter.
    pub source: FireAbility,
    /// Where it is anchored: a circle's centre, or a line's near end.
    pub position: Vec3,
    /// The ground it covers.
    pub shape: ZoneShape,
    /// Seconds before it burns out.
    pub remaining: f32,
    /// Seconds until it next deals damage.
    pub until_tick: f32,
    /// Seconds between ticks.
    pub interval: f32,
    /// Damage one tick deals, as a fraction of spell power.
    pub coefficient: f32,
    /// Ignite stacks a tick applies, if it applies any.
    pub ignite_stacks: u8,
    /// Chance a tick applies its Ignite. One means always.
    pub ignite_chance: f32,
}

impl Zone {
    /// Whether the zone is still burning.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.remaining > 0.0
    }

    /// How much of its life is left, for a UI ring.
    #[must_use]
    pub fn fraction(&self, total: f32) -> f32 {
        if total <= 0.0 {
            return 0.0;
        }
        (self.remaining / total).clamp(0.0, 1.0)
    }
}

/// A cast that has landed but whose effect has not arrived yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingImpact {
    /// Which ability is falling.
    pub ability: FireAbility,
    /// Where it will land.
    pub position: Vec3,
    /// Seconds until it does.
    pub remaining: f32,
    /// The delay it started with, for a UI countdown ring.
    pub total: f32,
}

/// Every zone and every impact still in flight.
#[derive(Debug, Default, Clone)]
pub struct ZoneField {
    zones: Vec<Zone>,
    /// One identity per live zone, parallel to `zones`.
    ///
    /// Parallel rather than a field on [`Zone`] so every existing construction site of a
    /// zone stays a plain struct literal about *fire*, and identity stays a property the
    /// field hands out. It is the field that knows how many zones have ever existed.
    ids: Vec<u32>,
    /// Zones that burned out on the last tick, with the identity they had.
    ///
    /// Held rather than returned through [`ZoneField::tick`]'s parameters, because an
    /// expiry is not a thing the simulation has to act on — only something an observer may
    /// want to hear about. Drained by the simulation each tick; never freed.
    expired: Vec<(u32, Zone)>,
    next_id: u32,
    pending: Vec<PendingImpact>,
}

impl ZoneField {
    /// An empty field with room for everything a fight can produce.
    #[must_use]
    pub fn new() -> Self {
        Self {
            zones: Vec::with_capacity(MAX_ZONES),
            ids: Vec::with_capacity(MAX_ZONES),
            expired: Vec::with_capacity(MAX_ZONES),
            next_id: 1,
            pending: Vec::with_capacity(MAX_PENDING_IMPACTS),
        }
    }

    /// The zones currently burning.
    #[must_use]
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    /// The identity of each burning zone, parallel to [`ZoneField::zones`].
    #[must_use]
    pub fn ids(&self) -> &[u32] {
        &self.ids
    }

    /// The zones that burned out since this was last called, and their identities.
    ///
    /// Empties the list, keeping its allocation.
    pub fn drain_expired(&mut self, out: &mut Vec<(u32, Zone)>) {
        out.append(&mut self.expired);
    }

    /// The impacts still falling.
    #[must_use]
    pub fn pending(&self) -> &[PendingImpact] {
        &self.pending
    }

    /// Whether any zone of this kind is burning.
    #[must_use]
    pub fn has(&self, kind: ZoneKind) -> bool {
        self.zones.iter().any(|zone| zone.kind == kind)
    }

    /// Adds a zone, replacing the oldest if the field is full, and names it.
    ///
    /// The identity is what lets an observer match an area's end to its start; a zone
    /// pushed out by a fuller field ends exactly as one that burned out does, because from
    /// outside there is no difference — the fire is gone either way.
    pub fn spawn(&mut self, zone: Zone) -> u32 {
        if self.zones.len() >= MAX_ZONES {
            let evicted = self.zones.remove(0);
            let id = self.ids.remove(0);
            self.expired.push((id, evicted));
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.zones.push(zone);
        self.ids.push(id);
        id
    }

    /// Schedules an impact.
    pub fn schedule(&mut self, impact: PendingImpact) {
        if self.pending.len() >= MAX_PENDING_IMPACTS {
            self.pending.remove(0);
        }
        self.pending.push(impact);
    }

    /// Advances everything, reporting what fired.
    ///
    /// Impacts land first: a Meteor whose delay ends this tick should burn the ground it
    /// just hit, not wait a frame for it.
    pub fn tick(
        &mut self,
        seconds: f32,
        impacts: &mut Vec<PendingImpact>,
        ticks: &mut Vec<ZoneTick>,
    ) {
        for index in (0..self.pending.len()).rev() {
            self.pending[index].remaining -= seconds;
            if self.pending[index].remaining <= 0.0 {
                impacts.push(self.pending.remove(index));
            }
        }

        for index in (0..self.zones.len()).rev() {
            let zone = &mut self.zones[index];
            zone.remaining -= seconds;
            zone.until_tick -= seconds;
            while zone.until_tick <= 0.0 {
                zone.until_tick += zone.interval.max(0.05);
                ticks.push(ZoneTick {
                    kind: zone.kind,
                    source: zone.source,
                    position: zone.position,
                    shape: zone.shape,
                    coefficient: zone.coefficient,
                    ignite_stacks: zone.ignite_stacks,
                    ignite_chance: zone.ignite_chance,
                });
            }
            if zone.remaining <= 0.0 {
                let burnt = self.zones.remove(index);
                let id = self.ids.remove(index);
                self.expired.push((id, burnt));
            }
        }
    }

    /// Removes everything. For a reset.
    ///
    /// The zones that were burning are reported as expired: a reset takes the fire off the
    /// ground, and an observer holding a picture of it has to be told to take it down too.
    pub fn clear(&mut self) {
        for (index, zone) in self.zones.drain(..).enumerate() {
            let id = self.ids.get(index).copied().unwrap_or(0);
            self.expired.push((id, zone));
        }
        self.ids.clear();
        self.pending.clear();
    }
}

/// One zone dealing its damage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneTick {
    /// What burned.
    pub kind: ZoneKind,
    /// Which ability to credit.
    pub source: FireAbility,
    /// Where the fire is anchored.
    pub position: Vec3,
    /// The ground it covers.
    pub shape: ZoneShape,
    /// Damage before modifiers, as a fraction of spell power.
    pub coefficient: f32,
    /// Ignite stacks it may apply.
    pub ignite_stacks: u8,
    /// Chance it applies them.
    pub ignite_chance: f32,
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn zone(kind: ZoneKind, duration: f32, interval: f32) -> Zone {
        Zone {
            kind,
            source: FireAbility::Firestorm,
            position: Vec3::ZERO,
            shape: ZoneShape::Circle { radius: 6.0 },
            remaining: duration,
            until_tick: interval,
            interval,
            coefficient: 0.70,
            ignite_stacks: 1,
            ignite_chance: 0.15,
        }
    }

    #[test]
    fn a_zone_ticks_on_its_interval_until_it_burns_out() {
        let mut field = ZoneField::new();
        field.spawn(zone(ZoneKind::Firestorm, 6.0, 0.5));

        let mut impacts = Vec::new();
        let mut ticks = Vec::new();
        for _ in 0..12 {
            field.tick(0.5, &mut impacts, &mut ticks);
        }
        assert_eq!(ticks.len(), 12, "the design's twelve ticks");
        assert!(field.zones().is_empty(), "the zone outlived its duration");
    }

    #[test]
    fn a_long_frame_still_fires_every_tick_it_covered() {
        let mut field = ZoneField::new();
        field.spawn(zone(ZoneKind::Firestorm, 6.0, 0.5));

        let mut impacts = Vec::new();
        let mut ticks = Vec::new();
        field.tick(2.0, &mut impacts, &mut ticks);
        assert_eq!(ticks.len(), 4);
    }

    #[test]
    fn an_impact_lands_after_its_delay_and_only_once() {
        let mut field = ZoneField::new();
        field.schedule(PendingImpact {
            ability: FireAbility::Meteor,
            position: Vec3::new(1.0, 0.0, 2.0),
            remaining: 1.0,
            total: 1.0,
        });

        let mut impacts = Vec::new();
        let mut ticks = Vec::new();
        field.tick(0.5, &mut impacts, &mut ticks);
        assert!(impacts.is_empty(), "it landed early");

        field.tick(0.6, &mut impacts, &mut ticks);
        assert_eq!(impacts.len(), 1);
        assert!(field.pending().is_empty());

        impacts.clear();
        field.tick(10.0, &mut impacts, &mut ticks);
        assert!(impacts.is_empty(), "it landed twice");
    }

    #[test]
    fn a_circle_holds_what_stands_in_it() {
        let shape = ZoneShape::Circle { radius: 5.0 };
        assert!(shape.contains(Vec3::ZERO, Vec3::new(3.0, 0.0, 3.0), 0.0));
        assert!(!shape.contains(Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0), 0.0));
        assert!(
            shape.contains(Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0), 1.5),
            "a body wider than the gap should still burn"
        );
        assert!(
            shape.contains(Vec3::ZERO, Vec3::new(0.0, 40.0, 0.0), 0.0),
            "height is not part of a footprint"
        );
    }

    #[test]
    fn a_line_burns_a_rectangle_ahead_of_where_it_was_aimed() {
        // Zero yaw faces -Z, which is the convention the executor and the meshes share.
        let shape = ZoneShape::Line {
            yaw: 0.0,
            length: 20.0,
            width: 6.0,
        };
        assert!(
            shape.contains(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0), 0.0),
            "ahead"
        );
        assert!(
            shape.contains(Vec3::ZERO, Vec3::new(2.9, 0.0, -19.0), 0.0),
            "corner"
        );
        assert!(
            !shape.contains(Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0), 0.0),
            "behind"
        );
        assert!(
            !shape.contains(Vec3::ZERO, Vec3::new(0.0, 0.0, -21.0), 0.0),
            "past"
        );
        assert!(
            !shape.contains(Vec3::ZERO, Vec3::new(4.0, 0.0, -10.0), 0.0),
            "beside"
        );
    }

    #[test]
    fn a_zone_carries_a_coefficient_rather_than_a_damage_number() {
        // The no-snapshot rule for ground fire: what a tick is worth is decided when it
        // fires, from the stats of that moment.
        let burning = zone(ZoneKind::BurningGround, 15.0, 3.0);
        assert!(burning.coefficient > 0.0);
        let mut field = ZoneField::new();
        field.spawn(burning);

        let mut impacts = Vec::new();
        let mut ticks = Vec::new();
        field.tick(3.0, &mut impacts, &mut ticks);
        assert_eq!(ticks.len(), 1);
        assert!((ticks[0].coefficient - 0.70).abs() < 1e-6);
    }

    #[test]
    fn the_field_never_grows_past_its_bound() {
        let mut field = ZoneField::new();
        for _ in 0..MAX_ZONES + 4 {
            field.spawn(zone(ZoneKind::Firestorm, 6.0, 0.5));
        }
        assert_eq!(field.zones().len(), MAX_ZONES);
    }
}
