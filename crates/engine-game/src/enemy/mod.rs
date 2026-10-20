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
pub mod strike;
pub mod threat;

use engine_core::math::Vec3;

use crate::combat::DamageType;
use crate::status::StatusKind;
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
    /// A rectangle starting at the anchor and running along `yaw`: a spear, a volley, the
    /// path of a charge.
    ///
    /// The same convention the player's own line zones use (`firemage::zone::ZoneShape::Line`):
    /// anchored at its near end, `width` is the whole width. One convention for two lines, so
    /// the effect that draws one draws the other.
    Line {
        /// Which way it runs, radians about +Y. Zero faces −Z.
        yaw: f32,
        /// How far it runs, in metres.
        length: f32,
        /// How wide it is, in metres, edge to edge.
        width: f32,
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
            Self::Line { yaw, length, width } => {
                let (dx, dz) = (position.x - anchor.x, position.z - anchor.z);
                let (fx, fz) = (-yaw.sin(), -yaw.cos());
                // Along the line and across it: the rectangle's own axes, with the body's
                // radius on every side so a wide target at the edge is caught.
                let along = dx * fx + dz * fz;
                let across = dx * -fz + dz * fx;
                along >= -radius && along <= length + radius && across.abs() <= width * 0.5 + radius
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
            Self::Line { length, .. } => length,
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
    /// The hostile furthest from the caster, within the ability's range.
    ///
    /// What a charge is aimed at: the one standing back, not the one in its face. Together
    /// with [`EnemyAbilityDef::min_range`] it is what makes a charge a positioning problem for
    /// the whole party rather than a second swing at the tank.
    Farthest,
    /// A hostile chosen at random when the cast starts, preferring anyone who is *not* the
    /// enemy's current target.
    ///
    /// Marks, garrotes, slows: the abilities that exist to reach past the tank. Random from the
    /// simulation's own seeded generator, so a replay of the same seed marks the same player.
    Random,
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
        /// The radius it spreads to, in steps, over its life. Zero is a pool that stays the
        /// size it landed at.
        ///
        /// Spreading Corruption's whole mechanic: the safe floor shrinks while the party
        /// fights, so standing at the edge of a pool is a decision with a clock on it.
        grow_to: f32,
        /// The pool's radius, when it is not the shape's own reach. Zero means the shape's.
        ///
        /// What lets a *line* leave a *circle*: a volley's arrows are a rectangle on the way
        /// in and a patch of cursed ground where each one stops.
        radius: f32,
    },
    /// A cast that does its damage *while* it is being cast, a tick at a time.
    ///
    /// Stopped by an interrupt like any other cast, which is the point: every tick that did
    /// not happen is damage the party did not take, and a channel whose ticks grow is a cast
    /// that gets more urgent to stop the longer it runs.
    Channel {
        /// Seconds between ticks.
        interval: f32,
        /// How much each tick grows over the one before, as a fraction of the first.
        ///
        /// Zero is a steady channel. Grave Drain's 0.35 makes its eighth tick three and a half
        /// times its first.
        ramp: f32,
    },
    /// A cast on one player: the caster runs at them, follows them, and hits them when it
    /// arrives — then slows them.
    ///
    /// Not telegraphed on the floor and not dodged: the cast bar says who is about to be run
    /// at, and the answer is to be ready for the hit and the slow, not to step off a line. The
    /// run goes through the collision world, so a charge into a pillar stops at the pillar.
    Charge {
        /// How fast it runs, in metres a second.
        speed: f32,
        /// A circle hit around where it lands, in metres, besides its target. Zero is the
        /// target alone — a pounce's landing blast is the case that is not zero.
        impact_radius: f32,
        /// A second, delayed ring around the landing, in metres. Zero is none.
        ///
        /// The empowered charges' escalation: the landing is survived by being out of the
        /// impact, and the shockwave by then *leaving* it.
        shockwave_radius: f32,
        /// Damage a second the ground it ran over deals afterwards. Zero leaves no trail.
        trail_damage_per_second: f32,
        /// Seconds the trail lasts.
        trail_duration: f32,
        /// How much the charged player is slowed when it lands, as a fraction.
        slow: f32,
        /// Seconds the slow lasts.
        slow_duration: f32,
    },
    /// The caster attacks faster for a while: an escalation, not a defence.
    ///
    /// Divides every cooldown and recovery it starts while it lasts. Nothing about it makes
    /// the monster harder to kill — it makes the monster more urgent to kill.
    Frenzy {
        /// Seconds it lasts.
        duration: f32,
        /// How much faster it attacks, as a fraction: 0.6 is sixty per cent faster.
        haste: f32,
    },
    /// A long cast that, **if it completes**, heals the caster and drives every living ally
    /// to attack faster for a while.
    ///
    /// The interrupt-priority ability: the cast bar is the warning, a kick is the answer, and
    /// what an unanswered one buys is the point of the fight's second half. Aimed at the
    /// caster itself, so a victim behind a pillar does not lose it.
    Hymn {
        /// Health the caster recovers, as a fraction of its maximum.
        heal_fraction: f32,
        /// How much faster its allies attack, as a fraction: 0.6 is sixty per cent faster.
        haste: f32,
        /// Seconds the allies' haste lasts.
        duration: f32,
    },
    /// A channel that drags every hostile in reach towards the caster while drowning pools
    /// open around it, and slams the ground at its feet when it ends.
    ///
    /// Uninterruptible: the answer is movement. Walking away from the caster at full speed
    /// out-runs the current, so the pull is a tax on casting and on being slowed rather than
    /// a death sentence. The ability's own [`EnemyAbilityDef::damage`] is the slam, hitting
    /// its [`EnemyAbilityDef::shape`] around the caster.
    Undertow {
        /// Seconds between the pools it opens.
        interval: f32,
        /// How fast the current drags, in metres a second.
        pull_speed: f32,
        /// How far from the caster the current reaches, in metres.
        pull_range: f32,
        /// Radius of each pool, in metres.
        pool_radius: f32,
        /// Damage a second to anything standing in a pool.
        pool_damage_per_second: f32,
        /// Seconds a pool lasts.
        pool_duration: f32,
        /// The nearest a pool opens to the caster, in metres.
        pool_near: f32,
        /// The furthest a pool opens from the caster, in metres.
        pool_far: f32,
    },
    /// A cast that, **if it completes**, puts a status on every living ally near the caster:
    /// itself included.
    ///
    /// The registry row says what the status does and for how long, so the same effect is a
    /// shielder's ward (less damage taken) and a foreman's rally (more damage dealt). Bosses
    /// are never bestowed upon: a ward on the thing the party came to kill would only be a
    /// second health bar. The cast bar is the warning and a kick is the answer.
    Bestow {
        /// Which status.
        kind: StatusKind,
        /// How far from the caster an ally is still reached, in metres.
        radius: f32,
        /// Seconds the status lasts.
        duration: f32,
    },
    /// A blast around the caster that costs it its life: the damage hits the ability's own
    /// [`EnemyAbilityDef::shape`], and then the caster dies.
    ///
    /// The cast is the fuse, drawn on the floor for its whole length. Killing the caster before
    /// it ends, or walking out of the circle, defuses it. A detonation counts as the caster's
    /// death for completion, like any other kill.
    Detonate,
    /// A delayed zone whose damage is **shared** between everyone standing in it when it lands.
    ///
    /// One player alone takes all of it, a full group takes a fraction each, and if nobody
    /// stands in it the party takes `overflow` of the total *each* - so the answer is to stack
    /// on the marked ground, the opposite of every spread mechanic. The ability's
    /// [`EnemyAbilityDef::damage`] is the total.
    Soak {
        /// The share of the total every player takes when nobody soaked it, as a fraction.
        overflow: f32,
    },
    /// A channel that pushes everyone inside the ability's [`TelegraphShape::Line`] along it,
    /// away from the caster, while slag pools open at its far end; whoever is still in the lane
    /// when it ends takes the ability's own damage.
    ///
    /// The push is a collided slide (the same displacement an Undertow or a charge makes), slower
    /// than a walk, so stepping out of the lane to the side beats it by a wide margin and
    /// standing in it carries you into the pools. Uninterruptible: the answer is movement.
    Conveyor {
        /// How fast the lane carries, in metres a second.
        push_speed: f32,
        /// Seconds between the pools it opens.
        interval: f32,
        /// Radius of each pool, in metres.
        pool_radius: f32,
        /// Damage a second to anything standing in a pool.
        pool_damage_per_second: f32,
        /// Seconds a pool lasts.
        pool_duration: f32,
        /// The nearest a pool opens along the lane, in metres from the caster.
        pool_near: f32,
        /// The furthest a pool opens along the lane, in metres from the caster.
        pool_far: f32,
    },
}

impl EnemyEffect {
    /// Whether this effect restores health rather than removing it.
    #[must_use]
    pub const fn is_healing(self) -> bool {
        matches!(self, Self::Heal { .. })
    }

    /// Whether this effect is a channel: damage while casting rather than when it lands.
    #[must_use]
    pub const fn is_channel(self) -> bool {
        matches!(self, Self::Channel { .. })
    }

    /// Whether this is the pulling channel.
    #[must_use]
    pub const fn is_undertow(self) -> bool {
        matches!(self, Self::Undertow { .. })
    }

    /// Whether this is the pushing channel.
    #[must_use]
    pub const fn is_conveyor(self) -> bool {
        matches!(self, Self::Conveyor { .. })
    }

    /// Whether the caster runs at its target when this lands.
    #[must_use]
    pub const fn is_charge(self) -> bool {
        matches!(self, Self::Charge { .. })
    }

    /// Whether this is aimed at the caster itself rather than at anybody else.
    #[must_use]
    pub const fn is_self(self) -> bool {
        matches!(
            self,
            Self::Frenzy { .. }
                | Self::Hymn { .. }
                | Self::Undertow { .. }
                | Self::Bestow { .. }
        )
    }
}

/// What an enemy hit leaves on its victim, besides the damage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnemyRider {
    /// Nothing.
    None,
    /// A wound that ticks, and stacks with every hit that lands another.
    ///
    /// The Ghoul's lesson: one bleed is noise, eight is a healer's whole attention. The
    /// stacks are the status system's own, so the nameplate and the unit frame already know
    /// how to show them.
    Bleed {
        /// Damage a second, per stack.
        damage_per_stack: f32,
        /// Seconds the wound lasts; every new stack refreshes it.
        duration: f32,
        /// Stacks one hit adds.
        stacks: u8,
        /// The most it can reach.
        max_stacks: u8,
    },
    /// Slower movement for a while.
    Slow {
        /// How much slower, as a fraction: 0.45 is forty-five per cent slower.
        fraction: f32,
        /// Seconds it lasts.
        duration: f32,
    },
    /// A status from the effect registry, one stack a hit.
    ///
    /// What lets an enemy apply a *dispellable* debuff without a rider of its own: the
    /// registry row says what it does and which school removes it.
    Afflict {
        /// Which effect.
        kind: StatusKind,
        /// Seconds it lasts; every new stack refreshes it.
        duration: f32,
        /// The most stacks it reaches.
        max_stacks: u8,
    },
}

/// A condition an ability needs before it is even considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnemyRequirement {
    /// Always available.
    None,
    /// Only against a target that is already bleeding.
    ///
    /// The empowered Ghoul's extra attacks: an ability that is simply not on the list until
    /// its own bleed is, so the brain needs no rule about it.
    VictimBleeding,
}

