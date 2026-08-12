//! What an enemy *is*: archetypes and the attacks they own, as data.
//!
//! ```text
//! EnemyKind ──▶ EnemyArchetype ──▶ health, size, speed, aggro, leash, abilities
//!                                            │
//!                                            ▼
//!                                    EnemyAbility ──▶ EnemyAbilityDef
//!                                                     range, cast, cooldown, damage,
//!                                                     shape, interruptibility
//! ```
//!
//! # Two static tables and no code
//!
//! An enemy type is a row in [`ARCHETYPES`]; an attack is a row in [`ENEMY_ABILITIES`]. The
//! state machine in [`brain`](crate::enemy::brain) reads both and contains no `match` on
//! which enemy it is driving — which is the whole answer to "how do we add a new enemy type
//! without modifying the AI". The same arrangement the effect and sound catalogues use, for
//! the same reason.
//!
//! # A shape is gameplay's, and the telegraph draws it
//!
//! [`TelegraphShape`] is the *hit test*. The executor collects its victims with it and the
//! telegraph event carries the same value, so what is drawn on the floor and what takes
//! damage cannot disagree — there is one number, not two.
//!
//! # Nothing here can present anything
//!
//! No mesh, no particle, no sound, no widget: this crate depends on `engine-core` alone. An
//! enemy announces what it did and presentation decides what that looks like.

pub mod brain;
pub mod threat;

use engine_core::math::Vec3;

use crate::combat::DamageType;
use crate::world::horizontal_distance;

/// Half-angle of an enemy cone attack, in radians. Sixty degrees of spread.
///
/// The same width the player's own cone uses (`targeting::CONE_HALF_ANGLE`), deliberately:
/// a player who has learned to read one cone has learned to read the other.
pub const CONE_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_6;

/// The area an attack covers, in world space.
///
/// This is the *gameplay* shape. [`TelegraphShape::contains`] is the hit test the executor
/// runs, and the same value travels on the telegraph event for presentation to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TelegraphShape {
    /// One target, at whatever range the ability reaches. No area to draw.
    Single,
    /// A disc on the ground, centred on the aim point.
    Circle {
        /// Metres from the centre.
        radius: f32,
    },
    /// A wedge from the caster, opening along `yaw`.
    Cone {
        /// Which way it points, radians about +Y. Zero faces −Z.
        yaw: f32,
        /// How far it reaches, in metres.
        range: f32,
        /// Half the spread, in radians.
        half_angle: f32,
    },
}

impl TelegraphShape {
    /// Whether `position` is inside this shape, given where it is anchored.
    ///
    /// `radius` is the target's own horizontal radius, so a wide body at the rim of a blast
    /// is caught by it — the same edge correction every other range test in this crate uses.
    #[must_use]
    pub fn contains(&self, anchor: Vec3, position: Vec3, radius: f32) -> bool {
        match *self {
            Self::Single => true,
            Self::Circle { radius: area } => horizontal_distance(anchor, position) - radius <= area,
            Self::Cone {
                yaw,
                range,
                half_angle,
            } => {
                let offset = Vec3::new(position.x - anchor.x, 0.0, position.z - anchor.z);
                let distance = offset.length();
                if distance - radius > range {
                    return false;
                }
                // A target standing on the caster is inside every cone: there is no
                // direction to test against, and being underneath something that is about
                // to swing is not an escape.
                if distance <= f32::EPSILON {
                    return true;
                }
                let facing = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
                let cosine = offset.dot(facing) / distance;
                cosine >= half_angle.cos()
            }
        }
    }

    /// The largest distance from its anchor this shape reaches, for a bounds check.
    #[must_use]
    pub fn reach(&self) -> f32 {
        match *self {
            Self::Single => 0.0,
            Self::Circle { radius } => radius,
            Self::Cone { range, .. } => range,
        }
    }

    /// Whether the shape covers an area rather than one target.
    #[must_use]
    pub fn is_area(&self) -> bool {
        !matches!(self, Self::Single)
    }
}

