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
pub mod hazard;
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

/// Who an ability is used *on*.
///
/// Separate from [`EnemyAim`], which says where the shape is anchored. This says who the
/// brain looks for before it will consider the ability at all — and it is the whole reason a
/// healer needs no code of its own: a support monster is an archetype whose first ability
/// selects a [`EnemyTarget::WoundedAlly`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyTarget {
    /// Whoever the enemy is fighting.
    Victim,
    /// The most wounded living ally in range, the caster included.
    ///
    /// If nothing is wounded there is no target, the ability is not chosen, and the enemy
    /// falls through to the next entry in its priority list.
    WoundedAlly,
}

/// What an ability *does* when it resolves.
///
/// Damage is the default and was the only case before this dungeon. The other two are
/// generic capabilities rather than monster-specific code: any archetype can heal, and any
/// archetype can leave burning ground, because both are rows in [`ENEMY_ABILITIES`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnemyEffect {
    /// Deals [`EnemyAbilityDef::damage`] to everything the shape catches.
    Damage,
    /// Restores health to whoever it was used on.
    Heal {
        /// Health restored.
        amount: f32,
    },
    /// Deals its damage once, then leaves ground that keeps burning.
    ///
    /// The hazard covers the ability's own [`EnemyAbilityDef::shape`], so what the telegraph
    /// drew, what the impact hit and what the pool covers are one value.
    Hazard {
        /// Seconds the ground burns for.
        duration: f32,
        /// Damage a second to anything standing in it.
        damage_per_second: f32,
    },
}

impl EnemyEffect {
    /// Whether this effect restores health rather than removing it.
    #[must_use]
    pub const fn is_healing(self) -> bool {
        matches!(self, Self::Heal { .. })
    }
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
    /// The Bone Ritualist's three-second nova. The interrupt lesson.
    RitualNova,
    /// The Crypt Warden's heal, used on whichever ally is worst off.
    Mend,
    /// The Cinder Weaver's burning ground.
    CinderPool,
    /// The Ash Revenant's melee filler. A heavier Swing.
    Sunder,
    /// The Ash Revenant's uninterruptible blast, centred on itself.
    AshBurst,
    /// The boss's wide cone. Uninterruptible: it is walked out of, not stopped.
    WardensSweep,
    /// The boss's ranged filler. What reaches a player who has run.
    EmberLance,
    /// The boss's interruptible circle. The fight's one interrupt window.
    Ashfall,
    /// The boss's burning ground, from phase two onwards.
    CinderRain,
    /// The boss's phase-three climax: a fourteen-metre circle centred on itself.
    ImmolationNova,
}

/// How many enemy abilities exist.
pub const ENEMY_ABILITY_COUNT: usize = 14;