/// Which kind of gathering an enemy cast sounds and looks like.
///
/// Gameplay's classification, not a sound: presentation maps each to an effect and a sound it
/// already owns. What keeps the audio director from having a `match` with a row per enemy
/// attack, which is what it had before and what adding forty attacks would have made absurd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnemyCue {
    /// A short, sharp gather: bolts, arrows, swings.
    Quick,
    /// A sweeping wind-up: cones, charges, slams.
    Sweep,
    /// A long, heavy gather: the big circles, the marks, the channels.
    Heavy,
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

    // ---------------------------------------------------------------- the Graveyard Crypt
    /// Skeleton Bonecaster: the filler bolt.
    BoneBolt,
    /// Skeleton Bonecaster: a scatter of impact zones around a target.
    BoneBarrage,
    /// Skeleton Bonecaster: a mark that follows its victim and detonates around them.
    GraveExplosion,
    /// Skeleton Bonecaster: a long line through its target.
    BoneSpear,
    /// Grave Acolyte: the filler bolt.
    ShadowBolt,
    /// Grave Acolyte: a pool of cursed ground under a player.
    CorruptedGround,
    /// Grave Acolyte: an interruptible channel pulsing a circle.
    DarkPulse,
    /// Grave Acolyte: an interruptible channel whose ticks grow.
    GraveDrain,
    /// Crypt Archer: the filler shot.
    BoneArrow,
    /// Crypt Archer: one arrow at each of several players.
    MultiShot,
    /// Crypt Archer: a shot that slows.
    CripplingArrow,
    /// Crypt Archer: marked circles that land after a delay.
    RainOfArrows,
    /// Skeleton Warrior: the filler swing.
    BoneSlash,
    /// Skeleton Warrior: a wide frontal cone.
    BoneCleave,
    /// Skeleton Warrior: a charge at whoever stands furthest back.
    BoneCharge,
    /// Skeleton Warrior: a slow, heavy, interruptible strike.
    ExecutionStrike,
    /// Grave Brute: the heavy filler.
    CrushingBlow,
    /// Grave Brute: a circle around itself.
    GroundSlam,
    /// Grave Brute: a charge at a distant player.
    GraveCharge,
    /// Grave Brute: a delayed ring around itself.
    BoneQuake,
    /// Crypt Ghoul: the fast filler.
    GhoulStrike,
    /// Crypt Ghoul: a hit that leaves a stacking bleed.
    RendingClaws,
    /// Crypt Ghoul: a leap at a random player, landing in a circle.
    Pounce,
    /// Crypt Ghoul: attacks much faster for a while.
    BloodFrenzy,

    // ---------------------------------------------------------------- the empowered adds
    /// Empowered Bonecaster: more, larger, faster impact zones.
    MassiveBoneBarrage,
    /// Empowered Bonecaster: several marks, and a detonation marks whoever it catches.
    GraveChainExplosion,
    /// Empowered Acolyte: a pool that spreads while it burns.
    SpreadingCorruption,
    /// Empowered Acolyte: Grave Drain on several players at once.
    MassGraveDrain,
    /// Empowered Archer: more rain, sooner.
    DeadlyRain,
    /// Empowered Archer: lines through several players that leave cursed ground.
    BoneVolley,
    /// Empowered Warrior: a wider, harder cleave.
    Massacre,
    /// Empowered Warrior: a charge whose landing sends out a shockwave.
    ShatteringCharge,
    /// Empowered Brute: impact zones scattered all around it.
    Gravequake,
    /// Empowered Brute: a charge that leaves a burning trail.
    DevastatingCharge,
    /// Empowered Ghoul: a harder, taller bleed.
    RendingFrenzy,
    /// Empowered Ghoul: extra attacks, only against a bleeding target.
    FeastOnTheWounded,
    /// Empowered Ghoul: a leap whose landing explodes, then explodes again.
    SavagePounce,

    // ---------------------------------------------------------------- the Hollow Steward
    /// Lord Ashgrave: the rapier filler.
    RapierThrust,
    /// Lord Ashgrave: a heavy riposte on the tank that slows.
    GougingRiposte,
    /// Lord Ashgrave: a long bleed on somebody who is not the tank.
    Garrote,
    /// Lord Ashgrave: a spinning circle around himself.
    DanceOfBlades,
    /// Lord Ashgrave: a charge through the room that leaves a trail.
    GhastlyWaltz,
    /// Lord Ashgrave: a mark that follows a player and detonates around them.
    MarkOfTheHost,
    /// Lord Ashgrave, unmasked: attacks much faster for a while.
    CrimsonFrenzy,

    // ---------------------------------------------------------------- the Drowned Catacombs
    /// Drowned Tidecaller: a heal on the most wounded ally.
    TidalMend,
    /// Drowned Tidecaller: the filler bolt.
    TideLash,
    /// Drowned Cultist: a bolt that leaves Waterlogged on somebody.
    BrineBolt,
    /// Catacomb Lurker: a wide frontal cone.
    TidalCleave,
    /// Catacomb Lurker: the heavy filler.
    LurkerClaw,
    /// Bilge Archer: marked circles around several players that land after a delay.
    BilgeVolley,
    /// The Drowned Abbess: the crozier filler.
    AbbessCrozier,
    /// The Drowned Abbess: a circle around herself.
    TidalSlam,
    /// The Drowned Abbess: a channel that drags the party in while pools open around her.
    Undertow,
    /// The Drowned Abbess: a long cast that heals her and spurs her allies if it completes.
    DrowningHymn,

    // ---------------------------------------------------------------- the Bone Foundry
    /// Slag Ironclad: wards every ally around it against a share of all damage.
    SlagBulwark,
    /// Slag Ironclad: a wide frontal cone.
    ShieldSweep,
    /// Slag Ironclad: the heavy filler.
    SlagBash,
    /// Cinder Bomber: a fuse, a circle and a death.
    CinderBlast,
    /// Slag Thrower: marked circles on two players that leave burning slag.
    SlagLob,
    /// Slag Thrower: the filler bolt.
    EmberFlick,
    /// Foundry Foreman: stokes every ally around it to hit harder.
    StokeTheFurnace,
    /// Foundry Foreman: a line through its victim.
    ChainLash,
    /// Foundry Foreman: the filler.
    ForemansClub,
    /// The Foundry Overseer: a circle around himself.
    FoundrySlam,
    /// The Foundry Overseer: marks on three players that chain through anyone caught.
    SlagBrand,
    /// The Foundry Overseer: the hammer filler.
    OverseerHammer,
    /// The Foundry Overseer: a shared-damage circle on one player's ground.
    MoltenLadle,
    /// The Foundry Overseer: a conveyor lane that carries the party into slag.
    ConveyorSurge,
    /// The Foundry Overseer, red-hot: attacks much faster until he dies.
    Overheat,
}

/// How many enemy abilities exist.
pub const ENEMY_ABILITY_COUNT: usize = 83;

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
        Self::BoneBolt,
        Self::BoneBarrage,
        Self::GraveExplosion,
        Self::BoneSpear,
        Self::ShadowBolt,
        Self::CorruptedGround,
        Self::DarkPulse,
        Self::GraveDrain,
        Self::BoneArrow,
        Self::MultiShot,
        Self::CripplingArrow,
        Self::RainOfArrows,
        Self::BoneSlash,
        Self::BoneCleave,
        Self::BoneCharge,
        Self::ExecutionStrike,
        Self::CrushingBlow,
        Self::GroundSlam,
        Self::GraveCharge,
        Self::BoneQuake,
        Self::GhoulStrike,
        Self::RendingClaws,
        Self::Pounce,
        Self::BloodFrenzy,
        Self::MassiveBoneBarrage,
        Self::GraveChainExplosion,
        Self::SpreadingCorruption,
        Self::MassGraveDrain,
        Self::DeadlyRain,
        Self::BoneVolley,
        Self::Massacre,
        Self::ShatteringCharge,
        Self::Gravequake,
        Self::DevastatingCharge,
        Self::RendingFrenzy,
        Self::FeastOnTheWounded,
        Self::SavagePounce,
        Self::RapierThrust,
        Self::GougingRiposte,
        Self::Garrote,
        Self::DanceOfBlades,
        Self::GhastlyWaltz,
        Self::MarkOfTheHost,
        Self::CrimsonFrenzy,
        Self::TidalMend,
        Self::TideLash,
        Self::BrineBolt,
        Self::TidalCleave,
        Self::LurkerClaw,
        Self::BilgeVolley,
        Self::AbbessCrozier,
        Self::TidalSlam,
        Self::Undertow,
        Self::DrowningHymn,
        Self::SlagBulwark,
        Self::ShieldSweep,
        Self::SlagBash,
        Self::CinderBlast,
        Self::SlagLob,
        Self::EmberFlick,
        Self::StokeTheFurnace,
        Self::ChainLash,
        Self::ForemansClub,
        Self::FoundrySlam,
        Self::SlagBrand,
        Self::OverseerHammer,
        Self::MoltenLadle,
        Self::ConveyorSurge,
        Self::Overheat,
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
    /// One sentence on what it does, for the dungeon map's sidebar.
    ///
    /// Words, not numbers: the sidebar prints the cast, cooldown, range and damage from the
    /// columns below, so a description never goes stale when a row is retuned.
    pub description: &'static str,
    /// What the party does about it, in a few words: `"interrupt - the ticks grow"`.
    ///
    /// The "answer" column of the dungeon's design tables. Empty where the design says there
    /// is nothing to do but take it.
    pub counter: &'static str,
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
    /// The closest a recipient may be for this to be chosen, in metres. Zero is no minimum.
    ///
    /// What makes a charge a charge: it is used on somebody *distant*, never as a second
    /// swing at whoever is already in the monster's face.
    pub min_range: f32,
    /// How many different hostiles it is aimed at. One is the recipient alone.
    ///
    /// The recipient first, then the nearest others in range. What turns one arrow into
    /// Multi-Shot and one drain into Mass Grave Drain without a line of code of their own.
    pub targets: u8,
    /// How many impact zones land around each target. One is the target's own.
    pub zones: u8,
    /// How far the extra zones scatter from their target, in metres.
    ///
    /// Deterministic: the zones are laid on a ring at fixed angles, turned by the simulation's
    /// seeded generator, so a barrage is never the same twice in a run and always the same in
    /// a replay.
    pub spread: f32,
    /// Seconds between the cast finishing and the zones landing. Zero lands with the cast.
    ///
    /// A delayed zone is drawn from the moment the cast finishes and resolves on its own
    /// clock, so the caster is free to do something else while the party moves — which is
    /// what makes Rain of Arrows and a Bone Quake a *second* thing to watch rather than a
    /// cast bar.
    pub delay: f32,
    /// Whether a delayed zone follows the target it was placed on.
    ///
    /// A mark: Grave Explosion goes where its victim goes, so the answer is to take it away
    /// from everybody else rather than to step out of it.
    pub follow: bool,
    /// Whether a detonation marks every other hostile it catches with a delayed zone of its
    /// own.
    ///
    /// What punishes stacking twice over: once for the splash, and once more for the marks
    /// the splash leaves. A chained mark does not chain again.
    pub chain: bool,
    /// What a hit leaves on its victim besides the damage.
    pub rider: EnemyRider,
    /// A condition it needs before it is considered.
    pub requires: EnemyRequirement,
    /// What its gathering sounds and looks like.
    pub cue: EnemyCue,
    /// Whether it is used at most once in each phase that lists it.
    ///
    /// Starting it locks it for the rest of the phase (an interrupt does not give it back),
    /// and entering a phase that lists it makes it ready again. How a boss casts something
    /// "at sixty-six and at thirty-three per cent" with no hand-written trigger.
    pub once_per_phase: bool,
}

impl EnemyAbilityDef {
    /// Every field at the value nearly every attack wants, for the table rows to override.
    ///
    /// The table is written as `EnemyAbilityDef { name, range, …, ..EnemyAbilityDef::BASE }`,
    /// so a row says what is particular about the attack and nothing else — a new column is
    /// then a default here rather than an edit to fifty rows.
    pub const BASE: Self = Self {
        name: "",
        description: "",
        counter: "",
        range: 3.0,
        cast_time: 0.0,
        cooldown: 2.0,
        damage: 0.0,
        damage_type: DamageType::Physical,
        shape: TelegraphShape::Single,
        aim: EnemyAim::Target,
        select: EnemyTarget::Victim,
        effect: EnemyEffect::Damage,
        interruptible: false,
        recovery: 0.4,
        min_range: 0.0,
        targets: 1,
        zones: 1,
        spread: 0.0,
        delay: 0.0,
        follow: false,
        chain: false,
        rider: EnemyRider::None,
        requires: EnemyRequirement::None,
        cue: EnemyCue::Quick,
        once_per_phase: false,
    };

    /// Whether this attack has a cast bar at all.
    #[must_use]
    pub const fn is_cast(&self) -> bool {
        self.cast_time > 0.0
    }

    /// How many zones it lays in total, across every target.
    #[must_use]
    pub fn zone_total(&self) -> usize {
        usize::from(self.targets.max(1)) * usize::from(self.zones.max(1))
    }

    /// Everything one uninterrupted use of it deals to one target standing in every zone.
    ///
    /// What the balance tests add up: a channel's growing ticks summed, a charge's landing
    /// and its shockwave, a bleed's full stack run. Hazards and trails are left out — they
    /// are what the player is being asked to leave.
    #[must_use]
    pub fn damage_per_use(&self) -> f32 {
        let hit = match self.effect {
            EnemyEffect::Channel { interval, ramp } => {
                let ticks = channel_ticks(self.cast_time, interval);
                (0..ticks)
                    .map(|index| self.damage * (1.0 + ramp * index as f32))
                    .sum()
            }
            EnemyEffect::Charge {
                shockwave_radius, ..
            } if shockwave_radius > 0.0 => self.damage * (1.0 + SHOCKWAVE_SHARE),
            EnemyEffect::Heal { .. }
            | EnemyEffect::Frenzy { .. }
            | EnemyEffect::Hymn { .. }
            | EnemyEffect::Bestow { .. } => 0.0,
            _ => self.damage,
        };
        let bleed = match self.rider {
            EnemyRider::Bleed {
                damage_per_stack,
                duration,
                stacks,
                ..
            } => damage_per_stack * f32::from(stacks) * duration,
            _ => 0.0,
        };
        hit + bleed
    }
}

/// How many ticks a channel of `duration` seconds at `interval` makes.
///
/// Rounded rather than floored: a four-second channel ticking every half second is eight
/// ticks, and float residue must not make it seven.
#[must_use]
pub fn channel_ticks(duration: f32, interval: f32) -> usize {
    if interval <= 0.0 || duration <= 0.0 {
        return 0;
    }
    ((duration / interval) + 1e-3).floor().max(1.0) as usize
}

/// A shockwave hits for this share of its charge's own damage.
///
/// A second, smaller hit: the landing is the big one, the ring is what catches a player who
/// stood still after dodging it.
pub const SHOCKWAVE_SHARE: f32 = 0.7;

/// Seconds between a charge landing and its shockwave.
pub const SHOCKWAVE_DELAY: f32 = 0.9;

/// Seconds a chained mark gives its new victim.
///
/// Shorter than the mark that made it: a chain is the punishment for standing together, and
/// the fix — spreading out — is the same fix the first mark already asked for.
pub const CHAIN_DELAY: f32 = 2.0;

/// Metres between the pools a charge's trail leaves.
pub const TRAIL_SPACING: f32 = 3.0;

/// The radius of each of a trail's pools, in metres.
pub const TRAIL_RADIUS: f32 = 2.0;

/// A wide cone's half-angle: sixty degrees, which is a hundred and twenty of spread.
///
/// The Skeleton Warrior's cleave and everything that escalates it. Twice the ordinary cone,
/// because "stood in front of the Skeleton Warrior" should be a mistake a healer standing off
/// to one side cannot make by accident, and one a healer standing behind the tank can.
pub const WIDE_CONE_HALF_ANGLE: f32 = std::f32::consts::FRAC_PI_3;