/// Where an ability is aimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyAim {
    /// At the target itself. A single-target attack, or an area centred on the victim.
    Target,
    /// A cone out of the caster, along its facing at the moment the cast started.
    Caster,
}

/// Every attack any enemy in the game can make.
///
/// A dense enum with a row in [`ENEMY_ABILITIES`], indexed by discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnemyAbility {
    /// The melee enemy's filler: instant, short range, single target.
    Swing,
    /// The melee enemy's telegraphed cone. Slow enough to walk out of.
    Cleave,
    /// The caster's telegraphed ground blast.
    GroundBlast,
    /// The caster's filler: a small bolt with a short cast and no area.
    Bolt,
}

/// How many enemy abilities exist.
pub const ENEMY_ABILITY_COUNT: usize = 4;

impl EnemyAbility {
    /// Every ability, in table order.
    pub const ALL: [Self; ENEMY_ABILITY_COUNT] =
        [Self::Swing, Self::Cleave, Self::GroundBlast, Self::Bolt];

    /// Its index into [`ENEMY_ABILITIES`].
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Everything about it.
    #[must_use]
    pub fn definition(self) -> &'static EnemyAbilityDef {
        &ENEMY_ABILITIES[self.index()]
    }

    /// Its name, for the log and the developer readout.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.definition().name
    }
}

/// Everything an enemy attack is.
///
/// Timing lives here and **never inside the state machine**: "how long is the wind-up" is a
/// property of the attack, and a boss that wants a four-second cast is a row, not a branch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyAbilityDef {
    /// Its name.
    pub name: &'static str,
    /// How far away it can be started, in metres, measured between edges.
    pub range: f32,
    /// Seconds of cast before it lands. Zero is instant.
    pub cast_time: f32,
    /// Seconds before it can be used again, counted from the moment it is started.
    pub cooldown: f32,
    /// Damage it deals to everything it catches.
    pub damage: f32,
    /// What kind of damage that is.
    pub damage_type: DamageType,
    /// The area it covers, and therefore what its telegraph draws.
    pub shape: TelegraphShape,
    /// Where the shape is anchored.
    pub aim: EnemyAim,
    /// Whether a player interrupt can stop it.
    ///
    /// A cast that cannot be interrupted is a cast the player has to move out of instead,
    /// which is a different kind of pressure and worth being able to express per ability.
    pub interruptible: bool,
    /// Seconds after it lands during which the enemy does nothing.
    ///
    /// The recovery window. It is what stops an enemy chaining attacks with no gap, and it
    /// is per ability because a big swing should leave a bigger opening than a small one.
    pub recovery: f32,
}

impl EnemyAbilityDef {
    /// Whether this attack has a cast bar at all.
    #[must_use]
    pub const fn is_cast(&self) -> bool {
        self.cast_time > 0.0
    }
}

/// The attack table.
///
/// Numbers are tuned against the player's 500 health and the class's own cast times: the
/// caster's blast is a fifth of the player's health and takes two seconds, so standing in
/// four of them is death and walking out of them is the whole lesson.
pub static ENEMY_ABILITIES: [EnemyAbilityDef; ENEMY_ABILITY_COUNT] = [
    EnemyAbilityDef {
        name: "SWING",
        range: 3.0,
        cast_time: 0.0,
        cooldown: 1.8,
        damage: 14.0,
        damage_type: DamageType::Physical,
        shape: TelegraphShape::Single,
        aim: EnemyAim::Target,
        interruptible: false,
        recovery: 0.4,
    },
    EnemyAbilityDef {
        name: "CLEAVE",
        range: 5.0,
        cast_time: 1.4,
        cooldown: 9.0,
        damage: 26.0,
        damage_type: DamageType::Physical,
        shape: TelegraphShape::Cone {
            // Rewritten at cast time with the caster's own facing; the stored yaw is the
            // neutral one, because a table cannot know which way something will be looking.
            yaw: 0.0,
            range: 6.0,
            half_angle: CONE_HALF_ANGLE,
        },
        aim: EnemyAim::Caster,
        interruptible: true,
        recovery: 0.8,
    },
    EnemyAbilityDef {
        name: "GROUND BLAST",
        range: 25.0,
        cast_time: 2.0,
        cooldown: 7.0,
        damage: 34.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 4.0 },
        aim: EnemyAim::Target,
        interruptible: true,
        recovery: 1.0,
    },
    EnemyAbilityDef {
        name: "BOLT",
        range: 25.0,
        cast_time: 1.2,
        cooldown: 2.4,
        damage: 12.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Single,
        aim: EnemyAim::Target,
        interruptible: true,
        recovery: 0.3,
    },
];

