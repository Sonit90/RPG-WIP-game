//! Fire left on the ground, and rocks still falling towards it.
//!
//! ```text
//! cast -> PendingImpact (delay) -> impact -> Zone (ticks until it burns out)
//! ```
//!
//! Firestorm, Inferno, Cataclysm's infernal ground and Meteor's burning ground are one type
//! with different numbers. Inferno follows the caster; the rest stay where they were put.
//!
//! Like [`super::ignite`], nothing here deals damage. A zone reports that it ticked and the
//! simulation decides what that means, so every damage instance in the game still goes
//! through one function.

use engine_core::math::Vec3;

use super::ability::FireAbility;

/// The most zones that can burn at once.
///
/// Fixed so the vector never grows during a fight. Four abilities can leave ground fire and
/// none of them can be cast twice before the first expires; eight is headroom, not a guess.
pub const MAX_ZONES: usize = 8;

/// The most impacts that can be in flight at once.
pub const MAX_PENDING_IMPACTS: usize = 8;

/// What a zone is, which decides how it behaves and how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneKind {
    /// Firestorm: a placed patch that ignites on a chance.
    Firestorm,
    /// Inferno: centred on the caster, ignites every tick.
    Inferno,
    /// Cataclysm's second phase: burns, ignites and slows.
    InfernalGround,
    /// What Meteor leaves behind with the Burning Ground talent.
    BurningGround,
}

impl ZoneKind {
    /// Uppercase name, for the UI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Firestorm => "FIRESTORM",
            Self::Inferno => "INFERNO",
            Self::InfernalGround => "INFERNAL GROUND",
            Self::BurningGround => "BURNING GROUND",
        }
    }

    /// Whether the zone follows the caster instead of staying put.
    #[must_use]
    pub fn follows_caster(self) -> bool {
        matches!(self, Self::Inferno)
    }

    /// Whether standing in it slows you down.
    #[must_use]
    pub fn slows(self) -> bool {
        matches!(self, Self::InfernalGround)
    }
}

/// A patch of fire on the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    /// What it is.
    pub kind: ZoneKind,
    /// Which ability made it, so damage is credited to the right row of the meter.
    pub source: FireAbility,
    /// Where it burns.
    pub position: Vec3,
    /// How far its fire reaches, in metres.
    pub radius: f32,
    /// Seconds before it burns out.
    pub remaining: f32,
    /// Seconds until it next deals damage.
    pub until_tick: f32,
    /// Seconds between ticks.
    pub interval: f32,
    /// Damage one tick deals, before modifiers.
    pub damage: f32,
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
    pending: Vec<PendingImpact>,
}

impl ZoneField {
    /// An empty field with room for everything a fight can produce.
    #[must_use]
    pub fn new() -> Self {
        Self {
            zones: Vec::with_capacity(MAX_ZONES),
            pending: Vec::with_capacity(MAX_PENDING_IMPACTS),
        }
    }