/// The attack table.
///
/// Numbers are tuned against the player's 500 health and the class's own cast times: the
/// caster's blast is a fifth of the player's health and takes two seconds, so standing in
/// four of them is death and walking out of them is the whole lesson.
pub static ENEMY_ABILITIES: [EnemyAbilityDef; ENEMY_ABILITY_COUNT] = [
    EnemyAbilityDef {
        name: "SWING",
        description: "The melee enemy's filler: instant, short range, single target.",
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "CLEAVE",
        description: "The melee enemy's telegraphed cone. Slow enough to walk out of.",
        cue: EnemyCue::Sweep,
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "GROUND BLAST",
        description: "The caster's telegraphed ground blast.",
        cue: EnemyCue::Heavy,
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "BOLT",
        description: "The caster's filler: a small bolt with a short cast and no area.",
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
        ..EnemyAbilityDef::BASE
    },
    // Three seconds and 85 damage in a 6.5 m circle: the largest thing in the trash roster,
    // and the one the player's interrupt exists to answer.
    EnemyAbilityDef {
        name: "RITUAL NOVA",
        description: "The Bone Ritualist's three-second nova. The interrupt lesson.",
        cue: EnemyCue::Heavy,
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
        ..EnemyAbilityDef::BASE
    },
    // Nearly half a Brute, on a nine-second cooldown. Not chosen at all unless something is
    // actually wounded, which is what makes an untouched pack's Warden open with Bolts.
    EnemyAbilityDef {
        name: "MEND",
        description: "The Crypt Warden's heal, used on whichever ally is worst off.",
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
        ..EnemyAbilityDef::BASE
    },
    // Small on impact, expensive to stand in: the pool is the mechanic, not the landing.
    EnemyAbilityDef {
        name: "CINDER POOL",
        description: "The Cinder Weaver's burning ground.",
        cue: EnemyCue::Sweep,
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
            grow_to: 0.0,
            radius: 0.0,
        },
        interruptible: true,
        recovery: 0.6,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "SUNDER",
        description: "The Ash Revenant's melee filler. A heavier Swing.",
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
        ..EnemyAbilityDef::BASE
    },
    // Centred on the caster and impossible to interrupt: the answer is distance, and 1.8 s
    // at the player's 7 m/s covers more than twice the radius.
    EnemyAbilityDef {
        name: "ASH BURST",
        description: "The Ash Revenant's uninterruptible blast, centred on itself.",
        cue: EnemyCue::Sweep,
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "WARDENS SWEEP",
        description: "The boss's wide cone. Uninterruptible: it is walked out of, not stopped.",
        cue: EnemyCue::Sweep,
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "EMBER LANCE",
        description: "The boss's ranged filler. What reaches a player who has run.",
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "ASHFALL",
        description: "The boss's interruptible circle. The fight's one interrupt window.",
        cue: EnemyCue::Heavy,
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
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "CINDER RAIN",
        description: "The boss's burning ground, from phase two onwards.",
        cue: EnemyCue::Sweep,
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
            grow_to: 0.0,
            radius: 0.0,
        },
        interruptible: false,
        recovery: 0.8,
        ..EnemyAbilityDef::BASE
    },
    // The largest telegraph in the game. Its range is deliberately longer than the arena is
    // wide, so it is never withheld for being "out of range" — the escape is the counterplay.
    EnemyAbilityDef {
        name: "IMMOLATION NOVA",
        description: "The boss's phase-three climax: a fourteen-metre circle centred on itself.",
        cue: EnemyCue::Heavy,
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
        ..EnemyAbilityDef::BASE
    },
    // ======================================================================================
    // The Graveyard Crypt. Tuned for a party of three — a Warrior tank, a damage dealer and a
    // Druid healer, each with 500 health — at level one. See `docs/dungeon/graveyard/`.
    //
    // The rule every number below follows: an avoidable hit is survivable *once* at full
    // health (under a third of 500) and lethal if it is ignored twice; the unavoidable damage
    // a pack puts on the tank stays under what one healer restores.
    // ======================================================================================

    // ---------------------------------------------------------------- Skeleton Bonecaster
    EnemyAbilityDef {
        name: "BONE BOLT",
        description: "The filler bolt.",
        range: 30.0,
        cast_time: 1.3,
        cooldown: 2.2,
        damage: 34.0,
        damage_type: DamageType::Arcane,
        interruptible: true,
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    // Four impact zones around the target: the one under them, and three a step away. A
    // player who walks two metres in any direction is hit by at most one.
    EnemyAbilityDef {
        name: "BONE BARRAGE",
        description: "A scatter of impact zones around a target.",
        counter: "Step out of the pattern",
        range: 30.0,
        cast_time: 1.4,
        cooldown: 11.0,
        damage: 55.0,
        shape: TelegraphShape::Circle { radius: 2.5 },
        zones: 4,
        spread: 4.5,
        delay: 1.6,
        interruptible: true,
        recovery: 0.6,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // The anti-stacking mark: it follows its victim, so the answer is to take it away from
    // everyone else. Ninety-five on the victim and on everybody standing with them.
    EnemyAbilityDef {
        name: "GRAVE EXPLOSION",
        description: "A mark that follows its victim and detonates around them.",
        counter: "Move away from the party with it - or kill the caster (its marks vanish)",
        range: 30.0,
        cast_time: 1.0,
        cooldown: 16.0,
        damage: 95.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Circle { radius: 5.0 },
        select: EnemyTarget::Random,
        delay: 4.0,
        follow: true,
        interruptible: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Uninterruptible and slow, because the counterplay is a sidestep: the line is drawn from
    // the caster through the target and a little past them.
    EnemyAbilityDef {
        name: "BONE SPEAR",
        description: "A long line through its target.",
        counter: "Sidestep",
        range: 30.0,
        cast_time: 2.0,
        cooldown: 13.0,
        damage: 110.0,
        shape: TelegraphShape::Line {
            yaw: 0.0,
            length: 34.0,
            width: 2.6,
        },
        aim: EnemyAim::Caster,
        recovery: 0.8,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Grave Acolyte
    EnemyAbilityDef {
        name: "SHADOW BOLT",
        description: "The filler bolt.",
        range: 30.0,
        cast_time: 1.5,
        cooldown: 2.4,
        damage: 38.0,
        damage_type: DamageType::Arcane,
        interruptible: true,
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "CORRUPTED GROUND",
        description: "A pool of cursed ground under a player.",
        counter: "Move out",
        range: 28.0,
        cast_time: 1.2,
        cooldown: 12.0,
        damage: 20.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Circle { radius: 4.0 },
        effect: EnemyEffect::Hazard {
            duration: 9.0,
            damage_per_second: 26.0,
            grow_to: 0.0,
            radius: 0.0,
        },
        interruptible: true,
        recovery: 0.6,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // Four pulses of 28 on a fixed circle: the ring stays where the target *was*, so the
    // party can leave it, and an interrupt ends it outright.
    EnemyAbilityDef {
        name: "DARK PULSE",
        description: "An interruptible channel pulsing a circle.",
        counter: "Leave it, or kick",
        range: 28.0,
        cast_time: 3.0,
        cooldown: 15.0,
        damage: 28.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Circle { radius: 5.5 },
        effect: EnemyEffect::Channel {
            interval: 0.75,
            ramp: 0.0,
        },
        interruptible: true,
        recovery: 0.8,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Eight ticks growing from 10 to 34.5: 178 if nobody interrupts it, which is over a
    // third of a player. The Acolyte is a kill-or-kick target because of this row.
    EnemyAbilityDef {
        name: "GRAVE DRAIN",
        description: "An interruptible channel whose ticks grow.",
        counter: "Interrupt - the ticks grow",
        range: 30.0,
        cast_time: 4.0,
        cooldown: 18.0,
        damage: 10.0,
        damage_type: DamageType::Arcane,
        effect: EnemyEffect::Channel {
            interval: 0.5,
            ramp: 0.35,
        },
        interruptible: true,
        recovery: 0.8,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Crypt Archer
    EnemyAbilityDef {
        name: "BONE ARROW",
        description: "The filler shot.",
        range: 38.0,
        cast_time: 1.1,
        cooldown: 2.0,
        damage: 40.0,
        interruptible: true,
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    // One arrow at each of up to three players. Not avoidable; it is the Archer's damage
    // output spread across the party, which is what makes the healer want it dead.
    EnemyAbilityDef {
        name: "MULTI-SHOT",
        description: "One arrow at each of several players.",
        counter: "Heal it; kill the archer",
        range: 38.0,
        cast_time: 1.2,
        cooldown: 9.0,
        damage: 45.0,
        targets: 3,
        recovery: 0.5,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "CRIPPLING ARROW",
        description: "A shot that slows.",
        counter: "Dispel (curse) or expect the rain",
        range: 38.0,
        cast_time: 0.8,
        cooldown: 10.0,
        damage: 30.0,
        select: EnemyTarget::Random,
        rider: EnemyRider::Slow {
            fraction: 0.45,
            duration: 5.0,
        },
        interruptible: true,
        recovery: 0.4,
        ..EnemyAbilityDef::BASE
    },
    // Two circles per player, drawn for 2.2 s. A slowed player is the one this finds.
    EnemyAbilityDef {
        name: "RAIN OF ARROWS",
        description: "Marked circles that land after a delay.",
        counter: "Move",
        range: 38.0,
        cast_time: 1.0,
        cooldown: 14.0,
        damage: 80.0,
        shape: TelegraphShape::Circle { radius: 3.5 },
        targets: 3,
        zones: 2,
        spread: 5.0,
        delay: 2.2,
        interruptible: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Skeleton Warrior
    EnemyAbilityDef {
        name: "BONE SLASH",
        description: "The filler swing.",
        range: 3.2,
        cooldown: 1.6,
        damage: 30.0,
        recovery: 0.4,
        ..EnemyAbilityDef::BASE
    },
    // A hundred and twenty degrees in front of it. The tank is always in it; nobody else
    // needs to be.
    EnemyAbilityDef {
        name: "BONE CLEAVE",
        description: "A wide frontal cone.",
        counter: "Tank faces it away; nobody else in front",
        range: 5.0,
        cast_time: 1.3,
        cooldown: 8.0,
        damage: 70.0,
        shape: TelegraphShape::Cone {
            yaw: 0.0,
            range: 7.0,
            half_angle: WIDE_CONE_HALF_ANGLE,
        },
        aim: EnemyAim::Caster,
        recovery: 0.8,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "BONE CHARGE",
        description: "A charge at whoever stands furthest back.",
        counter: "Be ready for it",
        range: 25.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 14.0,
        damage: 55.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Farthest,
        effect: EnemyEffect::Charge {
            speed: 22.0,
            impact_radius: 0.0,
            shockwave_radius: 0.0,
            trail_damage_per_second: 0.0,
            trail_duration: 0.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 0.8,
        min_range: 7.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // Two seconds of wind-up and a kick stops it. The biggest single hit a trash mob makes.
    EnemyAbilityDef {
        name: "EXECUTION STRIKE",
        description: "A slow, heavy, interruptible strike.",
        counter: "Interrupt, or a defensive",
        range: 3.5,
        cast_time: 2.0,
        cooldown: 15.0,
        damage: 130.0,
        interruptible: true,
        recovery: 1.0,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Grave Brute
    EnemyAbilityDef {
        name: "CRUSHING BLOW",
        description: "The heavy filler.",
        range: 3.5,
        cooldown: 3.2,
        damage: 62.0,
        recovery: 0.7,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "GROUND SLAM",
        description: "A circle around itself.",
        counter: "Step out",
        range: 6.0,
        cast_time: 1.5,
        cooldown: 10.0,
        damage: 60.0,
        shape: TelegraphShape::Circle { radius: 7.0 },
        aim: EnemyAim::Caster,
        recovery: 0.9,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "GRAVE CHARGE",
        description: "A charge at a distant player.",
        counter: "Heal the one it is cast on",
        range: 30.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 16.0,
        damage: 80.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Farthest,
        effect: EnemyEffect::Charge {
            speed: 20.0,
            impact_radius: 0.0,
            shockwave_radius: 0.0,
            trail_damage_per_second: 0.0,
            trail_duration: 0.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 1.0,
        min_range: 8.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // A short cast, then a ring around where the Brute stood that lands 2.2 s later: the
    // melee in its face are the ones who have to leave.
    EnemyAbilityDef {
        name: "BONE QUAKE",
        description: "A delayed ring around itself.",
        counter: "Melee get out",
        range: 6.0,
        cast_time: 0.6,
        cooldown: 18.0,
        damage: 90.0,
        shape: TelegraphShape::Circle { radius: 9.0 },
        aim: EnemyAim::Caster,
        delay: 2.2,
        recovery: 0.6,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Crypt Ghoul
    EnemyAbilityDef {
        name: "GHOUL STRIKE",
        description: "The fast filler.",
        range: 3.0,
        cooldown: 1.0,
        damage: 16.0,
        recovery: 0.25,
        ..EnemyAbilityDef::BASE
    },
    // Six a second per stack, eight stacks: a bleed the healer can ignore at two and cannot
    // at six.
    EnemyAbilityDef {
        name: "RENDING CLAWS",
        description: "A hit that leaves a stacking bleed.",
        counter: "Heal through / kill it",
        range: 3.0,
        cooldown: 5.0,
        damage: 14.0,
        rider: EnemyRider::Bleed {
            damage_per_stack: 6.0,
            duration: 8.0,
            stacks: 1,
            max_stacks: 8,
        },
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "POUNCE",
        description: "A leap at a random player, landing in a circle.",
        counter: "Spread",
        range: 22.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 14.0,
        damage: 50.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Random,
        effect: EnemyEffect::Charge {
            speed: 26.0,
            impact_radius: 4.0,
            shockwave_radius: 0.0,
            trail_damage_per_second: 0.0,
            trail_duration: 0.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 0.6,
        min_range: 6.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "BLOOD FRENZY",
        description: "Attacks much faster for a while.",
        counter: "Burst it now",
        range: 40.0,
        cooldown: 22.0,
        effect: EnemyEffect::Frenzy {
            duration: 8.0,
            haste: 0.6,
        },
        recovery: 0.2,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ======================================================================================
    // The empowered adds. Each keeps two of its abilities as the dungeon taught them and
    // escalates the other two *mechanically* — more zones, less warning, a second hit, a
    // second target — rather than by multiplying the number.
    // ======================================================================================
    // Seven zones instead of four, larger, landing in 1.2 s instead of 1.6.
    EnemyAbilityDef {
        name: "MASSIVE BONE BARRAGE",
        description: "More, larger, faster impact zones.",
        range: 30.0,
        cast_time: 1.2,
        cooldown: 9.0,
        damage: 70.0,
        shape: TelegraphShape::Circle { radius: 3.2 },
        zones: 7,
        spread: 7.0,
        delay: 1.2,
        interruptible: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Three marks at once, and every detonation marks whoever else it catches.
    EnemyAbilityDef {
        name: "GRAVE CHAIN EXPLOSION",
        description: "Several marks, and a detonation marks whoever it catches.",
        range: 30.0,
        cast_time: 1.0,
        cooldown: 15.0,
        damage: 100.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Circle { radius: 5.5 },
        select: EnemyTarget::Random,
        targets: 3,
        delay: 4.0,
        follow: true,
        chain: true,
        interruptible: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Lands at 3.5 m and spreads to 8 over its twelve seconds.
    EnemyAbilityDef {
        name: "SPREADING CORRUPTION",
        description: "A pool that spreads while it burns.",
        range: 28.0,
        cast_time: 1.0,
        cooldown: 11.0,
        damage: 25.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Circle { radius: 3.5 },
        effect: EnemyEffect::Hazard {
            duration: 12.0,
            damage_per_second: 30.0,
            grow_to: 8.0,
            radius: 0.0,
        },
        interruptible: true,
        recovery: 0.6,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // Grave Drain on three players at once: one interrupt stops all three, so the party has
    // to have one ready — and the empowered Acolyte casts it every 16 s.
    EnemyAbilityDef {
        name: "MASS GRAVE DRAIN",
        description: "Grave Drain on several players at once.",
        range: 30.0,
        cast_time: 4.0,
        cooldown: 16.0,
        damage: 9.0,
        damage_type: DamageType::Arcane,
        targets: 3,
        effect: EnemyEffect::Channel {
            interval: 0.5,
            ramp: 0.35,
        },
        interruptible: true,
        recovery: 0.8,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Three circles per player, 1.3 s of warning instead of 2.2.
    EnemyAbilityDef {
        name: "DEADLY RAIN",
        description: "More rain, sooner.",
        range: 38.0,
        cast_time: 1.0,
        cooldown: 11.0,
        damage: 85.0,
        shape: TelegraphShape::Circle { radius: 3.5 },
        targets: 3,
        zones: 3,
        spread: 6.0,
        delay: 1.3,
        interruptible: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // A line through each of three players that pierces everybody behind them, and cursed
    // ground where each one was aimed.
    EnemyAbilityDef {
        name: "BONE VOLLEY",
        description: "Lines through several players that leave cursed ground.",
        range: 38.0,
        cast_time: 1.4,
        cooldown: 12.0,
        damage: 70.0,
        shape: TelegraphShape::Line {
            yaw: 0.0,
            length: 38.0,
            width: 1.8,
        },
        aim: EnemyAim::Caster,
        targets: 3,
        effect: EnemyEffect::Hazard {
            duration: 7.0,
            damage_per_second: 22.0,
            grow_to: 0.0,
            radius: 3.0,
        },
        recovery: 0.6,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // A hundred and fifty degrees, nine metres, a hundred damage.
    EnemyAbilityDef {
        name: "MASSACRE",
        description: "A wider, harder cleave.",
        range: 6.0,
        cast_time: 1.3,
        cooldown: 8.0,
        damage: 100.0,
        shape: TelegraphShape::Cone {
            yaw: 0.0,
            range: 9.0,
            half_angle: WIDE_CONE_HALF_ANGLE * 1.25,
        },
        aim: EnemyAim::Caster,
        recovery: 0.8,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "SHATTERING CHARGE",
        description: "A charge whose landing sends out a shockwave.",
        range: 25.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 12.0,
        damage: 70.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Farthest,
        effect: EnemyEffect::Charge {
            speed: 22.0,
            impact_radius: 0.0,
            shockwave_radius: 7.0,
            trail_damage_per_second: 0.0,
            trail_duration: 0.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 1.0,
        min_range: 7.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // Six circles scattered round the Brute, the first under its feet.
    EnemyAbilityDef {
        name: "GRAVEQUAKE",
        description: "Impact zones scattered all around it.",
        range: 6.0,
        cast_time: 0.8,
        cooldown: 14.0,
        damage: 95.0,
        shape: TelegraphShape::Circle { radius: 4.0 },
        aim: EnemyAim::Caster,
        zones: 6,
        spread: 8.0,
        delay: 1.8,
        recovery: 0.6,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "DEVASTATING CHARGE",
        description: "A charge that leaves a burning trail.",
        range: 30.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 14.0,
        damage: 90.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Farthest,
        effect: EnemyEffect::Charge {
            speed: 20.0,
            impact_radius: 0.0,
            shockwave_radius: 0.0,
            trail_damage_per_second: 30.0,
            trail_duration: 6.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 1.0,
        min_range: 8.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // Two stacks a hit instead of one, nine a stack instead of six, and ten high.
    EnemyAbilityDef {
        name: "RENDING FRENZY",
        description: "A harder, taller bleed.",
        range: 3.0,
        cooldown: 4.0,
        damage: 18.0,
        rider: EnemyRider::Bleed {
            damage_per_stack: 9.0,
            duration: 10.0,
            stacks: 2,
            max_stacks: 10,
        },
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    // Not on its list at all until its target bleeds.
    EnemyAbilityDef {
        name: "FEAST ON THE WOUNDED",
        description: "Extra attacks, only against a bleeding target.",
        range: 3.0,
        cooldown: 2.5,
        damage: 24.0,
        requires: EnemyRequirement::VictimBleeding,
        recovery: 0.2,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "SAVAGE POUNCE",
        description: "A leap whose landing explodes, then explodes again.",
        range: 22.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 12.0,
        damage: 70.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Random,
        effect: EnemyEffect::Charge {
            speed: 26.0,
            impact_radius: 6.0,
            shockwave_radius: 8.0,
            trail_damage_per_second: 0.0,
            trail_duration: 0.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 0.8,
        min_range: 6.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // ======================================================================================
    // Lord Ashgrave, the Hollow Steward. Moroes-inspired: a gentleman's blade on the tank,
    // a garrote for whoever is not, a waltz through the room and a mark on somebody's back.
    // See `docs/dungeon/graveyard/boss_lord_ashgrave.md`.
    // ======================================================================================
    EnemyAbilityDef {
        name: "RAPIER THRUST",
        description: "The rapier filler.",
        range: 3.5,
        cooldown: 1.5,
        damage: 36.0,
        recovery: 0.35,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "GOUGING RIPOSTE",
        description: "A heavy riposte on the tank that slows.",
        counter: "A defensive; expect to be slow for the next Dance",
        range: 4.0,
        cast_time: 1.2,
        cooldown: 12.0,
        damage: 90.0,
        rider: EnemyRider::Slow {
            fraction: 0.5,
            duration: 4.0,
        },
        recovery: 0.8,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // Never on the tank if anybody else is there: the healer's problem for twenty seconds.
    EnemyAbilityDef {
        name: "GARROTE",
        description: "A long bleed on somebody who is not the tank.",
        counter: "The healer's long problem",
        range: 40.0,
        cast_time: 0.6,
        cooldown: 20.0,
        damage: 20.0,
        select: EnemyTarget::Random,
        rider: EnemyRider::Bleed {
            damage_per_stack: 8.0,
            duration: 20.0,
            stacks: 2,
            max_stacks: 6,
        },
        recovery: 0.4,
        cue: EnemyCue::Quick,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "DANCE OF BLADES",
        description: "A spinning circle around himself.",
        counter: "Everyone out, including the tank",
        range: 7.0,
        cast_time: 2.2,
        cooldown: 16.0,
        damage: 95.0,
        shape: TelegraphShape::Circle { radius: 8.0 },
        aim: EnemyAim::Caster,
        recovery: 1.0,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "GHASTLY WALTZ",
        description: "A charge through the room that leaves a trail.",
        counter: "Heal them; stay off the trail",
        range: 35.0,
        // A charge is instant: no wind-up, it simply goes.
        cast_time: 0.0,
        cooldown: 22.0,
        damage: 70.0,
        aim: EnemyAim::Caster,
        select: EnemyTarget::Farthest,
        effect: EnemyEffect::Charge {
            speed: 18.0,
            impact_radius: 0.0,
            shockwave_radius: 0.0,
            trail_damage_per_second: 25.0,
            trail_duration: 5.0,
            slow: 0.5,
            slow_duration: 1.0,
        },
        recovery: 1.0,
        min_range: 8.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "MARK OF THE HOST",
        description: "A mark that follows a player and detonates around them.",
        counter: "Take it away from the party",
        range: 40.0,
        cast_time: 1.0,
        cooldown: 24.0,
        damage: 110.0,
        damage_type: DamageType::Arcane,
        shape: TelegraphShape::Circle { radius: 6.0 },
        select: EnemyTarget::Random,
        delay: 5.0,
        follow: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "CRIMSON FRENZY",
        description: "Attacks much faster for a while.",
        range: 40.0,
        cooldown: 25.0,
        effect: EnemyEffect::Frenzy {
            duration: 10.0,
            haste: 0.5,
        },
        recovery: 0.2,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ======================================================================================
    // The Drowned Catacombs. Same party, same rule as the crypt: an avoidable hit is survivable
    // once at full health. This dungeon adds three questions the crypt does not ask — who do I
    // kick (a heal, a hymn), what do I dispel (Waterlogged), and which way do I run (a cleave
    // in a corridor, a pull towards the boss). See `docs/dungeon/DROWNED_CATACOMBS.md`.
    // ======================================================================================

    // ---------------------------------------------------------------- Drowned Tidecaller
    // Two hundred and eighty on the worst-off ally every six seconds: more than the pack's
    // damage can outrun, and nothing at all once it is kicked. Not chosen unless somebody is
    // wounded, so an untouched pack opens with its lash.
    EnemyAbilityDef {
        name: "TIDAL MEND",
        description: "A heal on the most wounded ally in sight.",
        counter: "Interrupt it, or kill the tidecaller first",
        range: 22.0,
        cast_time: 2.2,
        cooldown: 6.0,
        damage_type: DamageType::Arcane,
        select: EnemyTarget::WoundedAlly,
        effect: EnemyEffect::Heal { amount: 280.0 },
        interruptible: true,
        recovery: 0.8,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "TIDE LASH",
        description: "The filler bolt.",
        range: 28.0,
        cast_time: 1.4,
        cooldown: 2.6,
        damage: 30.0,
        damage_type: DamageType::Arcane,
        interruptible: true,
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Drowned Cultist
    // Aimed past the tank on purpose: the slow and the extra damage land on whoever is
    // standing furthest from the fight, which is usually the one who can least afford them.
    // Three stacks is forty-five per cent slower — slower than the Abbess's current.
    EnemyAbilityDef {
        name: "BRINE BOLT",
        description: "A bolt of brine that leaves Waterlogged on a random player: slower, and every blow lands harder.",
        counter: "Interrupt it; dispel Waterlogged",
        range: 30.0,
        cast_time: 1.6,
        cooldown: 6.0,
        damage: 36.0,
        damage_type: DamageType::Arcane,
        select: EnemyTarget::Random,
        rider: EnemyRider::Afflict {
            kind: StatusKind::Waterlogged,
            duration: 12.0,
            max_stacks: 3,
        },
        interruptible: true,
        recovery: 0.4,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Catacomb Lurker
    // A hundred and twenty degrees and nine metres, drawn for the whole cast. The tank turns it
    // away from the party; in a corridor there is nowhere to stand but the cone or behind it.
    EnemyAbilityDef {
        name: "TIDAL CLEAVE",
        description: "A wide frontal cone, drawn on the floor for the whole cast.",
        counter: "Tank faces it away from the party; step out of the cone",
        range: 5.5,
        cast_time: 1.6,
        cooldown: 8.0,
        damage: 95.0,
        shape: TelegraphShape::Cone {
            yaw: 0.0,
            range: 9.0,
            half_angle: WIDE_CONE_HALF_ANGLE,
        },
        aim: EnemyAim::Caster,
        recovery: 1.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "LURKER CLAW",
        description: "The heavy filler.",
        range: 3.5,
        cooldown: 2.0,
        damage: 42.0,
        recovery: 0.5,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Bilge Archer
    // Two circles on each of three players, drawn for almost two seconds from the end of the
    // cast. A kick stops the whole volley; walking out of the circles stops each of them.
    EnemyAbilityDef {
        name: "BILGE VOLLEY",
        description: "Marked circles around several players that land after a delay.",
        counter: "Move out of the circles, or interrupt the cast",
        range: 38.0,
        cast_time: 1.2,
        cooldown: 11.0,
        damage: 75.0,
        shape: TelegraphShape::Circle { radius: 3.2 },
        targets: 3,
        zones: 2,
        spread: 5.0,
        delay: 1.9,
        interruptible: true,
        recovery: 0.5,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- The Drowned Abbess
    // Her crozier on the tank, between the bigger things.
    EnemyAbilityDef {
        name: "ABBESS CROZIER",
        description: "The crozier filler.",
        range: 4.0,
        cooldown: 1.8,
        damage: 55.0,
        recovery: 0.4,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "TIDAL SLAM",
        description: "A circle around the Abbess.",
        counter: "Everyone out, including the tank",
        range: 7.0,
        cast_time: 1.8,
        cooldown: 11.0,
        damage: 85.0,
        shape: TelegraphShape::Circle { radius: 8.0 },
        aim: EnemyAim::Caster,
        recovery: 1.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // The pull is 4.6 m/s against a player's 7: walking away wins by a little, being slowed
    // (Waterlogged, three stacks) loses by a little, and standing still to cast loses outright.
    // Whoever is dragged to her feet is caught by the slam when it ends.
    EnemyAbilityDef {
        name: "UNDERTOW",
        description: "Drags every player towards the Abbess while drowning pools open around her, then slams the ground at her feet.",
        counter: "Walk out against the pull; stay out of the pools and the circle at her feet",
        range: 40.0,
        cast_time: 7.0,
        cooldown: 26.0,
        damage: 110.0,
        shape: TelegraphShape::Circle { radius: 7.0 },
        aim: EnemyAim::Caster,
        effect: EnemyEffect::Undertow {
            interval: 1.2,
            pull_speed: 4.6,
            pull_range: 40.0,
            pool_radius: 3.4,
            pool_damage_per_second: 38.0,
            pool_duration: 9.0,
            pool_near: 3.5,
            pool_far: 9.5,
        },
        recovery: 1.2,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Six seconds: long enough that someone has to be told to kick it, short enough that a
    // second kick is not needed. Once a phase, and an interrupt does not give it back.
    EnemyAbilityDef {
        name: "DROWNING HYMN",
        description: "A long hymn that heals the Abbess and makes every ally attack faster, if it is allowed to finish.",
        counter: "Interrupt it - and kill the tidecallers before the next one",
        range: 40.0,
        cast_time: 6.0,
        cooldown: 20.0,
        damage_type: DamageType::Arcane,
        effect: EnemyEffect::Hymn {
            heal_fraction: 0.10,
            haste: 0.6,
            duration: 14.0,
        },
        interruptible: true,
        recovery: 1.5,
        once_per_phase: true,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ======================================================================================
    // The Bone Foundry. The questions this dungeon adds: who do I kick (a ward, a rally), what
    // do I kill before it arrives (a fused bomber), where do I stand (a shared soak, a lane
    // that carries). See `docs/dungeon/BONE_FOUNDRY.md`.
    // ======================================================================================

    // ---------------------------------------------------------------- Slag Ironclad
    // Forty per cent off everything its pack takes, for eight seconds in every sixteen, if
    // the cast is allowed to finish: a kick is worth more here than a heal's worth of damage.
    EnemyAbilityDef {
        name: "SLAG BULWARK",
        description: "Wards every ally around it, itself included, against a share of all damage.",
        counter: "Interrupt it; the ward fades after a few seconds",
        range: 12.0,
        cast_time: 1.8,
        cooldown: 16.0,
        damage_type: DamageType::Fire,
        effect: EnemyEffect::Bestow {
            kind: StatusKind::Warded,
            radius: 14.0,
            duration: 8.0,
        },
        interruptible: true,
        recovery: 0.8,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "SHIELD SWEEP",
        description: "A wide frontal cone, drawn on the floor for the whole cast.",
        counter: "Tank faces it away from the party; step out of the cone",
        range: 5.0,
        cast_time: 1.5,
        cooldown: 9.0,
        damage: 70.0,
        shape: TelegraphShape::Cone {
            yaw: 0.0,
            range: 6.5,
            half_angle: WIDE_CONE_HALF_ANGLE,
        },
        aim: EnemyAim::Caster,
        recovery: 0.9,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "SLAG BASH",
        description: "The heavy filler.",
        range: 3.5,
        cooldown: 2.6,
        damage: 46.0,
        recovery: 0.6,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Cinder Bomber
    // A fuse of a second and a half, drawn as the circle it will burn. It dies when it goes off,
    // so there is no second one to dodge; it can be killed before then, and walked away from.
    EnemyAbilityDef {
        name: "CINDER BLAST",
        description: "A fuse, a circle around itself, and then the bomber is gone.",
        counter: "Kill it before the fuse ends, or run out of the circle",
        range: 3.0,
        cast_time: 1.5,
        cooldown: 5.0,
        damage: 120.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 4.5 },
        aim: EnemyAim::Caster,
        effect: EnemyEffect::Detonate,
        recovery: 0.2,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Slag Thrower
    // Two players, one circle each, drawn for 1.7 s after the cast and then left as ground that
    // burns for ten: the landing is dodged by moving, the pool by not standing back in it.
    EnemyAbilityDef {
        name: "SLAG LOB",
        description: "Marked circles under two players that land after a delay and leave burning slag.",
        counter: "Move out of the circle, then stay out of the pool; interrupt the cast",
        range: 32.0,
        cast_time: 1.6,
        cooldown: 9.0,
        damage: 45.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 3.2 },
        select: EnemyTarget::Random,
        targets: 2,
        delay: 1.7,
        effect: EnemyEffect::Hazard {
            duration: 10.0,
            damage_per_second: 38.0,
            grow_to: 0.0,
            radius: 0.0,
        },
        interruptible: true,
        recovery: 0.6,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "EMBER FLICK",
        description: "The filler bolt.",
        range: 28.0,
        cast_time: 1.2,
        cooldown: 2.4,
        damage: 26.0,
        damage_type: DamageType::Fire,
        interruptible: true,
        recovery: 0.3,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- Foundry Foreman
    // Thirty per cent more of everything its pack deals, for ten seconds in every eighteen. The
    // cast is long enough to be seen and short enough that the foreman has to be reached first.
    EnemyAbilityDef {
        name: "STOKE THE FURNACE",
        description: "Rallies every ally around it, itself included, to hit harder.",
        counter: "Interrupt it, or kill the foreman first",
        range: 20.0,
        cast_time: 2.2,
        cooldown: 18.0,
        damage_type: DamageType::Fire,
        effect: EnemyEffect::Bestow {
            kind: StatusKind::Stoked,
            radius: 16.0,
            duration: 10.0,
        },
        interruptible: true,
        recovery: 0.8,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "CHAIN LASH",
        description: "A line through its victim, drawn on the floor for the whole cast.",
        counter: "Step out of the line",
        range: 14.0,
        cast_time: 1.4,
        cooldown: 7.0,
        damage: 80.0,
        shape: TelegraphShape::Line {
            yaw: 0.0,
            length: 14.0,
            width: 2.4,
        },
        aim: EnemyAim::Caster,
        recovery: 0.8,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "FOREMANS CLUB",
        description: "The foreman's filler.",
        range: 3.5,
        cooldown: 2.2,
        damage: 52.0,
        recovery: 0.6,
        ..EnemyAbilityDef::BASE
    },
    // ---------------------------------------------------------------- The Foundry Overseer
    EnemyAbilityDef {
        name: "FOUNDRY SLAM",
        description: "A circle around the Overseer.",
        counter: "Everyone out, including the tank",
        range: 8.0,
        cast_time: 1.8,
        cooldown: 12.0,
        damage: 90.0,
        shape: TelegraphShape::Circle { radius: 7.5 },
        aim: EnemyAim::Caster,
        recovery: 1.0,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // The spread: three marks that follow their players, and a detonation marks whoever else
    // it catches, so one clump is two hits and then a second round of them.
    EnemyAbilityDef {
        name: "SLAG BRAND",
        description: "Marks on three players that follow them; a detonation marks whoever it catches.",
        counter: "Spread out: stay out of each other's circles",
        range: 36.0,
        cast_time: 1.2,
        cooldown: 16.0,
        damage: 85.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 4.5 },
        select: EnemyTarget::Random,
        targets: 3,
        delay: 4.0,
        follow: true,
        chain: true,
        recovery: 0.6,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    EnemyAbilityDef {
        name: "OVERSEER HAMMER",
        description: "The hammer filler.",
        range: 4.0,
        cooldown: 1.8,
        damage: 60.0,
        recovery: 0.4,
        ..EnemyAbilityDef::BASE
    },
    // The soak: a third of a thousand damage in one circle on somebody's feet, split between
    // whoever stands in it. Alone it hurts; together it is a bruise; unattended it spills over
    // the whole party. The opposite of the brand, and the two share a phase on purpose.
    EnemyAbilityDef {
        name: "MOLTEN LADLE",
        description: "A big circle on one player's ground; its damage is shared between everyone standing in it.",
        counter: "Stack in the circle together; nobody in it means the whole party is burned",
        range: 40.0,
        cast_time: 1.5,
        cooldown: 22.0,
        damage: 330.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Circle { radius: 5.5 },
        select: EnemyTarget::Random,
        delay: 5.5,
        effect: EnemyEffect::Soak { overflow: 0.6 },
        recovery: 0.8,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
    },
    // Five seconds of lane. The push is 3.4 m/s against a player's 7: stepping aside takes a
    // second and is always enough; riding it carries you sixteen metres, into the slag that
    // opens at the far end, and then the lane's own hit.
    EnemyAbilityDef {
        name: "CONVEYOR SURGE",
        description: "A lane across the foundry floor carries everyone in it away from the Overseer into pools of slag.",
        counter: "Step out of the lane sideways; never ride it to the end",
        range: 40.0,
        cast_time: 5.0,
        cooldown: 26.0,
        damage: 120.0,
        damage_type: DamageType::Fire,
        shape: TelegraphShape::Line {
            yaw: 0.0,
            length: 34.0,
            width: 10.0,
        },
        aim: EnemyAim::Caster,
        select: EnemyTarget::Random,
        effect: EnemyEffect::Conveyor {
            push_speed: 3.4,
            interval: 0.9,
            pool_radius: 3.0,
            pool_damage_per_second: 40.0,
            pool_duration: 8.0,
            pool_near: 20.0,
            pool_far: 33.0,
        },
        recovery: 1.2,
        cue: EnemyCue::Sweep,
        ..EnemyAbilityDef::BASE
    },
    // The soft enrage: once, for the rest of the fight. Everything he does comes a third faster.
    EnemyAbilityDef {
        name: "OVERHEAT",
        description: "The furnace goes red-hot: attacks much faster until he dies.",
        counter: "Burst him down",
        range: 40.0,
        cooldown: 30.0,
        effect: EnemyEffect::Frenzy {
            duration: 1200.0,
            haste: 0.35,
        },
        recovery: 0.2,
        once_per_phase: true,
        cue: EnemyCue::Heavy,
        ..EnemyAbilityDef::BASE
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
    /// Allies called the moment the phase begins, if it calls any.
    pub summon: Option<PhaseSummon>,
}

/// Allies a boss calls when it enters a phase.
///
/// They arrive at the dungeon's add spawn points (or beside the boss when it has none), already
/// in the fight, and belong to the boss's encounter: a wipe takes them away with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhaseSummon {
    /// Which monster, exactly as the table has it (not its empowered form).
    pub kind: EnemyKind,
    /// How many.
    pub count: u8,
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

    // ---------------------------------------------------------------- the Graveyard Crypt
    /// Ranged magical area damage: barrages, marks and a spear.
    SkeletonBonecaster,
    /// The dangerous caster: cursed ground and two channels that must be stopped.
    GraveAcolyte,
    /// Long-range physical pressure: volleys, slows and rain.
    CryptArcher,
    /// Melee cleave: the cone that punishes a tank facing it at the party.
    SkeletonWarrior,
    /// Heavy melee area and burst: slams, quakes and a charge.
    GraveBrute,
    /// Fast melee: stacking bleeds, a leap and a frenzy.
    CryptGhoul,
    /// The Bonecaster the boss calls: more zones, and marks that chain.
    EmpoweredBonecaster,
    /// The Acolyte the boss calls: ground that spreads, and a drain on everybody.
    EmpoweredAcolyte,
    /// The Archer the boss calls: faster rain, and volleys that curse the ground.
    EmpoweredArcher,
    /// The Warrior the boss calls: a wider cleave, and a charge with a shockwave.
    EmpoweredWarrior,
    /// The Brute the boss calls: a quake all around it, and a charge that leaves a trail.
    EmpoweredBrute,
    /// The Ghoul the boss calls: a deeper bleed, extra bites, and an exploding pounce.
    EmpoweredGhoul,
    /// The Graveyard Crypt's boss: Lord Ashgrave, the Hollow Steward.
    CryptSteward,

    // ---------------------------------------------------------------- the Drowned Catacombs
    /// Support: heals its most wounded ally. Low health, and the cast can be kicked.
    DrownedTidecaller,
    /// Caster: a bolt that leaves a dispellable slow and vulnerability on a random player.
    DrownedCultist,
    /// Elite melee that patrols the corridors with a wide frontal cleave.
    CatacombLurker,
    /// Ranged: volleys that mark the ground around several players.
    BilgeArcher,
    /// The Drowned Catacombs' boss: the Drowned Abbess.
    DrownedAbbess,

    // ---------------------------------------------------------------- the Bone Foundry
    /// Shielder: wards its allies against damage; kick the ward, or go through it.
    SlagIronclad,
    /// Suicide runner: a fused circle at the party's feet, defused by killing it.
    CinderBomber,
    /// Ranged: slag lobbed onto the ground under two players.
    SlagThrower,
    /// Elite buffer: stokes its allies to hit harder; kick the rally.
    FoundryForeman,
    /// The Bone Foundry's boss: the Foundry Overseer.
    FoundryOverseer,
}

/// How many enemy kinds there are.
pub const ENEMY_KIND_COUNT: usize = 30;

/// The six monsters the Graveyard Crypt is built from, in the order they are documented.
///
/// Three ranged, three melee. What the boss's add pool defaults to, and what the roster tests
/// iterate.
pub const GRAVEYARD_ROSTER: [EnemyKind; 6] = [
    EnemyKind::SkeletonBonecaster,
    EnemyKind::GraveAcolyte,
    EnemyKind::CryptArcher,
    EnemyKind::SkeletonWarrior,
    EnemyKind::GraveBrute,
    EnemyKind::CryptGhoul,
];

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
        Self::SkeletonBonecaster,
        Self::GraveAcolyte,
        Self::CryptArcher,
        Self::SkeletonWarrior,
        Self::GraveBrute,
        Self::CryptGhoul,
        Self::EmpoweredBonecaster,
        Self::EmpoweredAcolyte,
        Self::EmpoweredArcher,
        Self::EmpoweredWarrior,
        Self::EmpoweredBrute,
        Self::EmpoweredGhoul,
        Self::CryptSteward,
        Self::DrownedTidecaller,
        Self::DrownedCultist,
        Self::CatacombLurker,
        Self::BilgeArcher,
        Self::DrownedAbbess,
        Self::SlagIronclad,
        Self::CinderBomber,
        Self::SlagThrower,
        Self::FoundryForeman,
        Self::FoundryOverseer,
    ];

    /// Whether this kind is a boss, for the boss frame and the encounter readout.
    #[must_use]
    pub fn is_boss(self) -> bool {
        self.archetype().tier == MonsterTier::Boss
    }

    /// The kind whose [`EnemyArchetype::id`] is `id`, if there is one.
    ///
    /// How a level file names a monster: by the id, never by the index, so the table can
    /// grow and reorder without a single saved level changing meaning.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        let id = id.trim();
        Self::ALL
            .into_iter()
            .find(|kind| kind.archetype().id.eq_ignore_ascii_case(id))
    }

    /// Its id, as a level file spells it.
    #[must_use]
    pub fn id(self) -> &'static str {
        self.archetype().id
    }

    /// The empowered form the boss calls instead, if it has one.
    #[must_use]
    pub fn empowered(self) -> Option<Self> {
        self.archetype().empowered
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
    /// Its id in a level file: `"skeleton_warrior"`. Lower case, underscores, never renamed.
    pub id: &'static str,
    /// What it does in a fight, for the editor's browser and the documentation.
    pub role: MonsterRole,
    /// Whether it is ordinary, an empowered add or a boss.
    pub tier: MonsterTier,
    /// One line on what it asks of the party, for the editor's browser.
    pub summary: &'static str,
    /// How close a target may come before a ranged monster backs away, in metres. Zero is
    /// never.
    ///
    /// Only when nothing is ready to cast: a caster with a bolt off cooldown casts it rather
    /// than walking, so backing off costs it nothing it would otherwise have done.
    pub retreat_below: f32,
    /// What killing it is worth towards dungeon completion, before a placement overrides it.
    pub completion: f32,
    /// How it looks, in terms presentation can read without knowing what it is.
    pub look: MonsterLook,
    /// The empowered form a boss calls instead of this one.
    pub empowered: Option<EnemyKind>,
    /// What it can drop, for the dungeon map's loot list.
    ///
    /// Rows of the item table by id, never a copy of an item: the map shows the item's own
    /// name, quality, slot and stats. Empty means no loot is configured, and the map says
    /// exactly that rather than inventing rewards.
    pub loot: &'static [LootEntry],
}

/// One item a monster can drop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LootEntry {
    /// Which item, by its row in [`crate::character::ITEMS`].
    pub item: crate::character::ItemId,
    /// How likely it is to drop from one kill, `0..=1`.
    pub chance: f32,
}

/// What a monster does in a fight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonsterRole {
    /// Stays at arm's length and hits.
    Melee,
    /// Stays back and casts or shoots.
    Ranged,
    /// Keeps its allies alive.
    Support,
    /// A named, larger enemy with a phase of its own.
    Elite,
    /// The end of a dungeon.
    Boss,
}

impl MonsterRole {
    /// Its name, for the browser.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Melee => "MELEE",
            Self::Ranged => "RANGED",
            Self::Support => "SUPPORT",
            Self::Elite => "ELITE",
            Self::Boss => "BOSS",
        }
    }
}

/// How dangerous a kind is meant to be, independent of its role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MonsterTier {
    /// A monster a pack is made of.
    Normal,
    /// A monster the boss calls: an ordinary one's harder, escalated form.
    Empowered,
    /// A boss.
    Boss,
}

/// How a monster looks, as numbers presentation can read.
///
/// Not a mesh and not a material: `engine-game` cannot name either. A tint on the shared
/// enemy model, a glow, and the model's name for a future asset to replace the stand-in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonsterLook {
    /// Linear colour the enemy model is tinted towards.
    pub tint: [f32; 3],
    /// How much it glows, `0..1`. Empowered adds glow; ordinary monsters do not.
    pub glow: f32,
    /// The model it would be drawn with if the asset existed: `"skeleton_warrior"`.
    pub model: &'static str,
}

impl MonsterLook {
    /// The look every enemy had before looks existed: the dummy, tinted red.
    pub const HOSTILE: Self = Self {
        tint: [1.0, 0.45, 0.38],
        glow: 0.0,
        model: "training_dummy",
    };

    /// The hostile red, drawn with `model`: the Ashen Reliquary's monsters, which were
    /// tinted dummies before the crypt's models existed and borrow them now.
    #[must_use]
    pub const fn hostile(model: &'static str) -> Self {
        Self {
            model,
            ..Self::HOSTILE
        }
    }
}

/// What a monster's level multiplies its health and damage by.
///
/// Twelve per cent more health and six per cent more damage per level above the first: a
/// level is a dial for pacing — how long a pack takes — far more than for lethality, because
/// what kills a party in this dungeon is the mechanics.
#[must_use]
pub fn level_scale(level: u8) -> (f32, f32) {
    let above = f32::from(level.max(1) - 1);
    (1.0 + 0.12 * above, 1.0 + 0.06 * above)
}

/// How close the player must come, in metres, before a monster standing in a pack notices.
///
/// One number for every monster a level places, bosses included: how far a pack sees is a
/// property of the dungeon's pacing, not of one skeleton, and a pull whose reach depends on
/// which member the player walked towards is a pull nobody can plan. A level can still widen
/// one placement — see [`crate::dungeon`]'s `aggro_radius` override. The empowered adds a boss
/// calls are the exception: they arrive already in the fight.
///
/// 10.8, which is the original 18 m less forty per cent: packs noticed the player from too far
/// away to be pulled one at a time.
pub const AGGRO_RADIUS: f32 = 10.8;

/// How far an empowered add a boss calls notices the party: the original 40 m less forty per
/// cent. Still wide, because it arrives in the middle of the fight.
pub const EMPOWERED_AGGRO_RADIUS: f32 = 24.0;

impl EnemyArchetype {
    /// Every field at a neutral value, for the table rows to override.
    ///
    /// The same arrangement [`EnemyAbilityDef::BASE`] gives the ability table: a row names
    /// what is particular about its monster, and a column added later is a default here.
    pub const BASE: Self = Self {
        name: "",
        max_health: 100.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.5,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 45.0,
        preferred_range: 2.4,
        range_slack: 0.6,
        alert_time: 0.5,
        abilities: [None; MAX_ENEMY_ABILITIES],
        phases: &[],
        id: "",
        role: MonsterRole::Melee,
        tier: MonsterTier::Normal,
        summary: "",
        retreat_below: 0.0,
        completion: 0.0,
        look: MonsterLook::HOSTILE,
        empowered: None,
        loot: &[],
    };

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

    /// The allies it calls on entering `phase`, if that phase calls any.
    #[must_use]
    pub fn phase_summon(&self, phase: usize) -> Option<PhaseSummon> {
        let index = phase.checked_sub(1)?;
        self.phases.get(index)?.summon
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
    // 1.06 rather than a bigger number: the base speed is already 6.5 m/s and the player's
    // top speed is 7.0. A phase that scaled past that would be an elite that cannot be kited
    // and an Ash Burst that cannot be walked out of.
    speed_scale: 1.06,
    cooldown_scale: 0.7,
    summon: None,
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
        speed_scale: 1.04,
        cooldown_scale: 0.85,
        summon: None,
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
        // Held under the player's 7.0 m/s, which is what makes Immolation Nova a mechanic
        // rather than a death sentence: the boss stands still to cast it, and the player has
        // to be able to leave before it lands.
        speed_scale: 1.10,
        cooldown_scale: 0.7,
        summon: None,
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
        speed: 6.76,
        aggro_radius: AGGRO_RADIUS,
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
        id: "brute",
        role: MonsterRole::Melee,
        tier: MonsterTier::Normal,
        summary: "Closes and cleaves: the first lesson in where to stand.",
        completion: 4.0,
        look: MonsterLook::hostile("grave_brute"),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "EMBER ADEPT",
        max_health: 210.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.46,
        aggro_radius: AGGRO_RADIUS,
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
        id: "ember_adept",
        role: MonsterRole::Ranged,
        tier: MonsterTier::Normal,
        summary: "Keeps its distance and drops fire where you stand.",
        completion: 3.0,
        look: MonsterLook::hostile("skeleton_bonecaster"),
        ..EnemyArchetype::BASE
    },
    // Stands further back than the Adept, so in a mixed pack reaching it is a decision.
    EnemyArchetype {
        name: "BONE RITUALIST",
        max_health: 260.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.2,
        aggro_radius: AGGRO_RADIUS,
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
        id: "bone_ritualist",
        role: MonsterRole::Ranged,
        tier: MonsterTier::Normal,
        summary: "A three-second nova that must be interrupted.",
        completion: 4.0,
        look: MonsterLook::hostile("grave_acolyte"),
        ..EnemyArchetype::BASE
    },
    // Closer in than the other casters: a healer that stands further back than everything it
    // heals is a healer the player cannot reach, and the mechanic is target priority.
    EnemyArchetype {
        name: "CRYPT WARDEN",
        max_health: 240.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.72,
        aggro_radius: AGGRO_RADIUS,
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
        id: "crypt_warden",
        role: MonsterRole::Support,
        tier: MonsterTier::Normal,
        summary: "Heals whichever ally is worst off: kill it first.",
        completion: 4.0,
        look: MonsterLook::hostile("grave_acolyte"),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "CINDER WEAVER",
        max_health: 230.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.72,
        aggro_radius: AGGRO_RADIUS,
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
        id: "cinder_weaver",
        role: MonsterRole::Ranged,
        tier: MonsterTier::Normal,
        summary: "Leaves burning ground under the player.",
        completion: 3.0,
        look: MonsterLook::hostile("skeleton_bonecaster"),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "ASH REVENANT",
        max_health: 900.0,
        radius: 0.75,
        height: 2.3,
        speed: 6.5,
        aggro_radius: AGGRO_RADIUS,
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
        id: "ash_revenant",
        role: MonsterRole::Elite,
        tier: MonsterTier::Normal,
        summary: "An elite that changes at half health.",
        completion: 10.0,
        look: MonsterLook::hostile("skeleton_warrior"),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "WARDEN OF THE ASHEN GATE",
        max_health: 6000.0,
        radius: 1.1,
        height: 3.0,
        // Slower than the player in every phase. Nothing in this fight kills a player who is
        // moving, and everything in it kills a player who is not.
        speed: 6.24,
        aggro_radius: AGGRO_RADIUS,
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
        id: "warden_of_the_ashen_gate",
        role: MonsterRole::Boss,
        tier: MonsterTier::Boss,
        summary: "Three phases, each adding a mechanic.",
        completion: 0.0,
        look: MonsterLook::hostile("lord_ashgrave"),
        ..EnemyArchetype::BASE
    },
    // ======================================================================================
    // The Graveyard Crypt. Health is tuned for a party of three at level one — see the
    // ability table's header for the rule the damage follows. Every speed is under the
    // player's 7 m/s, so every charge, slam and quake can be walked out of.
    // ======================================================================================
    EnemyArchetype {
        name: "SKELETON BONECASTER",
        max_health: 1000.0,
        radius: 0.5,
        height: 2.0,
        speed: 5.4,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 18.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::GraveExplosion),
            Some(EnemyAbility::BoneSpear),
            Some(EnemyAbility::BoneBarrage),
            Some(EnemyAbility::BoneBolt),
            None,
            None,
        ],
        id: "skeleton_bonecaster",
        role: MonsterRole::Ranged,
        summary: "Ranged AoE: dodge the barrage, sidestep the spear, carry the mark away.",
        retreat_below: 7.0,
        completion: 4.0,
        look: MonsterLook {
            tint: [0.82, 0.86, 1.0],
            glow: 0.0,
            model: "skeleton_bonecaster",
        },
        empowered: Some(EnemyKind::EmpoweredBonecaster),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "GRAVE ACOLYTE",
        max_health: 1150.0,
        radius: 0.5,
        height: 1.95,
        speed: 5.4,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 16.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::GraveDrain),
            Some(EnemyAbility::DarkPulse),
            Some(EnemyAbility::CorruptedGround),
            Some(EnemyAbility::ShadowBolt),
            None,
            None,
        ],
        id: "grave_acolyte",
        role: MonsterRole::Ranged,
        summary: "Dangerous caster: interrupt the drain, leave the cursed ground.",
        retreat_below: 7.0,
        completion: 5.0,
        look: MonsterLook {
            tint: [0.62, 0.40, 0.88],
            glow: 0.0,
            model: "grave_acolyte",
        },
        empowered: Some(EnemyKind::EmpoweredAcolyte),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "CRYPT ARCHER",
        max_health: 950.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.6,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 24.0,
        range_slack: 4.0,
        alert_time: 0.45,
        abilities: [
            Some(EnemyAbility::RainOfArrows),
            Some(EnemyAbility::MultiShot),
            Some(EnemyAbility::CripplingArrow),
            Some(EnemyAbility::BoneArrow),
            None,
            None,
        ],
        id: "crypt_archer",
        role: MonsterRole::Ranged,
        summary: "Long-range pressure: out of the rain, and mind the slow.",
        retreat_below: 9.0,
        completion: 4.0,
        look: MonsterLook {
            tint: [0.66, 0.82, 0.70],
            glow: 0.0,
            model: "crypt_archer",
        },
        empowered: Some(EnemyKind::EmpoweredArcher),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "SKELETON WARRIOR",
        max_health: 1500.0,
        radius: 0.6,
        height: 2.1,
        speed: 6.3,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 2.5,
        range_slack: 0.6,
        alert_time: 0.4,
        abilities: [
            Some(EnemyAbility::ExecutionStrike),
            Some(EnemyAbility::BoneCharge),
            Some(EnemyAbility::BoneCleave),
            Some(EnemyAbility::BoneSlash),
            None,
            None,
        ],
        id: "skeleton_warrior",
        role: MonsterRole::Melee,
        summary: "Melee cleave: face it away from the party, kick the execution.",
        completion: 4.0,
        look: MonsterLook {
            tint: [0.93, 0.88, 0.76],
            glow: 0.0,
            model: "skeleton_warrior",
        },
        empowered: Some(EnemyKind::EmpoweredWarrior),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "GRAVE BRUTE",
        max_health: 2200.0,
        radius: 0.9,
        height: 2.7,
        speed: 5.9,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 2.8,
        range_slack: 0.6,
        alert_time: 0.6,
        abilities: [
            Some(EnemyAbility::BoneQuake),
            Some(EnemyAbility::GraveCharge),
            Some(EnemyAbility::GroundSlam),
            Some(EnemyAbility::CrushingBlow),
            None,
            None,
        ],
        id: "grave_brute",
        role: MonsterRole::Melee,
        summary: "Heavy AoE: leave the slam and the quake, spread for the charge.",
        completion: 6.0,
        look: MonsterLook {
            tint: [0.70, 0.62, 0.55],
            glow: 0.0,
            model: "grave_brute",
        },
        empowered: Some(EnemyKind::EmpoweredBrute),
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "CRYPT GHOUL",
        max_health: 1100.0,
        radius: 0.55,
        height: 1.6,
        // The fastest thing in the roster, and still under the player's 7.0.
        speed: 6.9,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 2.2,
        range_slack: 0.5,
        alert_time: 0.3,
        abilities: [
            Some(EnemyAbility::BloodFrenzy),
            Some(EnemyAbility::Pounce),
            Some(EnemyAbility::RendingClaws),
            Some(EnemyAbility::GhoulStrike),
            None,
            None,
        ],
        id: "crypt_ghoul",
        role: MonsterRole::Melee,
        summary: "Fast bleeds: kill it in its frenzy before the stacks climb.",
        completion: 3.0,
        look: MonsterLook {
            tint: [0.86, 0.55, 0.50],
            glow: 0.0,
            model: "crypt_ghoul",
        },
        empowered: Some(EnemyKind::EmpoweredGhoul),
        ..EnemyArchetype::BASE
    },
    // ---------------------------------------------------------------- the empowered adds
    // Larger, glowing, and with roughly twice the health of the monster the party already
    // knows — but what makes each one harder is its two escalated abilities, not the health.
    EnemyArchetype {
        name: "EMPOWERED BONECASTER",
        max_health: 2400.0,
        radius: 0.55,
        height: 2.3,
        speed: 5.4,
        aggro_radius: EMPOWERED_AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 18.0,
        range_slack: 3.0,
        alert_time: 0.3,
        abilities: [
            Some(EnemyAbility::GraveChainExplosion),
            Some(EnemyAbility::BoneSpear),
            Some(EnemyAbility::MassiveBoneBarrage),
            Some(EnemyAbility::BoneBolt),
            None,
            None,
        ],
        id: "empowered_bonecaster",
        role: MonsterRole::Ranged,
        tier: MonsterTier::Empowered,
        summary: "Chain marks and a massive barrage: spread, then keep moving.",
        retreat_below: 7.0,
        look: MonsterLook {
            tint: [0.82, 0.86, 1.0],
            glow: 0.65,
            model: "skeleton_bonecaster",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "EMPOWERED ACOLYTE",
        max_health: 2600.0,
        radius: 0.55,
        height: 2.25,
        speed: 5.4,
        aggro_radius: EMPOWERED_AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 16.0,
        range_slack: 3.0,
        alert_time: 0.3,
        abilities: [
            Some(EnemyAbility::MassGraveDrain),
            Some(EnemyAbility::DarkPulse),
            Some(EnemyAbility::SpreadingCorruption),
            Some(EnemyAbility::ShadowBolt),
            None,
            None,
        ],
        id: "empowered_acolyte",
        role: MonsterRole::Ranged,
        tier: MonsterTier::Empowered,
        summary: "A drain on everybody and ground that spreads: an interrupt every 16 s.",
        retreat_below: 7.0,
        look: MonsterLook {
            tint: [0.62, 0.40, 0.88],
            glow: 0.65,
            model: "grave_acolyte",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "EMPOWERED ARCHER",
        max_health: 2300.0,
        radius: 0.55,
        height: 2.2,
        speed: 5.6,
        aggro_radius: EMPOWERED_AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 24.0,
        range_slack: 4.0,
        alert_time: 0.3,
        abilities: [
            Some(EnemyAbility::DeadlyRain),
            Some(EnemyAbility::BoneVolley),
            Some(EnemyAbility::CripplingArrow),
            Some(EnemyAbility::BoneArrow),
            None,
            None,
        ],
        id: "empowered_archer",
        role: MonsterRole::Ranged,
        tier: MonsterTier::Empowered,
        summary: "Rain with half the warning, and volleys that curse the ground.",
        retreat_below: 9.0,
        look: MonsterLook {
            tint: [0.66, 0.82, 0.70],
            glow: 0.65,
            model: "crypt_archer",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "EMPOWERED WARRIOR",
        max_health: 3300.0,
        radius: 0.7,
        height: 2.45,
        speed: 6.3,
        aggro_radius: EMPOWERED_AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 2.6,
        range_slack: 0.6,
        alert_time: 0.3,
        abilities: [
            Some(EnemyAbility::ExecutionStrike),
            Some(EnemyAbility::ShatteringCharge),
            Some(EnemyAbility::Massacre),
            Some(EnemyAbility::BoneSlash),
            None,
            None,
        ],
        id: "empowered_warrior",
        role: MonsterRole::Melee,
        tier: MonsterTier::Empowered,
        summary: "A 150-degree Massacre and a charge that shatters: nobody in front.",
        look: MonsterLook {
            tint: [0.93, 0.88, 0.76],
            glow: 0.65,
            model: "skeleton_warrior",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "EMPOWERED BRUTE",
        max_health: 4400.0,
        radius: 1.0,
        height: 3.1,
        speed: 5.9,
        aggro_radius: EMPOWERED_AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 2.9,
        range_slack: 0.6,
        alert_time: 0.3,
        abilities: [
            Some(EnemyAbility::Gravequake),
            Some(EnemyAbility::DevastatingCharge),
            Some(EnemyAbility::GroundSlam),
            Some(EnemyAbility::CrushingBlow),
            None,
            None,
        ],
        id: "empowered_brute",
        role: MonsterRole::Melee,
        tier: MonsterTier::Empowered,
        summary: "Quakes all around it and a charge that leaves a trail.",
        look: MonsterLook {
            tint: [0.70, 0.62, 0.55],
            glow: 0.65,
            model: "grave_brute",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "EMPOWERED GHOUL",
        max_health: 2600.0,
        radius: 0.6,
        height: 1.85,
        speed: 6.9,
        aggro_radius: EMPOWERED_AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 2.2,
        range_slack: 0.5,
        alert_time: 0.25,
        abilities: [
            Some(EnemyAbility::BloodFrenzy),
            Some(EnemyAbility::SavagePounce),
            Some(EnemyAbility::FeastOnTheWounded),
            Some(EnemyAbility::RendingFrenzy),
            Some(EnemyAbility::GhoulStrike),
            None,
        ],
        id: "empowered_ghoul",
        role: MonsterRole::Melee,
        tier: MonsterTier::Empowered,
        summary: "Deeper bleeds, extra bites on the bleeding, and a pounce that explodes.",
        look: MonsterLook {
            tint: [0.86, 0.55, 0.50],
            glow: 0.65,
            model: "crypt_ghoul",
        },
        ..EnemyArchetype::BASE
    },
    // ---------------------------------------------------------------- the Hollow Steward
    EnemyArchetype {
        name: "LORD ASHGRAVE",
        max_health: 16000.0,
        radius: 1.0,
        height: 3.2,
        speed: 6.3,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 3.0,
        range_slack: 0.8,
        alert_time: 0.8,
        abilities: [
            Some(EnemyAbility::MarkOfTheHost),
            Some(EnemyAbility::GhastlyWaltz),
            Some(EnemyAbility::DanceOfBlades),
            Some(EnemyAbility::Garrote),
            Some(EnemyAbility::GougingRiposte),
            Some(EnemyAbility::RapierThrust),
        ],
        phases: &STEWARD_PHASES,
        id: "lord_ashgrave",
        role: MonsterRole::Boss,
        tier: MonsterTier::Boss,
        summary: "The Hollow Steward: two empowered guests, a garrote, a waltz and a mark.",
        look: MonsterLook {
            tint: [0.55, 0.14, 0.20],
            glow: 0.25,
            model: "lord_ashgrave",
        },
        ..EnemyArchetype::BASE
    },
    // ======================================================================================
    // The Drowned Catacombs. Models and tints are borrowed from the crypt's, the way the
    // empowered adds borrow theirs: a sea-green wash over a body the party already knows.
    // ======================================================================================
    // Low health on purpose: the question it asks is "can you reach it", not "can you outlast
    // it". Closer in than the cultists so a pack that is fought properly has it in reach.
    EnemyArchetype {
        name: "DROWNED TIDECALLER",
        max_health: 700.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.5,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 15.0,
        range_slack: 3.0,
        alert_time: 0.45,
        abilities: [
            Some(EnemyAbility::TidalMend),
            Some(EnemyAbility::TideLash),
            None,
            None,
            None,
            None,
        ],
        id: "drowned_tidecaller",
        role: MonsterRole::Support,
        summary: "Heals whichever ally is worst off: kick the heal, then kill it.",
        retreat_below: 7.0,
        completion: 4.0,
        look: MonsterLook {
            tint: [0.45, 0.78, 0.86],
            glow: 0.0,
            model: "skeleton_bonecaster",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "DROWNED CULTIST",
        max_health: 900.0,
        radius: 0.5,
        height: 1.95,
        speed: 5.4,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 17.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::BrineBolt),
            Some(EnemyAbility::ShadowBolt),
            None,
            None,
            None,
            None,
        ],
        id: "drowned_cultist",
        role: MonsterRole::Ranged,
        summary: "Brine bolts leave Waterlogged on a random player: kick them, dispel the rest.",
        retreat_below: 7.0,
        completion: 4.0,
        look: MonsterLook {
            tint: [0.36, 0.62, 0.74],
            glow: 0.0,
            model: "grave_acolyte",
        },
        ..EnemyArchetype::BASE
    },
    // Slower than the player and patrolling at half speed, so it can be watched, timed and
    // walked around — or pulled on purpose, away from the room it guards.
    EnemyArchetype {
        name: "CATACOMB LURKER",
        max_health: 3400.0,
        radius: 0.85,
        height: 2.5,
        speed: 6.0,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 2.6,
        range_slack: 0.6,
        alert_time: 0.6,
        abilities: [
            Some(EnemyAbility::TidalCleave),
            Some(EnemyAbility::LurkerClaw),
            None,
            None,
            None,
            None,
        ],
        id: "catacomb_lurker",
        role: MonsterRole::Elite,
        summary: "An elite that walks the corridors: turn its cleave away from the party.",
        completion: 10.0,
        look: MonsterLook {
            tint: [0.34, 0.52, 0.58],
            glow: 0.0,
            model: "grave_brute",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "BILGE ARCHER",
        max_health: 850.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.6,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 24.0,
        range_slack: 4.0,
        alert_time: 0.45,
        abilities: [
            Some(EnemyAbility::BilgeVolley),
            Some(EnemyAbility::BoneArrow),
            None,
            None,
            None,
            None,
        ],
        id: "bilge_archer",
        role: MonsterRole::Ranged,
        summary: "Volleys of marked circles: keep moving, and kick the volley.",
        retreat_below: 9.0,
        completion: 4.0,
        look: MonsterLook {
            tint: [0.52, 0.78, 0.70],
            glow: 0.0,
            model: "crypt_archer",
        },
        ..EnemyArchetype::BASE
    },
    // ---------------------------------------------------------------- the Drowned Abbess
    EnemyArchetype {
        name: "THE DROWNED ABBESS",
        max_health: 17000.0,
        radius: 1.1,
        height: 3.1,
        // Slower than the player in every phase: the Undertow is escaped by walking, and a
        // boss that outran a player would make that a lie.
        speed: 6.2,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 3.4,
        range_slack: 0.8,
        alert_time: 0.8,
        abilities: [
            Some(EnemyAbility::TidalSlam),
            Some(EnemyAbility::AbbessCrozier),
            Some(EnemyAbility::BrineBolt),
            None,
            None,
            None,
        ],
        phases: &ABBESS_PHASES,
        id: "drowned_abbess",
        role: MonsterRole::Boss,
        tier: MonsterTier::Boss,
        summary: "A pulling current, a hymn that must be kicked, and healers called at half health.",
        look: MonsterLook {
            tint: [0.30, 0.62, 0.72],
            glow: 0.3,
            model: "grave_acolyte",
        },
        ..EnemyArchetype::BASE
    },

    // ======================================================================================
    // The Bone Foundry. Existing bodies again, washed in forge colours: iron grey, ember orange.
    // ======================================================================================
    // Not a damage dealer. The question it asks is "do we kick the ward or go through it": a
    // pack under Slag Bulwark takes forty per cent less for eight seconds in sixteen.
    EnemyArchetype {
        name: "SLAG IRONCLAD",
        max_health: 3000.0,
        radius: 0.9,
        height: 2.6,
        speed: 5.6,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 2.6,
        range_slack: 0.6,
        alert_time: 0.6,
        abilities: [
            Some(EnemyAbility::SlagBulwark),
            Some(EnemyAbility::ShieldSweep),
            Some(EnemyAbility::SlagBash),
            None,
            None,
            None,
        ],
        id: "slag_ironclad",
        role: MonsterRole::Melee,
        summary: "Wards its pack against a share of all damage: kick the ward, turn the sweep.",
        completion: 6.0,
        look: MonsterLook {
            tint: [0.50, 0.50, 0.56],
            glow: 0.0,
            model: "grave_brute",
        },
        ..EnemyArchetype::BASE
    },
    // Faster than a walk and not much faster than a run: it arrives, and it is the first thing
    // anyone sees, because it is the only monster that will not stop for the tank.
    EnemyArchetype {
        name: "CINDER BOMBER",
        max_health: 700.0,
        radius: 0.5,
        height: 1.6,
        speed: 6.6,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 1.2,
        range_slack: 0.4,
        alert_time: 0.35,
        abilities: [
            Some(EnemyAbility::CinderBlast),
            None,
            None,
            None,
            None,
            None,
        ],
        id: "cinder_bomber",
        role: MonsterRole::Melee,
        summary: "Runs in, lights a fuse and blows up: kill it first, or leave the circle.",
        completion: 3.0,
        look: MonsterLook {
            tint: [1.0, 0.52, 0.22],
            glow: 0.6,
            model: "crypt_ghoul",
        },
        ..EnemyArchetype::BASE
    },
    EnemyArchetype {
        name: "SLAG THROWER",
        max_health: 950.0,
        radius: 0.5,
        height: 1.9,
        speed: 5.4,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 22.0,
        range_slack: 3.0,
        alert_time: 0.5,
        abilities: [
            Some(EnemyAbility::SlagLob),
            Some(EnemyAbility::EmberFlick),
            None,
            None,
            None,
            None,
        ],
        id: "slag_thrower",
        role: MonsterRole::Ranged,
        summary: "Lobs slag under two players at a time: leave the circle, then the pool.",
        retreat_below: 8.0,
        completion: 4.0,
        look: MonsterLook {
            tint: [0.95, 0.60, 0.30],
            glow: 0.3,
            model: "skeleton_bonecaster",
        },
        ..EnemyArchetype::BASE
    },
    // Walks its rounds in one place and calls the pack to arms in another; reach it before the
    // rally lands, or take the pack's next ten seconds at thirty per cent more.
    EnemyArchetype {
        name: "FOUNDRY FOREMAN",
        max_health: 3800.0,
        radius: 0.85,
        height: 2.5,
        speed: 6.0,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 50.0,
        preferred_range: 2.4,
        range_slack: 0.6,
        alert_time: 0.6,
        abilities: [
            Some(EnemyAbility::StokeTheFurnace),
            Some(EnemyAbility::ChainLash),
            Some(EnemyAbility::ForemansClub),
            None,
            None,
            None,
        ],
        id: "foundry_foreman",
        role: MonsterRole::Elite,
        summary: "Rallies its pack to hit harder: interrupt the rally, step out of the lash.",
        completion: 10.0,
        look: MonsterLook {
            tint: [0.92, 0.46, 0.20],
            glow: 0.15,
            model: "grave_acolyte",
        },
        ..EnemyArchetype::BASE
    },
    // ---------------------------------------------------------------- the Foundry Overseer
    EnemyArchetype {
        name: "THE FOUNDRY OVERSEER",
        max_health: 19000.0,
        radius: 1.2,
        height: 3.3,
        // Slower than a player in every phase, overheated included: the lane is walked out of.
        speed: 6.0,
        aggro_radius: AGGRO_RADIUS,
        leash_radius: 70.0,
        preferred_range: 2.8,
        range_slack: 0.8,
        alert_time: 0.8,
        abilities: [
            Some(EnemyAbility::FoundrySlam),
            Some(EnemyAbility::SlagBrand),
            Some(EnemyAbility::OverseerHammer),
            None,
            None,
            None,
        ],
        phases: &OVERSEER_PHASES,
        id: "foundry_overseer",
        role: MonsterRole::Boss,
        tier: MonsterTier::Boss,
        summary: "Spread for the brands, stack for the ladle, leave the lane, burn him before he overheats.",
        look: MonsterLook {
            tint: [0.95, 0.42, 0.16],
            glow: 0.5,
            model: "grave_brute",
        },
        ..EnemyArchetype::BASE
    },
];

/// The Foundry Overseer. Each phase adds one idea to the last: the ladle at seventy, the lane and
/// the bombers at fifty, the overheat and the throwers at twenty-five. The brand and the ladle
/// are opposites (spread, stack) and the lane is movement; the soft enrage is that he never
/// slows down once he is red-hot.
static OVERSEER_PHASES: [EnemyPhaseDef; 3] = [
    EnemyPhaseDef {
        name: "THE LADLE",
        at_or_below: 0.70,
        abilities: [
            Some(EnemyAbility::MoltenLadle),
            Some(EnemyAbility::FoundrySlam),
            Some(EnemyAbility::SlagBrand),
            Some(EnemyAbility::OverseerHammer),
            None,
            None,
        ],
        speed_scale: 1.0,
        cooldown_scale: 0.95,
        summon: None,
    },
    EnemyPhaseDef {
        name: "THE CONVEYOR",
        at_or_below: 0.50,
        abilities: [
            Some(EnemyAbility::ConveyorSurge),
            Some(EnemyAbility::MoltenLadle),
            Some(EnemyAbility::FoundrySlam),
            Some(EnemyAbility::SlagBrand),
            Some(EnemyAbility::OverseerHammer),
            None,
        ],
        speed_scale: 1.0,
        cooldown_scale: 0.9,
        summon: Some(PhaseSummon {
            kind: EnemyKind::CinderBomber,
            count: 3,
        }),
    },
    EnemyPhaseDef {
        name: "OVERHEAT",
        at_or_below: 0.25,
        abilities: [
            Some(EnemyAbility::Overheat),
            Some(EnemyAbility::ConveyorSurge),
            Some(EnemyAbility::MoltenLadle),
            Some(EnemyAbility::SlagBrand),
            Some(EnemyAbility::FoundrySlam),
            Some(EnemyAbility::OverseerHammer),
        ],
        speed_scale: 1.0,
        cooldown_scale: 0.85,
        summon: Some(PhaseSummon {
            kind: EnemyKind::SlagThrower,
            count: 2,
        }),
    },
];

/// The Drowned Abbess. The percentages are the design: the hymn at sixty-six and at
/// thirty-three (the same ability, once a phase), the tidecallers at fifty, and the Undertow
/// from the first hymn on. Every cooldown shortens a little as she goes, and she never moves
/// faster than the player.
static ABBESS_PHASES: [EnemyPhaseDef; 3] = [
    EnemyPhaseDef {
        name: "THE HYMN",
        at_or_below: 0.66,
        abilities: [
            Some(EnemyAbility::DrowningHymn),
            Some(EnemyAbility::Undertow),
            Some(EnemyAbility::TidalSlam),
            Some(EnemyAbility::AbbessCrozier),
            Some(EnemyAbility::BrineBolt),
            None,
        ],
        speed_scale: 1.0,
        cooldown_scale: 0.95,
        summon: None,
    },
    EnemyPhaseDef {
        name: "THE CHOIR",
        at_or_below: 0.5,
        abilities: [
            Some(EnemyAbility::Undertow),
            Some(EnemyAbility::TidalSlam),
            Some(EnemyAbility::AbbessCrozier),
            Some(EnemyAbility::BrineBolt),
            None,
            None,
        ],
        speed_scale: 1.0,
        cooldown_scale: 0.9,
        summon: Some(PhaseSummon {
            kind: EnemyKind::DrownedTidecaller,
            count: 2,
        }),
    },
    EnemyPhaseDef {
        name: "THE DEEP",
        at_or_below: 0.33,
        abilities: [
            Some(EnemyAbility::DrowningHymn),
            Some(EnemyAbility::Undertow),
            Some(EnemyAbility::TidalSlam),
            Some(EnemyAbility::AbbessCrozier),
            Some(EnemyAbility::BrineBolt),
            None,
        ],
        speed_scale: 1.04,
        cooldown_scale: 0.8,
        summon: None,
    },
];

/// Lord Ashgrave unmasked, at thirty per cent: the frenzy joins the list, every cooldown
/// shortens, and he moves a little faster — still slower than the player.
static STEWARD_PHASES: [EnemyPhaseDef; 1] = [EnemyPhaseDef {
    name: "UNMASKED",
    at_or_below: 0.3,
    abilities: [
        Some(EnemyAbility::CrimsonFrenzy),
        Some(EnemyAbility::MarkOfTheHost),
        Some(EnemyAbility::DanceOfBlades),
        Some(EnemyAbility::GhastlyWaltz),
        Some(EnemyAbility::GougingRiposte),
        Some(EnemyAbility::RapierThrust),
    ],
    speed_scale: 1.05,
    cooldown_scale: 0.8,
    summon: None,
}];

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
    fn every_monster_id_is_unique_and_round_trips() {
        for kind in EnemyKind::ALL {
            let id = kind.id();
            assert!(!id.is_empty(), "{kind:?} has no id");
            assert!(
                id.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{id} is not lower_snake_case"
            );
            assert_eq!(EnemyKind::from_id(id), Some(kind));
            assert_eq!(EnemyKind::from_id(&id.to_uppercase()), Some(kind));
        }
        assert_eq!(EnemyKind::from_id("no_such_monster"), None);
    }

    #[test]
    fn every_empowered_form_is_empowered_and_every_normal_one_is_not() {
        for kind in EnemyKind::ALL {
            if let Some(empowered) = kind.empowered() {
                assert_eq!(
                    empowered.archetype().tier,
                    MonsterTier::Empowered,
                    "{kind:?}"
                );
                assert_eq!(kind.archetype().tier, MonsterTier::Normal, "{kind:?}");
                assert_eq!(
                    empowered.archetype().role,
                    kind.archetype().role,
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn what_a_cast_plans_fits_in_a_cast() {
        for ability in EnemyAbility::ALL {
            let definition = ability.definition();
            assert!(
                definition.zone_total() <= brain::MAX_ZONES,
                "{} lays more zones than a cast can hold",
                definition.name
            );
            assert!(
                usize::from(definition.targets) <= brain::MAX_TARGETS,
                "{}",
                definition.name
            );
            // Anything that aims at more than one thing, lands later or channels needs a
            // cast to plan it in: an instant has no plan. A charge is the exception — it is
            // instant, and its run is started on the tick it is chosen.
            let planned = definition.zone_total() > 1
                || definition.delay > 0.0
                || definition.effect.is_channel();
            if definition.effect.is_charge() {
                assert!(
                    !definition.is_cast(),
                    "{} is a charge with a wind-up",
                    definition.name
                );
            }
            assert!(
                !planned || definition.is_cast(),
                "{} needs a cast to be planned",
                definition.name
            );
            // A charge is cast on one player: nothing on the floor, and a slow when it lands.
            if definition.effect.is_charge() {
                assert!(
                    matches!(definition.shape, TelegraphShape::Single),
                    "{}",
                    definition.name
                );
                assert!(
                    definition.min_range > 0.0,
                    "{} charges from melee",
                    definition.name
                );
            }
            if let EnemyEffect::Channel { interval, .. } = definition.effect {
                assert!(
                    channel_ticks(definition.cast_time, interval) >= 2,
                    "{}",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn a_channel_that_grows_is_worth_its_ticks_summed() {
        let drain = EnemyAbility::GraveDrain.definition();
        // 10 growing by 0.35 of itself for eight ticks: 10 × (8 + 0.35 × 28).
        assert!(
            (drain.damage_per_use() - 178.0).abs() < 1e-3,
            "{}",
            drain.damage_per_use()
        );
        assert_eq!(channel_ticks(4.0, 0.5), 8);
    }

    #[test]
    fn a_level_scales_health_more_than_damage() {
        assert_eq!(level_scale(1), (1.0, 1.0));
        let (health, damage) = level_scale(4);
        assert!(health > damage && damage > 1.0);
        assert_eq!(level_scale(0), level_scale(1), "level zero is level one");
    }

    #[test]
    fn a_line_catches_what_is_along_it_and_nothing_beside_or_behind() {
        let line = TelegraphShape::Line {
            yaw: std::f32::consts::FRAC_PI_2,
            length: 10.0,
            width: 2.0,
        };
        // A quarter turn runs the line along -X.
        assert!(line.contains(Vec3::ZERO, Vec3::new(-5.0, 0.0, 0.0), 0.0));
        assert!(!line.contains(Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0), 0.0));
        assert!(!line.contains(Vec3::ZERO, Vec3::new(-5.0, 0.0, 2.0), 0.0));
        assert!(
            line.contains(Vec3::ZERO, Vec3::new(-5.0, 0.0, 1.4), 0.5),
            "an edge body"
        );
        assert!((line.reach() - 10.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_cast_is_a_cast_and_a_swing_is_not() {
        assert!(!EnemyAbility::Swing.definition().is_cast());
        assert!(EnemyAbility::GroundBlast.definition().is_cast());
        assert!(EnemyAbility::GroundBlast.definition().interruptible);
        assert!(!EnemyAbility::Swing.definition().interruptible);
    }
}