/// How many abilities one enemy may own.
pub const MAX_ENEMY_ABILITIES: usize = 4;

/// What kind of enemy something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnemyKind {
    /// Closes to melee range and swings, with a telegraphed cone on a cooldown.
    Melee,
    /// Keeps its distance and casts, with a telegraphed ground blast on a cooldown.
    Caster,
}

/// How many enemy kinds there are.
pub const ENEMY_KIND_COUNT: usize = 2;

impl EnemyKind {
    /// Every kind, in table order.
    pub const ALL: [Self; ENEMY_KIND_COUNT] = [Self::Melee, Self::Caster];

    /// Its index into [`ARCHETYPES`].
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Everything about it.
    #[must_use]
    pub fn archetype(self) -> &'static EnemyArchetype {
        &ARCHETYPES[self.index()]
    }

    /// Its name, for the nameplate and the developer readout.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.archetype().name
    }
}

/// One enemy type, as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyArchetype {
    /// Its name.
    pub name: &'static str,
    /// Health it spawns with.
    pub max_health: f32,
    /// Horizontal radius, in metres.
    pub radius: f32,
    /// Height, in metres.
    pub height: f32,
    /// How fast it moves, in metres per second.
    pub speed: f32,
    /// How close the player must come, in metres, before it notices.
    pub aggro_radius: f32,
    /// How far from home it will go before giving up, in metres.
    pub leash_radius: f32,
    /// How close it tries to be to its target, in metres. It stops here.
    pub preferred_range: f32,
    /// How far the target may drift past `preferred_range` before it moves again.
    ///
    /// A dead band: without one, an enemy at exactly its preferred range twitches between
    /// chasing and standing still every tick.
    pub range_slack: f32,
    /// Seconds between noticing the player and acting on it.
    ///
    /// The Alert state's whole content. It exists so a pull reads as a reaction rather than
    /// as an instant switch, and so presentation has a moment to play a notice.
    pub alert_time: f32,
    /// What it can do, in the order it prefers to do it.
    ///
    /// **Order is the whole decision policy**: the brain casts the first ability that is off
    /// cooldown and in range. A boss with a priority list is this list, longer.
    pub abilities: [Option<EnemyAbility>; MAX_ENEMY_ABILITIES],
}

impl EnemyArchetype {
    /// Its abilities, in preference order.
    pub fn abilities(&self) -> impl Iterator<Item = EnemyAbility> + '_ {
        self.abilities.iter().filter_map(|ability| *ability)
    }

    /// The longest range any of its abilities reaches.
    #[must_use]
    pub fn longest_range(&self) -> f32 {
        self.abilities()
            .map(|ability| ability.definition().range)
            .fold(0.0, f32::max)
    }
}

