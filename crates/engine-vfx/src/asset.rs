//! Effects as data: what an effect is made of, before anything plays it.
//!
//! ```text
//! VfxAsset
//! ├── emitters  (0..4)   particles: how many, how fast, how long, what colour
//! ├── layers    (0..4)   meshes: a disc, a ring, a cone, a beam, a shell, a column
//! ├── lifetime            how long the instance lives once spawned
//! └── follows_owner       whether it rides the entity that spawned it
//! ```
//!
//! The whole catalogue is a `static` table indexed by [`VfxId`], exactly as
//! `engine-game`'s ability table is: a designer edits one row, and the runtime, the demo
//! scene and the tests all read that row rather than restating it. Adding an effect is a
//! row and a variant, and touches no system.
//!
//! # What is deliberately not here
//!
//! * **No mesh, material or texture handles.** Those are issued by the renderer, and this
//!   crate cannot name one. A layer names a *shape* — [`VfxShape`] — and presentation binds
//!   shapes to uploaded geometry once, the same way `EnvironmentVisuals` binds environment
//!   assets.
//! * **No curves beyond a start and an end.** Every animated quantity is a linear ramp
//!   between two values over the life of the thing. A curve editor is a tool, and a tool is
//!   not what makes a fireball read; two endpoints and the right numbers are.
//! * **No dynamic lights.** The renderer has one directional light, and adding real point
//!   lights is a lighting-model change this phase has no business making. A "light" here is
//!   [`MeshLayer::emissive`] — a surface that briefly ignores the lighting — which is the
//!   mechanism `InstanceStyle` already provides for free.

use engine_render_data::ParticleBlend;

/// The most emitters one effect may declare.
///
/// Four is what the richest effect in the catalogue uses (a core, a spray, embers and
/// smoke). The bound exists so an instance can hold its per-emitter accumulators in a fixed
/// array rather than in a `Vec` it would have to allocate per spawn.
pub const MAX_EMITTERS: usize = 4;

/// The most mesh layers one effect may declare.
pub const MAX_LAYERS: usize = 4;

/// A piece of geometry an effect can draw.
///
/// Six shapes cover every effect in the class. They are *primitives with a convention*, not
/// models: each is authored around the origin, one metre in its natural size, so a layer
/// scales it by the gameplay number — a radius, a range, a width — and gets a shape that
/// matches what the ability actually hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VfxShape {
    /// A flat disc on the ground, radius 1. Area footprints and impact scorches.
    Disc,
    /// A flat annulus on the ground, outer radius 1. The bright edge of an area.
    Ring,
    /// A flat 60° wedge on the ground, radius 1, pointing along −Z at yaw 0. Cones.
    Cone,
    /// A flat strip on the ground, one wide and one long, anchored at its near end and
    /// running along −Z at yaw 0. Lines of fire and beams.
    Strip,
    /// A sphere of radius 1, centred on the origin. Bolts, cores, shields.
    Shell,
    /// A column one across and one tall, standing on the origin. Pillars and impact beams.
    Column,
    /// A flat unit quad in XZ, one across and one long, centred, facing up.
    ///
    /// One piece of a footprint that has been laid onto the terrain. Nothing in the effect
    /// library asks for this shape: presentation produces it when it cuts a ground footprint
    /// into short pieces so each can sit at the height under it. It is in this list because
    /// it is geometry the director has to have bound and pool like any other, and it is
    /// *last* so no existing shape's index moves.
    Quad,
}

impl VfxShape {
    /// Every shape, so presentation can bind them all in one loop.
    pub const ALL: [Self; Self::COUNT] = [
        Self::Disc,
        Self::Ring,
        Self::Cone,
        Self::Strip,
        Self::Shell,
        Self::Column,
        Self::Quad,
    ];

    /// How many shapes there are, for the arrays presentation indexes by shape.
    pub const COUNT: usize = 7;

    /// Dense index, for an array of bound geometry.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Lowercase name, for diagnostics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disc => "disc",
            Self::Ring => "ring",
            Self::Cone => "cone",
            Self::Strip => "strip",
            Self::Shell => "shell",
            Self::Column => "column",
            Self::Quad => "quad",
        }
    }

    /// Whether the shape lies on the ground and should be drawn just above it.
    ///
    /// [`Self::Quad`] answers `false`: a piece of a laid-out footprint has already been put
    /// exactly where it goes, and lifting it again would raise it twice.
    #[must_use]
    pub fn is_ground(self) -> bool {
        matches!(self, Self::Disc | Self::Ring | Self::Cone | Self::Strip)
    }

    /// Whether a ground footprint is the *edge* of an area rather than its interior.
    ///
    /// The distinction presentation needs to lay a footprint onto the terrain: an edge keeps
    /// the thin band it always was, and an interior becomes a wider band inside the same
    /// boundary rather than a plate over it. Meaningless for the shapes that stand up, which
    /// answer `false`.
    #[must_use]
    pub fn is_edge(self) -> bool {
        matches!(self, Self::Ring)
    }
}

/// Where in an effect's life a layer or emitter is active, as a fraction in `0..=1`.
///
/// A pair rather than a delay and a duration, because every question asked of it is "is
/// this thing showing at time t" and a fraction pair answers that with two compares.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// When it starts, as a fraction of the instance's lifetime.
    pub start: f32,
    /// When it ends, as a fraction of the instance's lifetime.
    pub end: f32,
}