impl EnemyAbility {
    /// Every ability, in table order.
    pub const ALL: [Self; ENEMY_ABILITY_COUNT] = [
        Self::Swing,
        Self::Cleave,
        Self::GroundBlast,
        Self::Bolt,
        Self::RitualNova,
        Self::Mend,
        Self::CinderPool,
        Self::Sunder,
        Self::AshBurst,
        Self::WardensSweep,
        Self::EmberLance,
        Self::Ashfall,
        Self::CinderRain,
        Self::ImmolationNova,
    ];

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
    /// Who the ability is used on, which decides whether it is available at all.
    pub select: EnemyTarget,
    /// What happens when it resolves.
    pub effect: EnemyEffect,
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
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
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
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
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
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
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
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: true,
        recovery: 0.3,
    },
    // Three seconds and 85 damage in a 6.5 m circle: the largest thing in the trash roster,
    // and the one the player's interrupt exists to answer.
    EnemyAbilityDef {
        name: "RITUAL NOVA",
        range: 24.0,
        cast_time: 3.0,
        cooldown: 13.0,
        damage: 85.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 6.5 },
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: true,
        recovery: 1.4,
    },
    // Nearly half a Brute, on a nine-second cooldown. Not chosen at all unless something is
    // actually wounded, which is what makes an untouched pack's Warden open with Bolts.
    EnemyAbilityDef {
        name: "MEND",
        range: 22.0,
        cast_time: 2.0,
        cooldown: 9.0,
        damage: 0.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Single,
        aim: EnemyAim::Target,
        select: EnemyTarget::WoundedAlly,
        effect: EnemyEffect::Heal { amount: 140.0 },
        interruptible: true,
        recovery: 0.8,
    },
    // Small on impact, expensive to stand in: the pool is the mechanic, not the landing.
    EnemyAbilityDef {
        name: "CINDER POOL",
        range: 22.0,
        cast_time: 1.6,
        cooldown: 7.0,
        damage: 8.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 3.5 },
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Hazard {
            duration: 9.0,
            damage_per_second: 14.0,
        },
        interruptible: true,
        recovery: 0.6,
    },
    EnemyAbilityDef {
        name: "SUNDER",
        range: 3.5,
        cast_time: 0.0,
        cooldown: 2.6,
        damage: 22.0,
        damage_type: DamageType::Physical,
        shape: TelegraphShape::Single,
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: false,
        recovery: 0.5,
    },
    // Centred on the caster and impossible to interrupt: the answer is distance, and 1.8 s
    // at the player's 7 m/s covers more than twice the radius.
    EnemyAbilityDef {
        name: "ASH BURST",
        range: 7.0,
        cast_time: 1.8,
        cooldown: 11.0,
        damage: 46.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 7.0 },
        aim: EnemyAim::Caster,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: false,
        recovery: 1.0,
    },
    EnemyAbilityDef {
        name: "WARDENS SWEEP",
        range: 8.0,
        cast_time: 1.7,
        cooldown: 8.0,
        damage: 42.0,
        damage_type: DamageType::Physical,
        shape: TelegraphShape::Cone {
            yaw: 0.0,
            range: 11.0,
            half_angle: CONE_HALF_ANGLE,
        },
        aim: EnemyAim::Caster,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: false,
        recovery: 0.9,
    },
    EnemyAbilityDef {
        name: "EMBER LANCE",
        range: 30.0,
        cast_time: 1.0,
        cooldown: 3.5,
        damage: 20.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Single,
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: true,
        recovery: 0.3,
    },
    EnemyAbilityDef {
        name: "ASHFALL",
        range: 28.0,
        cast_time: 2.2,
        cooldown: 10.0,
        damage: 55.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 6.0 },
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: true,
        recovery: 1.0,
    },
    EnemyAbilityDef {
        name: "CINDER RAIN",
        range: 26.0,
        cast_time: 2.0,
        cooldown: 11.0,
        damage: 10.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 4.5 },
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Hazard {
            duration: 10.0,
            damage_per_second: 16.0,
        },
        interruptible: false,
        recovery: 0.8,
    },
    // The largest telegraph in the game. Its range is deliberately longer than the arena is
    // wide, so it is never withheld for being "out of range" — the escape is the counterplay.
    EnemyAbilityDef {
        name: "IMMOLATION NOVA",
        range: 40.0,
        cast_time: 3.2,
        cooldown: 15.0,
        damage: 120.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 14.0 },
        aim: EnemyAim::Caster,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: false,
        recovery: 1.6,
    },
];

/// How many abilities one enemy may own in one phase.
pub const MAX_ENEMY_ABILITIES: usize = 6;

/// Seconds an enemy does nothing for after changing phase.
///
/// Not a reward window — there is no loot in this phase — but a *readability* one: the fight
/// visibly stops and restarts as something else.
pub const PHASE_STAGGER: f32 = 1.5;

/// One phase of a multi-phase enemy.
///
/// A phase is a health threshold, a priority list and two scalars. That is the whole of it:
/// the boss and the Ash Revenant use the same struct, and neither has a line of code of its
/// own. Adding a fourth boss phase is a row here, not a branch in the state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyPhaseDef {
    /// Its name, for the readout and the boss frame.
    pub name: &'static str,
    /// The health fraction at or below which the enemy enters this phase.
    ///
    /// Phases are listed in descending threshold order and only ever advance, so healing an
    /// enemy back over a threshold does not put it back a phase.
    pub at_or_below: f32,
    /// What it can do in this phase, in preference order. Replaces the previous list.
    pub abilities: [Option<EnemyAbility>; MAX_ENEMY_ABILITIES],
    /// Multiplies its movement speed.
    pub speed_scale: f32,
    /// Multiplies every cooldown it starts in this phase.
    pub cooldown_scale: f32,
}

/// What kind of enemy something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnemyKind {
    /// Closes to melee range and swings, with a telegraphed cone on a cooldown.
    Melee,
    /// Keeps its distance and casts, with a telegraphed ground blast on a cooldown.
    Caster,
    /// Interrupt-critical caster: a three-second nova that must be stopped.
    Ritualist,
    /// Support: heals whichever of its allies is worst off.
    Warden,
    /// Area controller: leaves burning ground where the player is standing.
    Weaver,
    /// Elite melee specialist with a second phase of its own.
    Revenant,
    /// The dungeon's boss. Three phases, five abilities.
    Boss,
}