    /// The zones currently burning.
    #[must_use]
    pub fn zones(&self) -> &[Zone] {
        &self.zones
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

    /// Adds a zone, replacing the oldest if the field is full.
    ///
    /// Replacing rather than refusing: a dropped Cataclysm because eight patches of fire
    /// were already burning would be a bug the player could not see the cause of.
    pub fn spawn(&mut self, zone: Zone) {
        if self.zones.len() >= MAX_ZONES {
            self.zones.remove(0);
        }
        self.zones.push(zone);
    }

    /// Schedules an impact.
    pub fn schedule(&mut self, impact: PendingImpact) {
        if self.pending.len() >= MAX_PENDING_IMPACTS {
            self.pending.remove(0);
        }
        self.pending.push(impact);
    }

    /// Extends every Inferno zone, up to `maximum` seconds of extension in total.
    ///
    /// Inferno Cascade. The cap is tracked by the caller, which knows how much it has
    /// already granted this cast.
    pub fn extend(&mut self, kind: ZoneKind, seconds: f32) {
        for zone in &mut self.zones {
            if zone.kind == kind {
                zone.remaining += seconds;
            }
        }
    }

    /// Moves every caster-following zone to `position`.
    pub fn follow(&mut self, position: Vec3) {
        for zone in &mut self.zones {
            if zone.kind.follows_caster() {
                zone.position = position;
            }
        }
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
                    radius: zone.radius,
                    damage: zone.damage,
                    ignite_stacks: zone.ignite_stacks,
                    ignite_chance: zone.ignite_chance,
                });
            }
            if zone.remaining <= 0.0 {
                self.zones.remove(index);
            }
        }
    }

    /// Removes everything. For a reset.
    pub fn clear(&mut self) {
        self.zones.clear();
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
    /// Where the fire is.
    pub position: Vec3,
    /// How far it reaches.
    pub radius: f32,
    /// Damage before modifiers.
    pub damage: f32,
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
            radius: 6.0,
            remaining: duration,
            until_tick: interval,
            interval,
            damage: 60.0,
            ignite_stacks: 1,
            ignite_chance: 0.2,
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
        assert_eq!(ticks.len(), 12, "six seconds at two ticks a second");
        assert!(field.zones().is_empty(), "the zone outlived its duration");
    }

    #[test]
    fn a_long_frame_still_fires_every_tick_it_covered() {
        // A hitch must not eat damage the player is owed.
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
            remaining: 1.25,
            total: 1.25,
        });

        let mut impacts = Vec::new();
        let mut ticks = Vec::new();
        field.tick(1.0, &mut impacts, &mut ticks);
        assert!(impacts.is_empty(), "it landed early");
        assert_eq!(field.pending().len(), 1);

        field.tick(0.3, &mut impacts, &mut ticks);
        assert_eq!(impacts.len(), 1);
        assert_eq!(impacts[0].ability, FireAbility::Meteor);
        assert!(field.pending().is_empty());

        impacts.clear();
        field.tick(10.0, &mut impacts, &mut ticks);
        assert!(impacts.is_empty(), "it landed twice");
    }

    #[test]
    fn several_impacts_land_independently() {
        let mut field = ZoneField::new();
        for (index, delay) in [0.5_f32, 1.25, 2.0].into_iter().enumerate() {
            field.schedule(PendingImpact {
                ability: FireAbility::Meteor,
                position: Vec3::new(index as f32, 0.0, 0.0),
                remaining: delay,
                total: delay,
            });
        }
        let mut impacts = Vec::new();
        let mut ticks = Vec::new();

        field.tick(0.6, &mut impacts, &mut ticks);
        assert_eq!(impacts.len(), 1);
        impacts.clear();

        field.tick(1.5, &mut impacts, &mut ticks);
        assert_eq!(impacts.len(), 2);
        assert!(field.pending().is_empty());
    }

    #[test]
    fn inferno_follows_the_caster_and_placed_zones_do_not() {
        let mut field = ZoneField::new();
        field.spawn(zone(ZoneKind::Inferno, 4.0, 1.0));
        field.spawn(zone(ZoneKind::Firestorm, 6.0, 0.5));

        field.follow(Vec3::new(10.0, 0.0, 10.0));
        let inferno = field.zones().iter().find(|z| z.kind == ZoneKind::Inferno);
        let firestorm = field.zones().iter().find(|z| z.kind == ZoneKind::Firestorm);
        assert_eq!(
            inferno.map(|z| z.position),
            Some(Vec3::new(10.0, 0.0, 10.0))
        );
        assert_eq!(firestorm.map(|z| z.position), Some(Vec3::ZERO));
    }

    #[test]
    fn a_zone_can_be_extended_while_it_burns() {
        let mut field = ZoneField::new();
        field.spawn(zone(ZoneKind::Inferno, 4.0, 1.0));
        field.extend(ZoneKind::Inferno, 0.5);
        assert!((field.zones()[0].remaining - 4.5).abs() < 1e-6);

        // Extending a kind that is not burning is harmless.
        field.extend(ZoneKind::Firestorm, 5.0);
        assert_eq!(field.zones().len(), 1);
    }

    #[test]
    fn the_field_never_grows_past_its_capacity() {
        let mut field = ZoneField::new();
        for _ in 0..MAX_ZONES * 3 {
            field.spawn(zone(ZoneKind::BurningGround, 4.0, 1.0));
        }
        assert_eq!(field.zones().len(), MAX_ZONES);

        for _ in 0..MAX_PENDING_IMPACTS * 3 {
            field.schedule(PendingImpact {
                ability: FireAbility::Meteor,
                position: Vec3::ZERO,
                remaining: 1.0,
                total: 1.0,
            });
        }
        assert_eq!(field.pending().len(), MAX_PENDING_IMPACTS);
    }

    #[test]
    fn ticks_carry_what_the_simulation_needs_to_apply_them() {
        let mut field = ZoneField::new();
        field.spawn(zone(ZoneKind::InfernalGround, 8.0, 1.0));

        let mut impacts = Vec::new();
        let mut ticks = Vec::new();
        field.tick(1.0, &mut impacts, &mut ticks);
        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].kind, ZoneKind::InfernalGround);
        assert_eq!(ticks[0].radius, 6.0);
        assert_eq!(ticks[0].damage, 60.0);
        assert!(ticks[0].kind.slows(), "infernal ground should slow");
        assert!(!ZoneKind::Firestorm.slows());
    }

    #[test]
    fn clearing_removes_zones_and_impacts_alike() {
        let mut field = ZoneField::new();
        field.spawn(zone(ZoneKind::Firestorm, 6.0, 0.5));
        field.schedule(PendingImpact {
            ability: FireAbility::Meteor,
            position: Vec3::ZERO,
            remaining: 1.0,
            total: 1.0,
        });
        field.clear();
        assert!(field.zones().is_empty());
        assert!(field.pending().is_empty());
        assert!(!field.has(ZoneKind::Firestorm));
    }
}