impl Window {
    /// The whole life.
    pub const ALL: Self = Self {
        start: 0.0,
        end: 1.0,
    };

    /// A window from `start` to `end`.
    #[must_use]
    pub const fn new(start: f32, end: f32) -> Self {
        Self { start, end }
    }

    /// Whether `fraction` is inside the window.
    #[must_use]
    pub fn contains(&self, fraction: f32) -> bool {
        fraction >= self.start && fraction <= self.end
    }

    /// How far through the window `fraction` is, in `0..=1`.
    ///
    /// Zero-length windows report 1: a layer that exists for an instant is a layer that has
    /// finished, which is what a flash wants.
    #[must_use]
    pub fn progress(&self, fraction: f32) -> f32 {
        let span = self.end - self.start;
        if span <= 0.0 {
            return 1.0;
        }
        ((fraction - self.start) / span).clamp(0.0, 1.0)
    }
}

/// The volume an emitter's particles start inside, and which way they are thrown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpawnShape {
    /// All from the anchor, thrown in every direction.
    Point,
    /// From inside a sphere, thrown outwards from the centre.
    Sphere {
        /// Metres.
        radius: f32,
    },
    /// From a flat disc on the ground, thrown upwards and outwards.
    ///
    /// The radius is a *fraction of the instance's radius* rather than metres, so one
    /// emitter row serves a six-metre Meteor and a two-metre Detonate.
    Disc {
        /// Fraction of the instance's radius.
        radius: f32,
    },
    /// From the anchor, thrown into a cone of `half_angle` about the instance's facing.
    Cone {
        /// Radians.
        half_angle: f32,
    },
    /// Along the instance's forward axis, thrown sideways.
    ///
    /// The length is a fraction of the instance's length, which is how a band of fire fills
    /// exactly the strip the gameplay burns.
    Line {
        /// Fraction of the instance's length.
        length: f32,
        /// Fraction of the instance's radius, across the line.
        width: f32,
    },
}

/// One source of particles inside an effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmitterDesc {
    /// Particles thrown the moment the emitter opens.
    pub burst: u32,
    /// Particles thrown per second while it is open.
    pub rate: f32,
    /// When it is open, as a fraction of the instance's life.
    pub window: Window,
    /// Where the particles start and which way they go.
    pub shape: SpawnShape,
    /// Metres per second, low to high.
    pub speed: (f32, f32),
    /// Seconds a particle lives, low to high.
    pub lifetime: (f32, f32),
    /// Half-extent in metres at birth and at death.
    pub size: (f32, f32),
    /// Linear RGBA at birth.
    pub color_start: [f32; 4],
    /// Linear RGBA at death. The alpha ramp is what makes a particle fade.
    pub color_end: [f32; 4],
    /// Metres per second squared, downwards. Negative rises, which is what smoke does.
    pub gravity: f32,
    /// Fraction of speed shed per second. Zero coasts forever.
    pub drag: f32,
    /// How it is blended.
    pub blend: ParticleBlend,
    /// Radians per second a particle turns about the view axis, low to high.
    pub spin: (f32, f32),
    /// Whether the particles ride the instance or stay where they were born.
    ///
    /// A trail is `false`: the whole point of a trail is that it is left behind. An aura is
    /// `true`, so it moves with the target it is burning.
    pub attached: bool,
}

impl EmitterDesc {
    /// A one-shot burst with sensible defaults, for a row to override what it cares about.
    #[must_use]
    pub const fn burst(count: u32) -> Self {
        Self {
            burst: count,
            rate: 0.0,
            window: Window::new(0.0, 0.0),
            shape: SpawnShape::Point,
            speed: (1.0, 3.0),
            lifetime: (0.3, 0.6),
            size: (0.25, 0.0),
            color_start: [1.0, 0.6, 0.2, 1.0],
            color_end: [0.6, 0.1, 0.0, 0.0],
            gravity: 2.0,
            drag: 1.5,
            blend: ParticleBlend::Additive,
            spin: (-3.0, 3.0),
            attached: false,
        }
    }

    /// A continuous emitter at `rate` particles a second, open for the whole life.
    #[must_use]
    pub const fn stream(rate: f32) -> Self {
        Self {
            burst: 0,
            rate,
            window: Window::ALL,
            ..Self::burst(0)
        }
    }
}

/// How a mesh layer is oriented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerAlign {
    /// Flat on the ground, turned to the instance's yaw.
    Ground,
    /// Standing at the instance's position, turned to its yaw.
    Upright,
}

/// One piece of geometry inside an effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshLayer {
    /// What to draw.
    pub shape: VfxShape,
    /// When it is drawn, as a fraction of the instance's life.
    pub window: Window,
    /// How it sits in the world.
    pub align: LayerAlign,
    /// Multiplier on the instance's radius, at the start and end of the layer's window.
    ///
    /// A multiplier rather than metres, because an area's radius comes from the ability's
    /// own `radius` field: the layer decides that the bright edge is 6 % larger than the
    /// fill, not how big either of them is.
    pub radius: (f32, f32),
    /// Metres of height, at the start and end of the window. Ground shapes ignore it.
    pub height: (f32, f32),
    /// Linear RGBA tint, multiplied with whatever material presentation binds.
    pub color: [f32; 4],
    /// Emissive term added to the surface, at the start and end of the window.
    ///
    /// This is the whole of "a VFX light": a surface that ignores the scene lighting for as
    /// long as the ramp says so.
    pub emissive: (f32, f32),
    /// Radians per second the layer turns about +Y.
    pub spin: f32,
    /// Whether the layer's length is the instance's `length` rather than its radius.
    ///
    /// True for a strip, which is as long as the ability reaches and as wide as it burns.
    pub uses_length: bool,
}