/// The enemy table.
pub static ARCHETYPES: [EnemyArchetype; ENEMY_KIND_COUNT] = [
    EnemyArchetype {
        name: "BRUTE",
        max_health: 320.0,
        radius: 0.6,
        height: 2.0,
        // Slower than the player's 7 m/s: a melee enemy that could not be kited would make
        // Blink and the class's movement pointless.
        speed: 5.2,
        aggro_radius: 14.0,
        leash_radius: 40.0,
        preferred_range: 2.4,
        range_slack: 0.6,
        alert_time: 0.35,
        abilities: [
            Some(EnemyAbility::Cleave),
            Some(EnemyAbility::Swing),
            None,
            None,
        ],
    },
    EnemyArchetype {
        name: "EMBER ADEPT",
        max_health: 210.0,
        radius: 0.5,
        height: 1.9,
        speed: 4.2,
        aggro_radius: 20.0,
        leash_radius: 45.0,
        // It stops well short of its 25 m reach, so it stands where the player can see it
        // and reach it rather than sniping from the edge of the arena.
        preferred_range: 16.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::GroundBlast),
            Some(EnemyAbility::Bolt),
            None,
            None,
        ],
    },
];

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn both_tables_are_in_variant_order() {
        for kind in EnemyKind::ALL {
            assert_eq!(ARCHETYPES[kind.index()].name, kind.name(), "{kind:?}");
        }
        for ability in EnemyAbility::ALL {
            assert_eq!(
                ENEMY_ABILITIES[ability.index()].name,
                ability.name(),
                "{ability:?}"
            );
        }
    }

    #[test]
    fn every_archetype_can_actually_reach_its_own_attacks() {
        for kind in EnemyKind::ALL {
            let archetype = kind.archetype();
            assert!(archetype.abilities().count() > 0, "{kind:?} has no attack");
            assert!(
                archetype.preferred_range <= archetype.longest_range(),
                "{kind:?} stands further away than it can reach"
            );
            assert!(archetype.aggro_radius < archetype.leash_radius, "{kind:?}");
            assert!(
                archetype.max_health > 0.0 && archetype.speed > 0.0,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_circle_catches_a_wide_body_at_its_rim() {
        let shape = TelegraphShape::Circle { radius: 4.0 };
        let centre = Vec3::ZERO;
        assert!(shape.contains(centre, Vec3::new(4.5, 0.0, 0.0), 0.7));
        assert!(!shape.contains(centre, Vec3::new(6.0, 0.0, 0.0), 0.7));
        // Height is ignored: these are footprints on the ground.
        assert!(shape.contains(centre, Vec3::new(0.0, 50.0, 0.0), 0.0));
    }

    #[test]
    fn a_cone_catches_what_is_in_front_of_it_and_nothing_behind() {
        let shape = TelegraphShape::Cone {
            yaw: 0.0,
            range: 6.0,
            half_angle: CONE_HALF_ANGLE,
        };
        let anchor = Vec3::ZERO;
        // Yaw zero faces −Z.
        assert!(shape.contains(anchor, Vec3::new(0.0, 0.0, -4.0), 0.5));
        assert!(!shape.contains(anchor, Vec3::new(0.0, 0.0, 4.0), 0.5));
        assert!(!shape.contains(anchor, Vec3::new(0.0, 0.0, -9.0), 0.5));
        // Just outside thirty degrees.
        assert!(!shape.contains(anchor, Vec3::new(4.0, 0.0, -4.0), 0.0));
        assert!(shape.contains(anchor, Vec3::new(1.0, 0.0, -4.0), 0.0));
    }

    #[test]
    fn a_target_standing_on_the_caster_is_inside_every_shape() {
        let cone = TelegraphShape::Cone {
            yaw: 1.3,
            range: 6.0,
            half_angle: CONE_HALF_ANGLE,
        };
        assert!(cone.contains(Vec3::ZERO, Vec3::ZERO, 0.5));
        assert!(TelegraphShape::Single.contains(Vec3::ZERO, Vec3::new(99.0, 0.0, 0.0), 0.0));
    }

    #[test]
    fn a_cast_is_a_cast_and_a_swing_is_not() {
        assert!(!EnemyAbility::Swing.definition().is_cast());
        assert!(EnemyAbility::GroundBlast.definition().is_cast());
        assert!(EnemyAbility::GroundBlast.definition().interruptible);
        assert!(!EnemyAbility::Swing.definition().interruptible);
    }
}