/// How many enemy kinds there are.
pub const ENEMY_KIND_COUNT: usize = 7;

impl EnemyKind {
    /// Every kind, in table order.
    pub const ALL: [Self; ENEMY_KIND_COUNT] = [
        Self::Melee,
        Self::Caster,
        Self::Ritualist,
        Self::Warden,
        Self::Weaver,
        Self::Revenant,
        Self::Boss,
    ];

    /// Whether this kind is a boss, for the boss frame and the encounter readout.
    #[must_use]
    pub fn is_boss(self) -> bool {
        matches!(self, Self::Boss)
    }

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
    ///
    /// This is phase one. [`EnemyArchetype::phases`] holds the ones after it.
    pub abilities: [Option<EnemyAbility>; MAX_ENEMY_ABILITIES],
    /// The phases *after* the first, in descending threshold order.
    ///
    /// Empty for everything that does not change while it is being fought, which is most of
    /// the roster. A phase replaces the ability list and scales speed and cooldowns; see
    /// [`EnemyPhaseDef`].
    pub phases: &'static [EnemyPhaseDef],
}

impl EnemyArchetype {
    /// Its first phase's abilities, in preference order.
    pub fn abilities(&self) -> impl Iterator<Item = EnemyAbility> + '_ {
        self.abilities.iter().filter_map(|ability| *ability)
    }

    /// The abilities available in `phase`, in preference order.
    ///
    /// Phase zero is [`EnemyArchetype::abilities`]; every later index is a row in
    /// [`EnemyArchetype::phases`]. An index past the end clamps to the last phase, so a
    /// caller can never read off the end of a boss that gained a phase.
    pub fn phase_abilities(&self, phase: usize) -> impl Iterator<Item = EnemyAbility> + '_ {
        let list = match phase.checked_sub(1) {
            None => &self.abilities,
            Some(index) => match self.phases.get(index).or_else(|| self.phases.last()) {
                Some(phase) => &phase.abilities,
                None => &self.abilities,
            },
        };
        list.iter().filter_map(|ability| *ability)
    }

    /// How many phases it has in total, counting the first.
    #[must_use]
    pub fn phase_count(&self) -> usize {
        self.phases.len() + 1
    }

    /// The name of `phase`, or `"PHASE 1"` when it has only one.
    #[must_use]
    pub fn phase_name(&self, phase: usize) -> &'static str {
        match phase.checked_sub(1) {
            None => "PHASE 1",
            Some(index) => self.phases.get(index).map_or("PHASE 1", |phase| phase.name),
        }
    }

    /// How fast it moves in `phase`, in metres per second.
    #[must_use]
    pub fn phase_speed(&self, phase: usize) -> f32 {
        self.speed * self.phase_scalars(phase).0
    }

    /// What its cooldowns are multiplied by in `phase`.
    #[must_use]
    pub fn phase_cooldown_scale(&self, phase: usize) -> f32 {
        self.phase_scalars(phase).1
    }

    /// The speed and cooldown scalars of `phase`.
    fn phase_scalars(&self, phase: usize) -> (f32, f32) {
        match phase.checked_sub(1) {
            None => (1.0, 1.0),
            Some(index) => self.phases.get(index).map_or((1.0, 1.0), |phase| {
                (phase.speed_scale, phase.cooldown_scale)
            }),
        }
    }

    /// Which phase a `fraction` of health belongs to, never below `current`.
    ///
    /// Phases only advance: healing an enemy back over a threshold does not put it back, and
    /// a single hit that crosses two thresholds lands in the deeper of the two.
    #[must_use]
    pub fn phase_for_health(&self, fraction: f32, current: usize) -> usize {
        let mut phase = current;
        for (index, definition) in self.phases.iter().enumerate() {
            if fraction <= definition.at_or_below {
                phase = phase.max(index + 1);
            }
        }
        phase
    }

    /// The longest range any of its abilities reaches, in any phase.
    #[must_use]
    pub fn longest_range(&self) -> f32 {
        (0..self.phase_count())
            .flat_map(|phase| {
                self.phase_abilities(phase)
                    .map(|ability| ability.definition().range)
            })
            .fold(0.0, f32::max)
    }
}