impl MeshLayer {
    /// A ground disc for the whole life, with nothing animated.
    #[must_use]
    pub const fn disc() -> Self {
        Self {
            shape: VfxShape::Disc,
            window: Window::ALL,
            align: LayerAlign::Ground,
            radius: (1.0, 1.0),
            height: (0.0, 0.0),
            color: [1.0, 0.45, 0.15, 0.9],
            emissive: (0.9, 0.9),
            spin: 0.0,
            uses_length: false,
        }
    }
}

/// One effect, entirely as data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VfxAsset {
    /// Which effect this describes.
    pub id: VfxId,
    /// What it is, for diagnostics and the demo scene: `"FIREBOLT IMPACT"`.
    pub name: &'static str,
    /// Seconds an instance lives before it retires itself.
    ///
    /// An instance can be given a different lifetime at spawn — a burn lasts as long as the
    /// status does — but every effect has to declare one, because an effect with no end is
    /// a leak with a picture on it.
    pub lifetime: f32,
    /// Its particle sources.
    pub emitters: &'static [EmitterDesc],
    /// Its geometry.
    pub layers: &'static [MeshLayer],
}

impl VfxAsset {
    /// How many particles one instance of this effect will emit over its whole life.
    ///
    /// What the budget is computed from, and what the stress scenario is sized against. An
    /// estimate rather than a promise: a stream emitter's count depends on the lifetime the
    /// instance is actually given.
    #[must_use]
    pub fn estimated_particles(&self) -> u32 {
        self.emitters
            .iter()
            .map(|emitter| {
                let span = (emitter.window.end - emitter.window.start).max(0.0) * self.lifetime;
                emitter.burst + (emitter.rate * span) as u32
            })
            .sum()
    }
}

/// Which effect.
///
/// One variant per *reusable* effect, not per ability: eleven abilities share nine effects,
/// because a Detonate and a Meteor differ in radius and colour rather than in kind. The
/// mapping from an ability to an effect lives in presentation, which is where the decision
/// "this ability looks like that" belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum VfxId {
    /// The bolt itself: a bright core that flies and sheds embers.
    Projectile,
    /// A bolt arriving: a flash, a spray of sparks and a scorch that fades.
    Impact,
    /// A circular area burning on the ground for as long as gameplay says it does.
    GroundArea,
    /// A circular area that is *about* to be hit: the anticipation ring for a delayed rock.
    Telegraph,
    /// A cone of fire washing forward.
    ConeWave,
    /// A band of fire along a line, burning for a duration.
    LineFire,
    /// An explosion: a flash, an expanding shell and a lot of outward sparks.
    Explosion,
    /// A target burning, for as long as it is ignited.
    Ignited,
    /// The caster wrapped in a defensive shell.
    Shield,
    /// The caster gathering power: an aura that tightens while a cast runs.
    CastCharge,
    /// Somebody left from here.
    BlinkDeparture,
    /// Somebody arrived here.
    BlinkArrival,
    /// The Ultimate: the biggest thing in the catalogue.
    Cataclysm,
}

impl VfxId {
    /// Every effect, in table order.
    pub const ALL: [Self; EFFECT_COUNT] = [
        Self::Projectile,
        Self::Impact,
        Self::GroundArea,
        Self::Telegraph,
        Self::ConeWave,
        Self::LineFire,
        Self::Explosion,
        Self::Ignited,
        Self::Shield,
        Self::CastCharge,
        Self::BlinkDeparture,
        Self::BlinkArrival,
        Self::Cataclysm,
    ];

    /// Dense index into the effect table.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// This effect's definition.
    #[must_use]
    pub fn asset(self) -> &'static VfxAsset {
        &EFFECTS[self.index()]
    }

    /// What it is called.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.asset().name
    }
}

/// How many effects the catalogue holds.
pub const EFFECT_COUNT: usize = 13;

/// The largest multiple of an effect's radius that an *upright* layer may reach.
///
/// The world pipeline does not blend, so a standing mesh is a solid object in front of the
/// fight. Ground shapes are exempt — a footprint painted on the floor obscures the floor,
/// which is what it is for — but anything vertical has to stay small enough to read as a
/// core, a bolt or a pillar rather than as a wall. Milestone 19 found this the hard way:
/// an explosion whose flash was a sphere at the ability's own radius filled the screen.
pub const MAX_UPRIGHT_RADIUS: f32 = 0.6;

// The emitter and layer tables. Written out as consts so `EFFECTS` can borrow them for
// `'static`; a row references its emitters by name rather than inlining them, which keeps
// the effect table readable as a list of effects.

const PROJECTILE_EMITTERS: [EmitterDesc; 2] = [
    // The core's own glow, dense and short-lived, riding the bolt.
    EmitterDesc {
        rate: 140.0,
        window: Window::ALL,
        shape: SpawnShape::Sphere { radius: 0.12 },
        speed: (0.2, 0.8),
        lifetime: (0.10, 0.22),
        size: (0.30, 0.05),
        color_start: [1.0, 0.85, 0.45, 1.0],
        color_end: [1.0, 0.30, 0.05, 0.0],
        gravity: -0.4,
        drag: 2.0,
        attached: false,
        ..EmitterDesc::stream(140.0)
    },
    // Embers that fall out of the trail. Not attached, so they are left behind.
    EmitterDesc {
        rate: 45.0,
        window: Window::ALL,
        shape: SpawnShape::Sphere { radius: 0.2 },
        speed: (0.5, 1.6),
        lifetime: (0.25, 0.55),
        size: (0.10, 0.0),
        color_start: [1.0, 0.55, 0.15, 0.9],
        color_end: [0.5, 0.08, 0.0, 0.0],
        gravity: 3.0,
        drag: 1.0,
        attached: false,
        ..EmitterDesc::stream(45.0)
    },
];

const PROJECTILE_LAYERS: [MeshLayer; 1] = [MeshLayer {
    shape: VfxShape::Shell,
    window: Window::ALL,
    align: LayerAlign::Upright,
    radius: (0.35, 0.35),
    height: (0.35, 0.35),
    color: [1.0, 0.62, 0.20, 1.0],
    emissive: (2.2, 2.2),
    spin: 6.0,
    uses_length: false,
}];

const IMPACT_EMITTERS: [EmitterDesc; 2] = [
    EmitterDesc {
        burst: 26,
        shape: SpawnShape::Sphere { radius: 0.25 },
        speed: (2.5, 7.0),
        lifetime: (0.22, 0.5),
        size: (0.22, 0.0),
        color_start: [1.0, 0.85, 0.4, 1.0],
        color_end: [0.8, 0.15, 0.0, 0.0],
        gravity: 9.0,
        drag: 2.5,
        ..EmitterDesc::burst(26)
    },
    EmitterDesc {
        burst: 8,
        shape: SpawnShape::Sphere { radius: 0.35 },
        speed: (0.4, 1.4),
        lifetime: (0.5, 0.9),
        size: (0.45, 1.10),
        color_start: [0.25, 0.20, 0.18, 0.55],
        color_end: [0.10, 0.09, 0.09, 0.0],
        gravity: -0.8,
        drag: 2.0,
        blend: ParticleBlend::Alpha,
        ..EmitterDesc::burst(8)
    },
];

const IMPACT_LAYERS: [MeshLayer; 1] = [MeshLayer {
    shape: VfxShape::Shell,
    window: Window::new(0.0, 0.45),
    align: LayerAlign::Upright,
    // Well under `MAX_UPRIGHT_RADIUS`: the geometry is opaque, so a flash the size of the
    // thing it hit would hide the hit. The sparks carry the size; this is the core.
    radius: (0.18, 0.55),
    height: (0.18, 0.55),
    color: [1.0, 0.75, 0.35, 1.0],
    emissive: (2.6, 0.0),
    spin: 0.0,
    uses_length: false,
}];

const GROUND_AREA_EMITTERS: [EmitterDesc; 2] = [
    EmitterDesc {
        rate: 90.0,
        shape: SpawnShape::Disc { radius: 0.95 },
        speed: (1.0, 2.6),
        lifetime: (0.4, 0.9),
        size: (0.35, 0.05),
        color_start: [1.0, 0.55, 0.15, 0.9],
        color_end: [0.7, 0.10, 0.0, 0.0],
        gravity: -1.6,
        drag: 1.2,
        attached: true,
        ..EmitterDesc::stream(90.0)
    },
    EmitterDesc {
        rate: 14.0,
        shape: SpawnShape::Disc { radius: 0.8 },
        speed: (0.3, 1.0),
        lifetime: (0.9, 1.6),
        size: (0.6, 1.6),
        color_start: [0.22, 0.18, 0.16, 0.35],
        color_end: [0.10, 0.09, 0.09, 0.0],
        gravity: -1.2,
        drag: 1.6,
        blend: ParticleBlend::Alpha,
        attached: true,
        ..EmitterDesc::stream(14.0)
    },
];

const GROUND_AREA_LAYERS: [MeshLayer; 2] = [
    MeshLayer {
        color: [1.0, 0.40, 0.12, 0.75],
        emissive: (1.1, 1.1),
        ..MeshLayer::disc()
    },
    MeshLayer {
        shape: VfxShape::Ring,
        radius: (1.02, 1.02),
        color: [1.0, 0.70, 0.30, 0.95],
        emissive: (2.0, 2.0),
        spin: 0.35,
        ..MeshLayer::disc()
    },
];

const TELEGRAPH_LAYERS: [MeshLayer; 2] = [
    MeshLayer {
        color: [1.0, 0.35, 0.10, 0.35],
        emissive: (0.4, 1.4),
        ..MeshLayer::disc()
    },
    // A ring that closes as the rock falls: the anticipation *is* the shrinking radius, so
    // a player can read how long is left without a number.
    MeshLayer {
        shape: VfxShape::Ring,
        radius: (1.35, 1.0),
        color: [1.0, 0.80, 0.35, 0.95],
        emissive: (1.6, 2.6),
        ..MeshLayer::disc()
    },
];