/// The Ash Revenant's second phase: faster, shorter cooldowns, and the Brute's cone on top.
static REVENANT_PHASES: [EnemyPhaseDef; 1] = [EnemyPhaseDef {
    name: "PHASE 2",
    at_or_below: 0.5,
    abilities: [
        Some(EnemyAbility::AshBurst),
        Some(EnemyAbility::Cleave),
        Some(EnemyAbility::Sunder),
        None,
        None,
        None,
    ],
    speed_scale: 1.15,
    cooldown_scale: 0.7,
}];

/// The boss's phases. Descending thresholds, and each one adds a mechanic rather than a
/// multiplier — see `docs/dungeon/boss/warden_of_the_ashen_gate.md`.
static BOSS_PHASES: [EnemyPhaseDef; 2] = [
    EnemyPhaseDef {
        name: "PHASE 2 - THE ASH",
        at_or_below: 0.66,
        abilities: [
            Some(EnemyAbility::CinderRain),
            Some(EnemyAbility::WardensSweep),
            Some(EnemyAbility::Ashfall),
            Some(EnemyAbility::EmberLance),
            Some(EnemyAbility::Swing),
            None,
        ],
        speed_scale: 1.08,
        cooldown_scale: 0.85,
    },
    EnemyPhaseDef {
        name: "PHASE 3 - THE BURNING",
        at_or_below: 0.33,
        abilities: [
            Some(EnemyAbility::ImmolationNova),
            Some(EnemyAbility::CinderRain),
            Some(EnemyAbility::WardensSweep),
            Some(EnemyAbility::Ashfall),
            Some(EnemyAbility::Swing),
            None,
        ],
        speed_scale: 1.15,
        cooldown_scale: 0.7,
    },
];

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
            None,
            None,
        ],
        phases: &[],
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
            None,
            None,
        ],
        phases: &[],
    },
    // Stands further back than the Adept, so in a mixed pack reaching it is a decision.
    EnemyArchetype {
        name: "BONE RITUALIST",
        max_health: 260.0,
        radius: 0.5,
        height: 1.9,
        speed: 4.0,
        aggro_radius: 18.0,
        leash_radius: 45.0,
        preferred_range: 18.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::RitualNova),
            Some(EnemyAbility::Bolt),
            None,
            None,
            None,
            None,
        ],
        phases: &[],
    },
    // Closer in than the other casters: a healer that stands further back than everything it
    // heals is a healer the player cannot reach, and the mechanic is target priority.
    EnemyArchetype {
        name: "CRYPT WARDEN",
        max_health: 240.0,
        radius: 0.5,
        height: 1.9,
        speed: 4.4,
        aggro_radius: 16.0,
        leash_radius: 45.0,
        preferred_range: 14.0,
        range_slack: 3.0,
        alert_time: 0.45,
        abilities: [
            Some(EnemyAbility::Mend),
            Some(EnemyAbility::Bolt),
            None,
            None,
            None,
            None,
        ],
        phases: &[],
    },
    EnemyArchetype {
        name: "CINDER WEAVER",
        max_health: 230.0,
        radius: 0.5,
        height: 1.9,
        speed: 4.4,
        aggro_radius: 18.0,
        leash_radius: 45.0,
        preferred_range: 15.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::CinderPool),
            Some(EnemyAbility::Bolt),
            None,
            None,
            None,
            None,
        ],
        phases: &[],
    },
    EnemyArchetype {
        name: "ASH REVENANT",
        max_health: 900.0,
        radius: 0.75,
        height: 2.3,
        speed: 5.0,
        aggro_radius: 16.0,
        leash_radius: 45.0,
        preferred_range: 2.6,
        range_slack: 0.6,
        // The longest alert in the roster: an elite gets a longer beat before it starts.
        alert_time: 0.6,
        abilities: [
            Some(EnemyAbility::AshBurst),
            Some(EnemyAbility::Sunder),
            None,
            None,
            None,
            None,
        ],
        phases: &REVENANT_PHASES,
    },
    EnemyArchetype {
        name: "WARDEN OF THE ASHEN GATE",
        max_health: 6000.0,
        radius: 1.1,
        height: 3.0,
        // Slower than the player in every phase. Nothing in this fight kills a player who is
        // moving, and everything in it kills a player who is not.
        speed: 4.8,
        aggro_radius: 28.0,
        leash_radius: 70.0,
        preferred_range: 3.0,
        range_slack: 0.8,
        alert_time: 0.8,
        abilities: [
            Some(EnemyAbility::WardensSweep),
            Some(EnemyAbility::Ashfall),
            Some(EnemyAbility::EmberLance),
            Some(EnemyAbility::Swing),
            None,
            None,
        ],
        phases: &BOSS_PHASES,
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