const TELEGRAPH_EMITTERS: [EmitterDesc; 1] = [EmitterDesc {
    rate: 26.0,
    shape: SpawnShape::Disc { radius: 1.0 },
    speed: (0.6, 1.8),
    lifetime: (0.3, 0.7),
    size: (0.16, 0.0),
    color_start: [1.0, 0.65, 0.25, 0.8],
    color_end: [0.7, 0.15, 0.0, 0.0],
    gravity: -1.0,
    drag: 1.0,
    attached: true,
    ..EmitterDesc::stream(26.0)
}];

const CONE_EMITTERS: [EmitterDesc; 2] = [
    EmitterDesc {
        burst: 90,
        shape: SpawnShape::Cone {
            half_angle: std::f32::consts::FRAC_PI_6,
        },
        speed: (7.0, 15.0),
        lifetime: (0.35, 0.7),
        size: (0.45, 0.10),
        color_start: [1.0, 0.72, 0.28, 1.0],
        color_end: [0.8, 0.15, 0.0, 0.0],
        gravity: 0.6,
        drag: 2.2,
        ..EmitterDesc::burst(90)
    },
    EmitterDesc {
        burst: 18,
        shape: SpawnShape::Cone {
            half_angle: std::f32::consts::FRAC_PI_6,
        },
        speed: (3.0, 7.0),
        lifetime: (0.6, 1.1),
        size: (0.7, 1.6),
        color_start: [0.24, 0.19, 0.17, 0.4],
        color_end: [0.10, 0.09, 0.09, 0.0],
        gravity: -0.6,
        drag: 2.4,
        blend: ParticleBlend::Alpha,
        ..EmitterDesc::burst(18)
    },
];

/// No geometry: the cone is its fire alone.
///
/// A bordered wedge laid on the floor at the moment of the cast read as a second, different
/// shape from the aiming indicator the player had just released, so the footprint is said
/// once - by the indicator, before the cast - and the cast itself is the spray of particles.
const CONE_LAYERS: [MeshLayer; 0] = [];

const LINE_EMITTERS: [EmitterDesc; 2] = [
    EmitterDesc {
        rate: 150.0,
        shape: SpawnShape::Line {
            length: 1.0,
            width: 1.0,
        },
        speed: (1.4, 3.4),
        lifetime: (0.4, 0.85),
        size: (0.36, 0.05),
        color_start: [1.0, 0.58, 0.18, 0.95],
        color_end: [0.7, 0.10, 0.0, 0.0],
        gravity: -2.2,
        drag: 1.1,
        attached: true,
        ..EmitterDesc::stream(150.0)
    },
    EmitterDesc {
        rate: 20.0,
        shape: SpawnShape::Line {
            length: 1.0,
            width: 0.9,
        },
        speed: (0.4, 1.2),
        lifetime: (0.9, 1.7),
        size: (0.7, 1.8),
        color_start: [0.22, 0.18, 0.16, 0.35],
        color_end: [0.10, 0.09, 0.09, 0.0],
        gravity: -1.4,
        drag: 1.6,
        blend: ParticleBlend::Alpha,
        attached: true,
        ..EmitterDesc::stream(20.0)
    },
];

const LINE_LAYERS: [MeshLayer; 1] = [MeshLayer {
    shape: VfxShape::Strip,
    color: [1.0, 0.42, 0.12, 0.7],
    emissive: (1.3, 1.3),
    uses_length: true,
    ..MeshLayer::disc()
}];

const EXPLOSION_EMITTERS: [EmitterDesc; 3] = [
    EmitterDesc {
        burst: 120,
        shape: SpawnShape::Disc { radius: 0.15 },
        speed: (9.0, 20.0),
        lifetime: (0.3, 0.75),
        size: (0.45, 0.05),
        color_start: [1.0, 0.85, 0.45, 1.0],
        color_end: [0.85, 0.15, 0.0, 0.0],
        gravity: 6.0,
        drag: 2.6,
        ..EmitterDesc::burst(120)
    },
    EmitterDesc {
        burst: 40,
        shape: SpawnShape::Sphere { radius: 0.4 },
        speed: (1.0, 4.0),
        lifetime: (0.5, 1.0),
        size: (0.8, 0.1),
        color_start: [1.0, 0.55, 0.15, 0.9],
        color_end: [0.6, 0.08, 0.0, 0.0],
        gravity: -3.0,
        drag: 1.8,
        ..EmitterDesc::burst(40)
    },
    EmitterDesc {
        burst: 22,
        shape: SpawnShape::Disc { radius: 0.5 },
        speed: (1.5, 4.5),
        lifetime: (0.8, 1.6),
        size: (1.0, 2.4),
        color_start: [0.26, 0.21, 0.18, 0.5],
        color_end: [0.10, 0.09, 0.09, 0.0],
        gravity: -1.6,
        drag: 2.2,
        blend: ParticleBlend::Alpha,
        ..EmitterDesc::burst(22)
    },
];

const EXPLOSION_LAYERS: [MeshLayer; 2] = [
    MeshLayer {
        shape: VfxShape::Shell,
        window: Window::new(0.0, 0.35),
        align: LayerAlign::Upright,
        // A core rather than a dome. An opaque sphere at the explosion's full radius would
        // fill the screen for a third of a second, which is the one thing an effect at the
        // centre of a fight must never do — the expanding ring below carries the size.
        radius: (0.12, 0.45),
        height: (0.12, 0.45),
        color: [1.0, 0.72, 0.32, 1.0],
        emissive: (3.0, 0.0),
        spin: 0.0,
        uses_length: false,
    },
    MeshLayer {
        shape: VfxShape::Ring,
        window: Window::new(0.05, 1.0),
        radius: (0.3, 1.25),
        color: [1.0, 0.55, 0.20, 0.8],
        emissive: (2.2, 0.0),
        ..MeshLayer::disc()
    },
];

const IGNITED_EMITTERS: [EmitterDesc; 1] = [EmitterDesc {
    rate: 22.0,
    shape: SpawnShape::Sphere { radius: 0.35 },
    speed: (0.3, 0.9),
    lifetime: (0.4, 0.8),
    size: (0.16, 0.02),
    color_start: [1.0, 0.52, 0.14, 0.85],
    color_end: [0.65, 0.10, 0.0, 0.0],
    gravity: -1.8,
    drag: 1.4,
    attached: true,
    ..EmitterDesc::stream(22.0)
}];

// A ring on the ground and a skin of sparks — deliberately **not** a shell around the
// character. The world pass is opaque, so a bubble at body radius would hide the player
// inside their own defensive cooldown: the state would be unreadable at exactly the moment
// it matters. The particles say "something is around me"; the ring says "it is mine".
const SHIELD_LAYERS: [MeshLayer; 1] = [MeshLayer {
    shape: VfxShape::Ring,
    radius: (1.15, 1.15),
    color: [1.0, 0.55, 0.22, 0.8],
    emissive: (1.4, 1.4),
    spin: 0.9,
    ..MeshLayer::disc()
}];

const SHIELD_EMITTERS: [EmitterDesc; 1] = [EmitterDesc {
    rate: 30.0,
    shape: SpawnShape::Sphere { radius: 0.9 },
    speed: (0.2, 0.7),
    lifetime: (0.3, 0.7),
    size: (0.14, 0.02),
    color_start: [1.0, 0.70, 0.30, 0.8],
    color_end: [0.8, 0.25, 0.05, 0.0],
    gravity: -0.6,
    drag: 1.0,
    attached: true,
    ..EmitterDesc::stream(30.0)
}];

const CAST_EMITTERS: [EmitterDesc; 1] = [EmitterDesc {
    rate: 55.0,
    shape: SpawnShape::Sphere { radius: 0.75 },
    // Inward: a negative speed range throws particles towards the anchor, which is what
    // "gathering" looks like and what tells a player a cast is filling rather than leaking.
    speed: (-3.0, -1.2),
    lifetime: (0.25, 0.5),
    size: (0.16, 0.03),
    color_start: [1.0, 0.65, 0.25, 0.85],
    color_end: [1.0, 0.85, 0.55, 0.0],
    gravity: -0.5,
    drag: 0.4,
    attached: true,
    ..EmitterDesc::stream(55.0)
}];

const BLINK_DEPARTURE_EMITTERS: [EmitterDesc; 1] = [EmitterDesc {
    burst: 40,
    shape: SpawnShape::Sphere { radius: 0.5 },
    speed: (0.5, 3.0),
    lifetime: (0.3, 0.6),
    size: (0.22, 0.0),
    color_start: [1.0, 0.60, 0.30, 0.9],
    color_end: [0.5, 0.10, 0.0, 0.0],
    gravity: 1.5,
    drag: 2.5,
    ..EmitterDesc::burst(40)
}];

const BLINK_DEPARTURE_LAYERS: [MeshLayer; 1] = [MeshLayer {
    shape: VfxShape::Column,
    align: LayerAlign::Upright,
    radius: (0.5, 0.15),
    height: (2.0, 2.6),
    color: [1.0, 0.55, 0.25, 0.7],
    emissive: (2.0, 0.0),
    ..MeshLayer::disc()
}];

const BLINK_ARRIVAL_EMITTERS: [EmitterDesc; 1] = [EmitterDesc {
    burst: 55,
    shape: SpawnShape::Sphere { radius: 0.7 },
    speed: (-4.0, -1.0),
    lifetime: (0.25, 0.5),
    size: (0.20, 0.0),
    color_start: [1.0, 0.75, 0.40, 1.0],
    color_end: [0.7, 0.20, 0.0, 0.0],
    gravity: 0.5,
    drag: 1.0,
    ..EmitterDesc::burst(55)
}];

const BLINK_ARRIVAL_LAYERS: [MeshLayer; 1] = [MeshLayer {
    shape: VfxShape::Ring,
    radius: (1.4, 0.3),
    color: [1.0, 0.70, 0.35, 0.9],
    emissive: (2.4, 0.0),
    ..MeshLayer::disc()
}];

const CATACLYSM_EMITTERS: [EmitterDesc; 3] = [
    // The column of fire, for the whole life.
    EmitterDesc {
        rate: 260.0,
        shape: SpawnShape::Disc { radius: 0.5 },
        speed: (4.0, 12.0),
        lifetime: (0.5, 1.1),
        size: (0.55, 0.05),
        color_start: [1.0, 0.80, 0.40, 1.0],
        color_end: [0.85, 0.15, 0.0, 0.0],
        gravity: -5.0,
        drag: 1.2,
        attached: true,
        ..EmitterDesc::stream(260.0)
    },
    // The outward blast, once, at the moment it lands.
    EmitterDesc {
        burst: 200,
        window: Window::new(0.25, 0.25),
        shape: SpawnShape::Disc { radius: 0.2 },
        speed: (12.0, 26.0),
        lifetime: (0.4, 0.9),
        size: (0.5, 0.05),
        color_start: [1.0, 0.85, 0.45, 1.0],
        color_end: [0.85, 0.12, 0.0, 0.0],
        gravity: 7.0,
        drag: 2.4,
        ..EmitterDesc::burst(200)
    },
    EmitterDesc {
        rate: 26.0,
        shape: SpawnShape::Disc { radius: 0.9 },
        speed: (1.0, 3.0),
        lifetime: (1.2, 2.2),
        size: (1.4, 3.2),
        color_start: [0.26, 0.21, 0.18, 0.45],
        color_end: [0.10, 0.09, 0.09, 0.0],
        gravity: -2.0,
        drag: 1.8,
        blend: ParticleBlend::Alpha,
        attached: true,
        ..EmitterDesc::stream(26.0)
    },
];

const CATACLYSM_LAYERS: [MeshLayer; 3] = [
    // Anticipation: a ring that closes over the first quarter of the effect.
    MeshLayer {
        shape: VfxShape::Ring,
        window: Window::new(0.0, 0.25),
        radius: (1.5, 1.0),
        color: [1.0, 0.70, 0.30, 0.9],
        emissive: (1.4, 2.8),
        ..MeshLayer::disc()
    },
    // The pillar.
    MeshLayer {
        shape: VfxShape::Column,
        window: Window::new(0.25, 1.0),
        align: LayerAlign::Upright,
        // A pillar at the full radius would be an opaque wall across the arena. A third of
        // it reads as a column of fire and leaves the fight behind it visible.
        radius: (0.3, 0.2),
        height: (14.0, 4.0),
        color: [1.0, 0.55, 0.20, 0.65],
        emissive: (2.6, 0.4),
        spin: 1.5,
        uses_length: false,
    },
    // The scorch it leaves.
    MeshLayer {
        window: Window::new(0.25, 1.0),
        radius: (1.0, 1.0),
        color: [0.9, 0.32, 0.10, 0.8],
        emissive: (1.6, 0.2),
        ..MeshLayer::disc()
    },
];

/// Every effect, in [`VfxId`] order.
pub static EFFECTS: [VfxAsset; EFFECT_COUNT] = [
    VfxAsset {
        id: VfxId::Projectile,
        name: "PROJECTILE",
        // Overridden at spawn: a bolt lives exactly as long as its flight takes, which
        // depends on how far away the target was.
        lifetime: 0.6,
        emitters: &PROJECTILE_EMITTERS,
        layers: &PROJECTILE_LAYERS,
    },
    VfxAsset {
        id: VfxId::Impact,
        name: "IMPACT",
        lifetime: 0.55,
        emitters: &IMPACT_EMITTERS,
        layers: &IMPACT_LAYERS,
    },
    VfxAsset {
        id: VfxId::GroundArea,
        name: "GROUND AREA",
        // Overridden at spawn with the zone's own duration.
        lifetime: 6.0,
        emitters: &GROUND_AREA_EMITTERS,
        layers: &GROUND_AREA_LAYERS,
    },
    VfxAsset {
        id: VfxId::Telegraph,
        name: "TELEGRAPH",
        // Overridden at spawn with the impact's own delay.
        lifetime: 1.2,
        emitters: &TELEGRAPH_EMITTERS,
        layers: &TELEGRAPH_LAYERS,
    },
    VfxAsset {
        id: VfxId::ConeWave,
        name: "CONE WAVE",
        lifetime: 0.7,
        emitters: &CONE_EMITTERS,
        layers: &CONE_LAYERS,
    },
    VfxAsset {
        id: VfxId::LineFire,
        name: "LINE FIRE",
        lifetime: 6.0,
        emitters: &LINE_EMITTERS,
        layers: &LINE_LAYERS,
    },
    VfxAsset {
        id: VfxId::Explosion,
        name: "EXPLOSION",
        lifetime: 0.9,
        emitters: &EXPLOSION_EMITTERS,
        layers: &EXPLOSION_LAYERS,
    },
    VfxAsset {
        id: VfxId::Ignited,
        name: "IGNITED",
        // Overridden at spawn with what is left of the burn.
        lifetime: 6.0,
        emitters: &IGNITED_EMITTERS,
        layers: &[],
    },
    VfxAsset {
        id: VfxId::Shield,
        name: "SHIELD",
        lifetime: 8.0,
        emitters: &SHIELD_EMITTERS,
        layers: &SHIELD_LAYERS,
    },
    VfxAsset {
        id: VfxId::CastCharge,
        name: "CAST CHARGE",
        lifetime: 1.0,
        emitters: &CAST_EMITTERS,
        layers: &[],
    },
    VfxAsset {
        id: VfxId::BlinkDeparture,
        name: "BLINK DEPARTURE",
        lifetime: 0.45,
        emitters: &BLINK_DEPARTURE_EMITTERS,
        layers: &BLINK_DEPARTURE_LAYERS,
    },
    VfxAsset {
        id: VfxId::BlinkArrival,
        name: "BLINK ARRIVAL",
        lifetime: 0.5,
        emitters: &BLINK_ARRIVAL_EMITTERS,
        layers: &BLINK_ARRIVAL_LAYERS,
    },
    VfxAsset {
        id: VfxId::Cataclysm,
        name: "CATACLYSM",
        lifetime: 4.0,
        emitters: &CATACLYSM_EMITTERS,
        layers: &CATACLYSM_LAYERS,
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
    fn every_effect_sits_in_the_slot_its_id_names() {
        // The table is indexed by the enum, so a row in the wrong place would silently give
        // an ability somebody else's effect.
        for id in VfxId::ALL {
            assert_eq!(id.asset().id, id, "{} is in the wrong slot", id.name());
        }
        assert_eq!(EFFECTS.len(), EFFECT_COUNT);
    }

    #[test]
    fn every_effect_ends() {
        // An effect with no lifetime is a leak with a picture on it.
        for id in VfxId::ALL {
            let asset = id.asset();
            assert!(asset.lifetime > 0.0, "{} lives forever", asset.name);
        }
    }

    #[test]
    fn no_effect_declares_more_than_the_runtime_can_hold() {
        // An instance keeps a fixed-size accumulator per emitter, so the bound is not
        // advisory.
        for id in VfxId::ALL {
            let asset = id.asset();
            assert!(
                asset.emitters.len() <= MAX_EMITTERS,
                "{} has {} emitters",
                asset.name,
                asset.emitters.len()
            );
            assert!(
                asset.layers.len() <= MAX_LAYERS,
                "{} has {} layers",
                asset.name,
                asset.layers.len()
            );
        }
    }

    #[test]
    fn every_effect_actually_shows_something() {
        for id in VfxId::ALL {
            let asset = id.asset();
            assert!(
                !asset.emitters.is_empty() || !asset.layers.is_empty(),
                "{} draws nothing at all",
                asset.name
            );
        }
    }

    #[test]
    fn every_particle_fades_out_rather_than_vanishing() {
        // A particle whose end alpha is not zero pops out of existence at the end of its
        // life, which is the single most obvious way for an effect to look cheap.
        for id in VfxId::ALL {
            for emitter in id.asset().emitters {
                assert_eq!(
                    emitter.color_end[3],
                    0.0,
                    "{} has an emitter that ends opaque",
                    id.name()
                );
                assert!(
                    emitter.lifetime.0 > 0.0 && emitter.lifetime.1 >= emitter.lifetime.0,
                    "{} has an emitter with an impossible lifetime",
                    id.name()
                );
            }
        }
    }

    #[test]
    fn windows_are_ordered_and_inside_a_life() {
        for id in VfxId::ALL {
            for emitter in id.asset().emitters {
                let window = emitter.window;
                assert!(
                    window.start >= 0.0 && window.end <= 1.0 && window.start <= window.end,
                    "{} has an emitter window of {window:?}",
                    id.name()
                );
            }
            for layer in id.asset().layers {
                let window = layer.window;
                assert!(
                    window.start >= 0.0 && window.end <= 1.0 && window.start <= window.end,
                    "{} has a layer window of {window:?}",
                    id.name()
                );
            }
        }
    }

    #[test]
    fn no_upright_layer_is_wide_enough_to_hide_the_fight() {
        // The readability rule, checkable. Geometry is opaque, so a standing mesh at an
        // ability's full radius is a wall between the player and what they are fighting.
        for id in VfxId::ALL {
            for layer in id.asset().layers {
                if layer.align != LayerAlign::Upright {
                    continue;
                }
                let widest = layer.radius.0.max(layer.radius.1);
                assert!(
                    widest <= MAX_UPRIGHT_RADIUS,
                    "{} has an upright layer reaching {widest} of its radius",
                    id.name()
                );
            }
        }
    }

    #[test]
    fn nothing_wraps_the_character_in_an_opaque_shell() {
        // The same rule at the place it was actually violated: a defensive cooldown drawn as
        // a bubble hid the player inside it.
        for id in [VfxId::Shield, VfxId::CastCharge, VfxId::Ignited] {
            assert!(
                id.asset()
                    .layers
                    .iter()
                    .all(|layer| layer.shape != VfxShape::Shell),
                "{} wraps its owner in a shell",
                id.name()
            );
        }
    }

    #[test]
    fn the_estimate_counts_bursts_and_streams() {
        let asset = VfxId::Explosion.asset();
        assert_eq!(asset.estimated_particles(), 120 + 40 + 22);

        // A stream over a whole life is its rate times that life.
        let ignited = VfxId::Ignited.asset();
        assert_eq!(ignited.estimated_particles(), (22.0 * 6.0) as u32);
    }

    #[test]
    fn a_window_reports_where_in_itself_a_moment_falls() {
        let window = Window::new(0.25, 0.75);
        assert!(!window.contains(0.1));
        assert!(window.contains(0.5));
        assert_eq!(window.progress(0.25), 0.0);
        assert_eq!(window.progress(0.5), 0.5);
        assert_eq!(window.progress(1.0), 1.0);
        // An instant window has finished the moment it exists, which is what a flash wants.
        assert_eq!(Window::new(0.3, 0.3).progress(0.3), 1.0);
    }

    #[test]
    fn ground_shapes_are_the_ones_that_lie_flat() {
        assert!(VfxShape::Disc.is_ground());
        assert!(VfxShape::Strip.is_ground());
        assert!(!VfxShape::Shell.is_ground());
        assert!(!VfxShape::Column.is_ground());
        for (index, shape) in VfxShape::ALL.iter().enumerate() {
            assert_eq!(shape.index(), index);
        }
    }
}
