//! Combat, drawn: the layer that decides what an ability looks like.
//!
//! ```text
//! Simulation ─ events ─▶ VfxDirector ─▶ VfxSystem ─┬─▶ ParticleDrawList ─▶ particle pass
//!             (observe)   (decides)                └─▶ MeshDraw ─▶ RenderWorld entities
//! ```
//!
//! # What lives here and what does not
//!
//! `engine-vfx` knows what an explosion is *made of*. This module knows that **Detonate**
//! looks like one, how big it is, and what colour. That split is the same one the rest of
//! the project uses: `engine-game` owns the mechanic, `engine-assets` owns the shape, and
//! presentation is the only place allowed to say "this gameplay thing looks like that".
//!
//! Everything below is driven by *observing* [`CombatEvent`]s, exactly as the damage meter
//! and the floating numbers are. Nothing in `engine-game` knows the director exists, and no
//! gameplay function is called to produce an effect. Deleting this file would leave a game
//! that plays identically and shows nothing.
//!
//! # The two ways an effect starts
//!
//! * **An event.** A cast landed, a rock is falling, a burn was applied, somebody blinked.
//!   These are moments, and a moment is what an event is.
//! * **A state.** Flame Shield is up; a cast is running. These are not moments — a shield
//!   that started before the player alt-tabbed is still up — so they are *synchronised*
//!   each frame against the simulation, spawning when the state begins and stopping when it
//!   ends. Doing this from events alone would need a second copy of the state machine here,
//!   and the copy would be the thing that drifted.
//!
//! # Geometry is gameplay's, never the artist's
//!
//! Every radius, length and angle handed to an effect comes from the ability table or from
//! the [`ZoneShape`] the event carried — the same numbers the executor hits with. A cone
//! effect is `CONE_HALF_ANGLE` wide because that is what the cone catches. There is no
//! second set of numbers to drift.
//!
//! # Meshes are opaque, particles are not
//!
//! The world pipeline does not blend, so an effect's geometry cannot be translucent. A
//! layer's alpha is therefore folded into its *brightness*: a fading ring dims rather than
//! becoming see-through. Everything that genuinely needs translucency — fire, embers, smoke
//! — is a particle, and the particle pass blends. This is a real limitation and it is the
//! reason the geometry is used for footprints and shells rather than for the fire itself.
//!
//! # A footprint is laid on the terrain, not on a plane
//!
//! Every flat shape — [`VfxShape::Disc`], [`VfxShape::Ring`], [`VfxShape::Cone`],
//! [`VfxShape::Strip`] — used to be one horizontal quad or disc a few centimetres above the
//! effect's own `y`. On real ground that is a plate: half of a burning zone buried in the
//! hillside and the other half hanging over it. So a flat shape is no longer drawn as itself.
//! It is handed to [`engine_presentation::ui::ground_decal`](crate::ui::ground_decal) — the
//! same builder the targeting indicator uses — which walks its boundary, cuts it into short
//! pieces, and puts each piece at the height under it, tilted to the slope it spans. Each
//! piece is a [`VfxShape::Quad`] instance, and because the whole system shares one material
//! every piece of every footprint in the frame collapses into a single instanced draw.
//!
//! **An interior becomes a band, not a plate.** A [`VfxShape::Ring`] was already an edge and
//! stays one, at the thickness it always had. A disc, wedge or strip becomes a wide band just
//! inside the same boundary, which leaves the ground it marks visible — the thing an opaque
//! plate could never do, and the reason the fire itself is particles.
//!
//! **The budget is a frame, not an effect.** A footprint costs up to
//! [`ground_decal::MAX_SEGMENTS`](crate::ui::ground_decal::MAX_SEGMENTS) pieces, and a busy
//! fight can have thirty zones and telegraphs at once. Past [`MAX_BAND_PIECES`] in a frame the
//! remaining footprints fall back to the old flat shape rather than being cut short: a plate
//! on a hillside is wrong, and half an outline is worse.

use engine_core::math::{Sphere, Vec3};
use engine_core::transform::Transform;
use engine_game::enemy::TelegraphShape;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::{FireAbility, ZoneShape};
use engine_game::sim::Simulation;
use engine_render_data::{
    InstanceStyle, MaterialHandle, MeshHandle, ParticleDrawList, RenderEntity, RenderEntityDesc,
    RenderWorld,
};
use engine_vfx::{MeshDraw, VfxHandle, VfxId, VfxShape, VfxSpawn, VfxStats, VfxSystem};

use crate::ui::ground_decal::{self, DecalBand, DecalScratch, GroundSurface};
use crate::ui::targeting::TargetShape;

/// How far above the ground a flat effect is drawn, in metres.
///
/// Coplanar with the floor is a depth-buffer tie and flickers. Slightly higher than the
/// targeting indicator's own lift, so an area that is burning draws over the circle that
/// placed it rather than fighting it.
const GROUND_LIFT: f32 = 0.06;

/// The most pieces every footprint in one frame may use between them.
///
/// A per-frame budget rather than a per-effect one, because the cost that matters is the
/// frame's: one zone is a hundred pieces and thirty zones would be three thousand. Past this
/// the remaining footprints are drawn as the flat shapes they used to be — a whole plate that
/// fits the ground badly, rather than a fraction of an outline that has a hole in it.
///
/// At a thousand, four burning zones and a handful of telegraphs are all laid properly, which
/// is more than a fight has at once.
pub const MAX_BAND_PIECES: usize = 1024;

/// How thick an edge footprint's band is, as a fraction of its radius.
///
/// The ring mesh it replaces is an annulus of 6% of its own radius, so this is what keeps a
/// ring the thickness it has always been at every size.
const EDGE_BAND_FRACTION: f32 = 0.06;

/// How thick an interior footprint's band is, as a fraction of its extent.
///
/// Wide enough to read as the area being marked rather than as a second outline, narrow
/// enough to leave the middle of the zone showing the ground.
const FILL_BAND_FRACTION: f32 = 0.22;

/// The thinnest band, in metres, so a small footprint is still visible at all.
const MIN_BAND_WIDTH: f32 = 0.18;

/// The most of a footprint's own extent a band may take, as a fraction.
///
/// A band wider than this on a small footprint would close over the middle and be the opaque
/// plate this whole path exists to stop drawing.
const MAX_BAND_SHARE: f32 = 0.55;

/// How fast a cosmetic bolt travels, in metres per second.
///
/// Fast enough that the visual never lags the damage by long — the hit has already been
/// applied when the bolt leaves — and slow enough to read as a thrown object. At 35 m, the
/// class's single-target range, this is just under half a second.
pub const BOLT_SPEED: f32 = 80.0;

/// The longest a cosmetic bolt may take, in seconds.
///
/// A cap rather than a pure distance-over-speed, because the damage number is already on
/// screen: a bolt that took a second to catch up would read as a second, unrelated hit.
pub const BOLT_MAX_FLIGHT: f32 = 0.45;

/// The shortest, so a point-blank cast still shows something leaving the caster.
pub const BOLT_MIN_FLIGHT: f32 = 0.06;

/// How high above its feet a character's effects are centred, as a fraction of its height.
pub const CHEST_FRACTION: f32 = 0.6;

/// Radius of the effect that plays when a character lands, in metres.
const LANDING_RADIUS: f32 = 0.9;

/// Falling speed above which a landing shows dust, in metres per second.
///
/// Walking off a step is not an impact. This is about a metre of drop.
pub const LANDING_SPEED: f32 = 4.5;

/// Colour of dust, which is the one effect in the class that is not fire.
const DUST_TINT: [f32; 4] = [0.55, 0.50, 0.44, 0.7];

/// A blocked attack: the pale flash of a shield taking a blow.
const BLOCKED_TINT: [f32; 4] = [0.80, 0.82, 0.88, 0.9];

/// A parried or dodged attack: cooler and thinner, because nothing touched the character.
const AVOIDED_TINT: [f32; 4] = [0.62, 0.78, 0.95, 0.7];

/// Deflection being spent: the resource's own colour, and the one moment it is worth
/// drawing.
const DEFLECTION_TINT: [f32; 4] = [0.95, 0.82, 0.35, 1.0];

/// Puncture Blood's shell.
const ABSORB_TINT: [f32; 4] = [0.85, 0.25, 0.30, 0.85];

/// A spell going back where it came from.
const REFLECT_TINT: [f32; 4] = [0.70, 0.55, 0.95, 1.0];

/// A stun or a silence landing.
const CONTROL_TINT: [f32; 4] = [0.95, 0.90, 0.55, 0.9];

/// Colour of the mage's own defensive shell.
const SHIELD_TINT: [f32; 4] = [1.0, 0.62, 0.30, 1.0];

/// Colour of the Fire Mastery aura, which reuses the shell at a different size and hue.
const MASTERY_TINT: [f32; 4] = [1.0, 0.90, 0.45, 1.0];

/// A line telegraph — a spear, a volley, a charge's path. Bone-white, so a line on the floor
/// never reads as the player's own line of fire.
const LINE_TELEGRAPH_TINT: [f32; 4] = [0.92, 0.90, 0.82, 0.75];

/// A mark that follows a player: violet, the one colour on the floor that moves.
const MARK_TINT: [f32; 4] = [0.72, 0.35, 1.0, 0.9];

/// The sealed boss gate: a violet wall of light across the doorway.
const GATE_TINT: [f32; 4] = [0.55, 0.30, 0.95, 0.85];

/// An add arriving, and the gate breaking.
const CRYPT_TINT: [f32; 4] = [0.65, 0.35, 0.95, 1.0];

/// One uploaded shape, and the bounds the culler tests it with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeBinding {
    /// The uploaded geometry.
    pub mesh: MeshHandle,
    /// The mesh's local-space bounds.
    pub bounds: Sphere,
}

/// The geometry and material every effect is drawn with.
///
/// Bound once at startup, exactly as [`crate::EnvironmentVisuals`] binds the environment
/// library: six meshes and **one** material for the whole system. One material is what lets
/// the render list group every effect in the frame into six batches at most, whatever is
/// exploding.
///
/// The meshes have a contract the director relies on, and the app that uploads them has to
/// honour it:
///
/// | Shape | Authored as |
/// | --- | --- |
/// | `Disc` | radius 1 on the XZ plane at y = 0 |
/// | `Ring` | outer radius 1 on the XZ plane at y = 0 |
/// | `Cone` | radius 1 wedge of the gameplay half-angle, pointing along −Z |
/// | `Strip` | 1 wide, 1 long, anchored at the origin, running along −Z |
/// | `Shell` | radius 1, centred on the origin |
/// | `Column` | 1 across and 1 tall, centred on the origin |
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VfxVisuals {
    shapes: [Option<ShapeBinding>; VfxShape::COUNT],
    material: MaterialHandle,
}

impl VfxVisuals {
    /// An empty binding set drawing with `material`.
    #[must_use]
    pub fn new(material: MaterialHandle) -> Self {
        Self {
            shapes: [None; VfxShape::COUNT],
            material,
        }
    }

    /// Binds one shape to uploaded geometry.
    pub fn bind(&mut self, shape: VfxShape, mesh: MeshHandle, bounds: Sphere) {
        self.shapes[shape.index()] = Some(ShapeBinding { mesh, bounds });
    }

    /// What a shape is drawn with, if anything has been bound.
    #[must_use]
    pub fn get(&self, shape: VfxShape) -> Option<ShapeBinding> {
        self.shapes[shape.index()]
    }

    /// The one material every effect shares.
    #[must_use]
    pub fn material(&self) -> MaterialHandle {
        self.material
    }

    /// How many shapes have geometry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shapes.iter().filter(|shape| shape.is_some()).count()
    }

    /// Whether nothing has been bound. A director with none of it draws no geometry and
    /// still runs its particles.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A cosmetic bolt in flight.
///
/// Gameplay is hitscan and has already applied the damage; this is the picture catching up.
#[derive(Debug, Clone, Copy)]
struct Bolt {
    handle: VfxHandle,
    from: Vec3,
    to: Vec3,
    elapsed: f32,
    duration: f32,
    /// What plays when it lands.
    impact_radius: f32,
}

/// What the director is holding, for a readout and for the budget.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct VfxDirectorStats {
    /// What the runtime holds.
    pub system: VfxStats,
    /// Render entities the geometry is currently using.
    pub entities: usize,
    /// Bolts in flight.
    pub bolts: usize,
    /// Effects the director asked for and the system refused.
    pub refused: u64,
}

/// Turns combat into effects.
#[derive(Debug)]
pub struct VfxDirector {
    system: VfxSystem,
    visuals: VfxVisuals,
    /// Render entities per shape, reused across frames and hidden when unused.
    ///
    /// Per shape because geometry is fixed at spawn: an entity that drew a disc last frame
    /// cannot draw a shell this frame without being despawned and respawned, which is the
    /// slot churn `InstanceStyle` exists to avoid.
    pools: [Vec<RenderEntity>; VfxShape::COUNT],
    /// Working buffers for laying a footprint onto the terrain, reused every frame.
    ///
    /// Owned here so a fight full of burning ground allocates nothing: they grow to the
    /// largest footprint the session has drawn and then stay that size.
    band_scratch: DecalScratch,
    /// Last frame's laid bands, by position in the frame's draw order.
    ///
    /// A footprint that has not moved produces the same pieces over the same ground, and
    /// building them is trigonometry per piece over a hundred pieces per zone. Kept across
    /// frames and never freed, like every other scratch here.
    band_cache: Vec<CachedBand>,
    /// This frame's particles, rebuilt every frame and never freed.
    particles: ParticleDrawList,
    /// One draw list per worker, for a parallel particle write. Kept across frames so the
    /// pass allocates nothing once the peak has been seen; empty while the write is serial,
    /// which it is in every scene the game has today.
    particle_scratch: Vec<ParticleDrawList>,
    bolts: Vec<Bolt>,
    /// The burn playing on each target, by entity slot.
    ignited: Vec<Option<(u32, VfxHandle)>>,
    /// The area playing for each live zone, by the gameplay id the event carried.
    areas: Vec<(u32, VfxHandle)>,
    /// The shape on the floor for each enemy attack that is coming, by telegraph id.
    ///
    /// The shape is kept beside the handle so that when the attack lands, the explosion is
    /// drawn at the size the telegraph promised rather than at a size picked here.
    telegraphs: Vec<(u32, VfxHandle, TelegraphShape)>,
    /// The gathering effect on each enemy that is casting, by entity slot.
    enemy_casts: Vec<(u32, VfxHandle)>,
    /// The mage's defensive shell, while Flame Shield is up.
    shield: Option<VfxHandle>,
    /// The wall across the boss's doorway, while the dungeon keeps it shut.
    gate: Option<VfxHandle>,
    /// The Fire Mastery aura, while it is up.
    mastery: Option<VfxHandle>,
    /// The gathering effect, while a cast is running.
    cast: Option<VfxHandle>,
    enabled: bool,
    refused: u64,
}

impl VfxDirector {
    /// A director drawing with `visuals`.
    #[must_use]
    pub fn new(visuals: VfxVisuals) -> Self {
        Self {
            system: VfxSystem::default(),
            visuals,
            pools: [const { Vec::new() }; VfxShape::COUNT],
            band_scratch: DecalScratch::new(),
            band_cache: Vec::new(),
            particles: ParticleDrawList::default(),
            particle_scratch: Vec::new(),
            bolts: Vec::with_capacity(32),
            ignited: Vec::with_capacity(64),
            areas: Vec::with_capacity(16),
            telegraphs: Vec::with_capacity(16),
            enemy_casts: Vec::with_capacity(16),
            shield: None,
            gate: None,
            mastery: None,
            cast: None,
            enabled: true,
            refused: 0,
        }
    }

    /// Turns every effect on or off.
    ///
    /// Off stops new effects and ends the ones playing, so "with VFX" and "without VFX" are
    /// two measurable states of the same build rather than two builds.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.system.clear();
            self.bolts.clear();
            self.areas.clear();
            self.telegraphs.clear();
            self.enemy_casts.clear();
            self.ignited.clear();
            self.shield = None;
            self.mastery = None;
            self.cast = None;
            self.particles.clear();
        }
    }

    /// Whether effects play.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// This frame's particles.
    #[must_use]
    pub fn particles(&self) -> &ParticleDrawList {
        &self.particles
    }

    /// What the director is holding.
    #[must_use]
    pub fn stats(&self) -> VfxDirectorStats {
        VfxDirectorStats {
            system: self.system.stats(),
            entities: self.pools.iter().map(Vec::len).sum(),
            bolts: self.bolts.len(),
            refused: self.refused,
        }
    }

    /// The runtime, for a demo scene that spawns effects directly.
    pub fn system_mut(&mut self) -> &mut VfxSystem {
        &mut self.system
    }

    /// Ends everything and hides its geometry. For a scene change or a combat reset.
    pub fn clear(&mut self, world: &mut RenderWorld) {
        self.system.clear();
        self.bolts.clear();
        self.areas.clear();
        self.telegraphs.clear();
        self.enemy_casts.clear();
        self.ignited.clear();
        self.shield = None;
        self.mastery = None;
        self.cast = None;
        self.particles.clear();
        for pool in &mut self.pools {
            for entity in pool.iter() {
                world.set_visible(*entity, false);
            }
        }
    }

    /// Reads a frame's combat events and starts what they call for.
    ///
    /// Takes the simulation because an effect needs to know how tall a target is and where
    /// its chest is — questions about the *world*, not about the event. Nothing here writes
    /// to the simulation, and it is borrowed immutably to say so.
    pub fn observe(&mut self, simulation: &Simulation) {
        if !self.enabled {
            return;
        }
        for index in 0..simulation.events().len() {
            let event = simulation.events()[index];
            self.observe_one(&event, simulation);
        }
    }

    /// Starts whatever one event calls for.
    fn observe_one(&mut self, event: &CombatEvent, simulation: &Simulation) {
        match *event {
            // A bolt: the line it flew, from the event, with the impact played on arrival.
            CombatEvent::ProjectileLaunched {
                target,
                from,
                to,
                ability,
                ..
            } => {
                let fire = ability.fire();
                let height = simulation.world().height(target);
                let origin = from + Vec3::new(0.0, CHEST_FRACTION * 1.7, 0.0);
                let destination = to + Vec3::new(0.0, height * CHEST_FRACTION, 0.0);
                self.launch_bolt(origin, destination, fire);
            }
            // The rock is committed to. The ring closes over exactly the delay gameplay is
            // counting down, so the anticipation is the timer.
            CombatEvent::ImpactScheduled {
                ability,
                position,
                radius,
                delay,
                ..
            } => {
                let effect = if ability.fire() == Some(FireAbility::Cataclysm) {
                    VfxId::Cataclysm
                } else {
                    VfxId::Telegraph
                };
                self.spawn(
                    VfxSpawn::at(effect, position)
                        .with_radius(radius.max(1.0))
                        .lasting(delay.max(0.05)),
                );
            }
            // It landed. An explosion sized by the radius that was actually hit.
            CombatEvent::ImpactLanded {
                position, radius, ..
            } => {
                self.spawn(VfxSpawn::at(VfxId::Explosion, position).with_radius(radius.max(1.0)));
            }
            // Fire on the ground, for as long as the zone burns and in the shape it burns.
            CombatEvent::AreaEffectStarted {
                id,
                position,
                shape,
                duration,
                ..
            } => {
                let spawn = match shape {
                    ZoneShape::Circle { radius } => {
                        VfxSpawn::at(VfxId::GroundArea, position).with_radius(radius)
                    }
                    ZoneShape::Line { yaw, length, width } => {
                        VfxSpawn::at(VfxId::LineFire, position)
                            .facing(yaw)
                            .with_radius(width)
                            .with_length(length)
                    }
                }
                .lasting(duration);
                if let Some(handle) = self.spawn(spawn) {
                    self.areas.push((id, handle));
                }
            }
            CombatEvent::AreaEffectEnded { id, .. } => {
                if let Some(index) = self.areas.iter().position(|(area, _)| *area == id) {
                    let (_, handle) = self.areas.swap_remove(index);
                    // Stopped rather than cleared: the fire stops being fed, and the embers
                    // already in the air burn out on their own.
                    self.system.stop(handle);
                }
            }
            // An instant that hits an area rather than a target: the cone, and the burn's
            // explosion. Both are announced as `AbilityUsed`, which is the only event that
            // knows a cone was cast at all.
            CombatEvent::AbilityUsed {
                source,
                ability,
                facing,
                ..
            } => {
                self.observe_ability(source, ability, facing, simulation);
            }
            // A teleport is two effects, because it is two places.
            CombatEvent::Teleported { from, to, .. } => {
                self.spawn(VfxSpawn::at(VfxId::BlinkDeparture, from).with_radius(0.8));
                self.spawn(VfxSpawn::at(VfxId::BlinkArrival, to).with_radius(1.1));
            }
            // Burning: one aura per target, refreshed rather than stacked.
            CombatEvent::StatusApplied {
                target, duration, ..
            } => self.ignite(target, duration, simulation),
            CombatEvent::StatusRemoved { target, .. } => self.extinguish(target),
            CombatEvent::Death { entity, .. } => self.extinguish(entity),
            // A hard landing kicks up dust. A step down does not.
            CombatEvent::Landed {
                position, speed, ..
            } => {
                if speed >= LANDING_SPEED {
                    let force = (speed / LANDING_SPEED).min(2.0);
                    self.spawn(
                        VfxSpawn::at(VfxId::Impact, position)
                            .with_radius(LANDING_RADIUS * force)
                            .tinted(DUST_TINT),
                    );
                }
            }
            // An enemy attack that is coming: the floor says where and the clock says when.
            // Its lifetime is the cast's own, so the shape closes exactly as the attack
            // lands - the timing is gameplay's, not a guess made here.
            CombatEvent::TelegraphStarted {
                anchor,
                shape,
                delay,
                id,
                follow,
                ..
            } => {
                let spawn = match shape {
                    // A mark rides its victim: stepping out of it is not the answer, taking it
                    // away from everybody else is — so it is drawn where they are.
                    TelegraphShape::Circle { radius } if follow.is_some() => follow.map(|victim| {
                        VfxSpawn::at(VfxId::Telegraph, Vec3::ZERO)
                            .with_radius(radius.max(0.5))
                            .lasting(delay.max(0.05))
                            .tinted(MARK_TINT)
                            .attached_to(victim.index() as u32)
                    }),
                    TelegraphShape::Circle { radius } => Some(
                        VfxSpawn::at(VfxId::Telegraph, anchor)
                            .with_radius(radius.max(0.5))
                            .lasting(delay.max(0.05)),
                    ),
                    // A spear, a volley or a charge's path: the band along the ground the
                    // player's own line effect is drawn with, in bone rather than fire.
                    TelegraphShape::Line { yaw, length, width } => Some(
                        VfxSpawn::at(VfxId::LineFire, anchor)
                            .facing(yaw)
                            .with_radius(width.max(0.5))
                            .with_length(length.max(0.5))
                            .lasting(delay.max(0.05))
                            .tinted(LINE_TELEGRAPH_TINT),
                    ),
                    TelegraphShape::Cone {
                        yaw,
                        range,
                        half_angle: _,
                    } => Some(
                        VfxSpawn::at(VfxId::ConeWave, anchor)
                            .facing(yaw)
                            .with_radius(range.max(0.5))
                            .lasting(delay.max(0.05))
                            .raised(0.35),
                    ),
                    // A single-target attack covers no ground; there is nothing to draw and
                    // the cast bar is what says it is coming.
                    TelegraphShape::Single => None,
                };
                if let Some(handle) = spawn.and_then(|spawn| self.spawn(spawn)) {
                    self.telegraphs.push((id, handle, shape));
                }
            }
            // It landed, or it did not. Either way the shape comes off the floor; only a
            // landing gets an explosion, and it is drawn at the size that was promised.
            CombatEvent::TelegraphEnded {
                id,
                anchor,
                resolved,
                ..
            } => {
                let Some(index) = self
                    .telegraphs
                    .iter()
                    .position(|(telegraph, _, _)| *telegraph == id)
                else {
                    return;
                };
                let (_, handle, shape) = self.telegraphs.swap_remove(index);
                self.system.stop_and_clear(handle);
                if resolved {
                    let radius = match shape {
                        TelegraphShape::Circle { radius } => radius,
                        TelegraphShape::Cone { range, .. } => range * 0.5,
                        TelegraphShape::Line { width, .. } => width,
                        TelegraphShape::Single => 1.0,
                    };
                    self.spawn(VfxSpawn::at(VfxId::Explosion, anchor).with_radius(radius.max(1.0)));
                }
            }
            // An enemy gathering. The player's own cast tell is synchronised from state
            // instead, because the player's cast is a thing this director can ask about.
            CombatEvent::CastStarted {
                source,
                ability,
                duration,
                ..
            } => {
                if ability.is_enemy() {
                    self.begin_enemy_cast(source, duration, simulation);
                }
            }
            CombatEvent::CastEnded {
                source, ability, ..
            } => {
                if ability.is_enemy() {
                    self.end_enemy_cast(source);
                }
            }
            // Everything else is either a state the frame synchronises, or moves no picture.
            // A phase change: one burst at the enemy that changed, so "it became something
            // else" is visible from across the room without reading a frame.
            CombatEvent::PhaseChanged { entity, .. } => {
                let Some(position) = simulation.world().position(entity) else {
                    return;
                };
                let height = simulation.world().height(entity).max(1.0);
                self.spawn(
                    VfxSpawn::at(
                        VfxId::Explosion,
                        position + Vec3::new(0.0, height * 0.5, 0.0),
                    )
                    .with_radius(height.mul_add(0.5, 2.0)),
                );
            }
            // The Warrior's defensive stream, drawn out of the **existing** catalogue: a
            // block is an impact, a shield is the shell Flame Shield already uses, and a
            // reflected spell is an explosion where it turned round. No new effects are
            // authored for this class.
            CombatEvent::AttackDefended {
                defender,
                outcome,
                position,
                ..
            } => {
                let height = simulation.world().height(defender).max(1.0);
                let tint = if outcome.is_avoided() {
                    AVOIDED_TINT
                } else {
                    BLOCKED_TINT
                };
                self.spawn(
                    VfxSpawn::at(
                        VfxId::Impact,
                        position + Vec3::new(0.0, height * CHEST_FRACTION, 0.0),
                    )
                    .with_radius(0.7)
                    .tinted(tint),
                );
            }
            // Spending is the one moment the resource is worth showing: the passive bonuses
            // have just come back, and the player pressed something to make it happen.
            CombatEvent::DeflectionSpent { entity, .. } => {
                let Some(position) = simulation.world().position(entity) else {
                    return;
                };
                self.spawn(
                    VfxSpawn::at(VfxId::BlinkArrival, position)
                        .with_radius(1.2)
                        .tinted(DEFLECTION_TINT),
                );
            }
            CombatEvent::AbsorbApplied {
                target, duration, ..
            } => {
                let Some(position) = simulation.world().position(target) else {
                    return;
                };
                let height = simulation.world().height(target).max(1.0);
                self.spawn(
                    VfxSpawn::at(VfxId::Shield, position + Vec3::new(0.0, height * 0.5, 0.0))
                        .with_radius(1.1)
                        .lasting(duration)
                        .tinted(ABSORB_TINT),
                );
            }
            CombatEvent::SpellReflected { source, .. } => {
                let Some(position) = simulation.world().position(source) else {
                    return;
                };
                let height = simulation.world().height(source).max(1.0);
                self.spawn(
                    VfxSpawn::at(
                        VfxId::Explosion,
                        position + Vec3::new(0.0, height * 0.5, 0.0),
                    )
                    .with_radius(1.4)
                    .tinted(REFLECT_TINT),
                );
            }
            CombatEvent::ControlApplied { target, .. } => {
                let Some(position) = simulation.world().position(target) else {
                    return;
                };
                let height = simulation.world().height(target).max(1.0);
                self.spawn(
                    VfxSpawn::at(VfxId::Impact, position + Vec3::new(0.0, height, 0.0))
                        .with_radius(0.9)
                        .tinted(CONTROL_TINT),
                );
            }
            // The gate breaking: a burst where it stood, the size of the doorway.
            CombatEvent::BossUnlocked { position, .. } => {
                self.spawn(
                    VfxSpawn::at(VfxId::Explosion, position + Vec3::new(0.0, 1.5, 0.0))
                        .with_radius(6.0)
                        .tinted(CRYPT_TINT),
                );
            }
            // An add stepping out of nowhere: the arrival Blink already draws, in the crypt's
            // colour, so "that one was called" reads from across the hall.
            CombatEvent::AddSummoned { position, .. } => {
                self.spawn(
                    VfxSpawn::at(VfxId::BlinkArrival, position)
                        .with_radius(2.2)
                        .tinted(CRYPT_TINT),
                );
            }
            CombatEvent::DungeonCompleted { .. } => {
                let player = simulation.player().entity;
                if let Some(position) = simulation.world().position(player) {
                    self.spawn(
                        VfxSpawn::at(VfxId::BlinkArrival, position)
                            .with_radius(3.0)
                            .tinted(MASTERY_TINT),
                    );
                }
            }
            // A point of Deflection and a shield taking a hit move no picture of their own:
            // both happen several times a second in a pack, and the block that caused them
            // has already been drawn.
            CombatEvent::Damage(_)
            | CombatEvent::Healing { .. }
            | CombatEvent::Jumped { .. }
            | CombatEvent::DeflectionGained { .. }
            | CombatEvent::AstralPowerGained { .. }
            | CombatEvent::AstralPowerArmed { .. }
            | CombatEvent::AstralPowerSpent { .. }
            | CombatEvent::ManaSpent { .. }
            | CombatEvent::ManaRestored { .. }
            | CombatEvent::Dispelled { .. }
            | CombatEvent::DungeonProgress { .. }
            | CombatEvent::Absorbed { .. }
            // Aggro is floating text, drawn by the damage numbers.
            | CombatEvent::Aggro { .. } => {}
        }
    }

    /// The effects an ability produces at the moment it is used.
    fn observe_ability(
        &mut self,
        source: EntityId,
        ability: engine_game::AbilityId,
        facing: f32,
        simulation: &Simulation,
    ) {
        let Some(fire) = ability.fire() else {
            return;
        };
        let definition = fire.definition();
        let origin = simulation.world().position(source).unwrap_or(Vec3::ZERO);
        // The yaw the cast was aimed with, off the event - not the caster's facing now.
        // A tick that turns the character after the cone has already picked its victims
        // would otherwise paint the wedge somewhere nothing was hit.
        let yaw = facing;

        match fire {
            // A wash of fire filling exactly the wedge the executor collects from.
            FireAbility::FlameCone => {
                self.spawn(
                    VfxSpawn::at(VfxId::ConeWave, origin)
                        .facing(yaw)
                        .with_radius(definition.range)
                        .raised(0.4),
                );
            }
            // The burn going off. Centred on the target, because that is the centre the
            // executor collected its hit list around.
            FireAbility::Detonate => {
                let centre = simulation
                    .player()
                    .target
                    .and_then(|target| simulation.world().position(target))
                    .unwrap_or(origin);
                self.spawn(
                    VfxSpawn::at(VfxId::Explosion, centre).with_radius(definition.radius.max(1.0)),
                );
            }
            _ => {}
        }
    }

    /// Puts a gathering effect on an enemy that has started casting.
    fn begin_enemy_cast(&mut self, source: EntityId, duration: f32, simulation: &Simulation) {
        self.end_enemy_cast(source);
        let slot = source.index() as u32;
        let height = simulation.world().height(source).max(1.0);
        if let Some(handle) = self.spawn(
            VfxSpawn::at(VfxId::CastCharge, Vec3::ZERO)
                .with_radius(height * 0.4)
                .raised(height * CHEST_FRACTION)
                .lasting(duration.max(0.05))
                .attached_to(slot),
        ) {
            self.enemy_casts.push((slot, handle));
        }
    }

    /// Takes it off again, however the cast ended.
    fn end_enemy_cast(&mut self, source: EntityId) {
        let slot = source.index() as u32;
        if let Some(index) = self
            .enemy_casts
            .iter()
            .position(|(entity, _)| *entity == slot)
        {
            let (_, handle) = self.enemy_casts.swap_remove(index);
            self.system.stop_and_clear(handle);
        }
    }

    /// Starts a bolt flying from `from` to `to`.
    fn launch_bolt(&mut self, from: Vec3, to: Vec3, ability: Option<FireAbility>) {
        let distance = (to - from).length();
        let duration = (distance / BOLT_SPEED).clamp(BOLT_MIN_FLIGHT, BOLT_MAX_FLIGHT);
        // A heavier spell throws a bigger bolt and a bigger impact. Read from the ability's
        // own coefficient, so a rebalance moves the picture with the damage.
        let weight = ability.map_or(1.0, |ability| {
            (ability.definition().coefficient / 2.0).clamp(0.5, 2.0)
        });
        let Some(handle) = self.spawn(
            VfxSpawn::at(VfxId::Projectile, from)
                .with_radius(weight)
                .lasting(duration),
        ) else {
            return;
        };
        self.bolts.push(Bolt {
            handle,
            from,
            to,
            elapsed: 0.0,
            duration,
            impact_radius: weight,
        });
    }

    /// Puts a burn on a target, or refreshes the one already there.
    fn ignite(&mut self, target: EntityId, duration: f32, simulation: &Simulation) {
        let slot = target.index();
        if self.ignited.len() <= slot {
            self.ignited.resize(slot + 1, None);
        }
        // Refresh rather than stack: a target at five stacks would otherwise be wearing five
        // auras, which is five times the particles for a fact the nameplate already states.
        if let Some((generation, handle)) = self.ignited[slot]
            && generation == target.generation()
            && self.system.contains(handle)
        {
            self.system.stop_and_clear(handle);
        }
        let height = simulation.world().height(target).max(1.0);
        let handle = self.spawn(
            VfxSpawn::at(VfxId::Ignited, Vec3::ZERO)
                .with_radius(height * 0.35)
                .raised(height * CHEST_FRACTION)
                .lasting(duration)
                .attached_to(slot as u32),
        );
        self.ignited[slot] = handle.map(|handle| (target.generation(), handle));
    }

    /// Takes the burn off a target, if it has one.
    fn extinguish(&mut self, target: EntityId) {
        let slot = target.index();
        let Some(Some((generation, handle))) = self.ignited.get(slot).copied() else {
            return;
        };
        if generation != target.generation() {
            return;
        }
        self.system.stop_and_clear(handle);
        self.ignited[slot] = None;
    }

    /// Advances every effect and rebuilds what the frame draws.
    ///
    /// `delta` is real time, not simulation time: an effect is presentation and is expected
    /// to move every frame, which is what stops a 60 Hz simulation from looking like 60 FPS.
    pub fn update(
        &mut self,
        simulation: &Simulation,
        delta: f32,
        world: &mut RenderWorld,
        ground: &GroundSurface<'_>,
    ) {
        self.update_with(simulation, delta, world, ground, None);
    }

    /// [`VfxDirector::update`] with the per-particle passes optionally spread across `jobs`.
    ///
    /// Both of them stay on the calling thread below the pool's own size threshold, which
    /// every scene the game has today is under.
    pub fn update_with(
        &mut self,
        simulation: &Simulation,
        delta: f32,
        world: &mut RenderWorld,
        ground: &GroundSurface<'_>,
        jobs: Option<&engine_core::jobs::JobSystem>,
    ) {
        if !self.enabled {
            self.particles.clear();
            return;
        }
        self.advance_bolts(delta);
        self.prune();
        self.sync_states(simulation);

        // Where an attached effect's owner is now. An entity that has gone answers `None`,
        // and the effect riding it ends rather than hanging over an empty patch of floor.
        let game = simulation.world();
        self.system.update_with(
            delta,
            |key| {
                let id = game.id_at(key as usize)?;
                if !game.is_alive(id) {
                    return None;
                }
                game.position(id)
            },
            jobs,
        );

        self.particles.clear();
        self.system
            .write_particles_with(&mut self.particles, jobs, &mut self.particle_scratch);
        self.sync_entities(world, ground);
    }

    /// Forgets every laid band, so the next frame rebuilds them against the ground as it
    /// now is.
    ///
    /// The band cache keys on the footprint, not on the terrain: a height field has no
    /// version to compare and hashing one per frame would cost more than the rebuild it
    /// saves. So the level tells the director when it has moved the ground — which happens
    /// when a level loads and when an author edits the terrain, and never during play.
    pub fn invalidate_ground(&mut self) {
        for band in &mut self.band_cache {
            band.key = None;
        }
    }

    /// Forgets the effects that ended without the director being told.
    ///
    /// An attached effect ends by itself when the thing it rides dies — which is the right
    /// behaviour and leaves this bookkeeping holding a handle to a slot that has moved on.
    /// Pruning here rather than at every read means "is this target burning" is one lookup
    /// rather than a lookup and a liveness check.
    fn prune(&mut self) {
        for entry in &mut self.ignited {
            if let Some((_, handle)) = *entry
                && !self.system.contains(handle)
            {
                *entry = None;
            }
        }
        self.areas
            .retain(|(_, handle)| self.system.contains(*handle));
        self.telegraphs
            .retain(|(_, handle, _)| self.system.contains(*handle));
        self.enemy_casts
            .retain(|(_, handle)| self.system.contains(*handle));
    }

    /// Moves every bolt along its line, playing its impact when it arrives.
    fn advance_bolts(&mut self, delta: f32) {
        let mut index = 0;
        while index < self.bolts.len() {
            let bolt = &mut self.bolts[index];
            bolt.elapsed += delta;
            let fraction = (bolt.elapsed / bolt.duration).clamp(0.0, 1.0);
            // A shallow arc, highest in the middle: a dead-straight line between two chests
            // reads as a laser rather than as something thrown.
            let arc =
                (fraction * std::f32::consts::PI).sin() * (bolt.to - bolt.from).length() * 0.06;
            let position = bolt.from + (bolt.to - bolt.from) * fraction + Vec3::new(0.0, arc, 0.0);
            let bolt = *bolt;
            self.system.set_position(bolt.handle, position);

            if fraction >= 1.0 {
                self.system.stop(bolt.handle);
                self.spawn(VfxSpawn::at(VfxId::Impact, bolt.to).with_radius(bolt.impact_radius));
                self.bolts.swap_remove(index);
                continue;
            }
            index += 1;
        }
    }

    /// Starts and stops the effects that follow a *state* rather than a moment.
    fn sync_states(&mut self, simulation: &Simulation) {
        self.sync_gate(simulation);
        let player = simulation.player().entity;
        let slot = player.index() as u32;
        let height = simulation.world().height(player).max(1.0);

        // Flame Shield: a shell around the mage for exactly as long as the timer runs.
        let shielded = simulation.mage().flame_shield.is_active();
        self.sync_aura(
            shielded,
            SyncedAura::Shield,
            VfxSpawn::at(VfxId::Shield, Vec3::ZERO)
                .with_radius(height * 0.55)
                .raised(height * CHEST_FRACTION)
                .lasting(f32::MAX)
                .tinted(SHIELD_TINT)
                .attached_to(slot),
        );

        // Fire Mastery: the same shell, smaller and gold. Reused deliberately — it is the
        // same statement ("a state is running on the caster") and a second shape would cost
        // a mesh, a binding and a reason.
        let mastered = simulation.mage().mastery_buff.is_active();
        self.sync_aura(
            mastered,
            SyncedAura::Mastery,
            VfxSpawn::at(VfxId::Shield, Vec3::ZERO)
                .with_radius(height * 0.32)
                .raised(height * 1.05)
                .lasting(f32::MAX)
                .tinted(MASTERY_TINT)
                .attached_to(slot),
        );

        // The cast tell. Its lifetime is the cast's own remaining time, so the gather
        // finishes exactly when the spell lands — the timing is gameplay's, not an
        // animation's guess at it.
        let casting = simulation.casting().is_some();
        self.sync_aura(
            casting,
            SyncedAura::Cast,
            VfxSpawn::at(VfxId::CastCharge, Vec3::ZERO)
                .with_radius(height * 0.45)
                .raised(height * CHEST_FRACTION)
                .lasting(f32::MAX)
                .attached_to(slot),
        );
    }

    /// Keeps the wall across the boss's doorway up for exactly as long as the gate is shut.
    ///
    /// State rather than an event: the gate is shut from the moment the level is built, and
    /// a director created after that — or cleared by a reset — still has to draw it.
    fn sync_gate(&mut self, simulation: &Simulation) {
        let gate = simulation
            .dungeon_run()
            .and_then(|run| run.rules().gate)
            .filter(|_| simulation.gate_is_closed());
        let live = self.gate.is_some_and(|handle| self.system.contains(handle));
        match (gate, live) {
            (Some(gate), false) => {
                let (from, to) = gate.ends();
                let yaw = engine_game::enemy::brain::yaw_towards(from, to);
                self.gate = self.spawn(
                    VfxSpawn::at(VfxId::LineFire, from)
                        .facing(yaw)
                        .with_radius(1.4)
                        .with_length(gate.half_extents.x * 2.0)
                        .lasting(f32::MAX)
                        .tinted(GATE_TINT),
                );
            }
            (None, true) => {
                if let Some(handle) = self.gate.take() {
                    self.system.stop(handle);
                }
            }
            _ => {}
        }
    }

    /// Keeps one state-driven effect in step with the state that owns it.
    fn sync_aura(&mut self, active: bool, which: SyncedAura, spawn: VfxSpawn) {
        let current = match which {
            SyncedAura::Shield => self.shield,
            SyncedAura::Mastery => self.mastery,
            SyncedAura::Cast => self.cast,
        };
        let live = current.is_some_and(|handle| self.system.contains(handle));

        let next = match (active, live, current) {
            // The state started, or the effect was dropped and the state is still running.
            (true, false, _) => self.spawn(spawn),
            // The state ended. Cleared rather than stopped: a shield's sparks should not
            // outlive the shield by a second.
            (false, true, Some(handle)) => {
                self.system.stop_and_clear(handle);
                None
            }
            (false, _, _) => None,
            (true, true, handle) => handle,
        };

        match which {
            SyncedAura::Shield => self.shield = next,
            SyncedAura::Mastery => self.mastery = next,
            SyncedAura::Cast => self.cast = next,
        }
    }

    /// Starts an effect, counting the refusal if the system is full.
    fn spawn(&mut self, spawn: VfxSpawn) -> Option<VfxHandle> {
        let handle = self.system.spawn(spawn);
        if handle.is_none() {
            self.refused += 1;
        }
        handle
    }

    /// Rebuilds the render entities this frame's geometry needs.
    ///
    /// Entities are pooled per shape and *reused*: a frame with three discs and last frame's
    /// five leaves two hidden rather than despawning them, so a busy fight stops touching
    /// the `RenderWorld`'s slot allocator entirely.
    fn sync_entities(&mut self, world: &mut RenderWorld, ground: &GroundSurface<'_>) {
        let mut used = [0_usize; VfxShape::COUNT];
        // How many ground bands this frame has laid, which is the cache slot each one uses.
        // Position in the frame's draw order rather than an effect id, because a mesh draw
        // does not carry one. An effect retiring shifts the ones behind it by a slot and
        // they rebuild once; the key is what makes that correct rather than merely likely.
        let mut bands = 0_usize;
        let quad = self.visuals.get(VfxShape::Quad);
        for index in 0..self.system.mesh_draws().len() {
            let draw = self.system.mesh_draws()[index];
            let style = style_of(&draw);

            // A flat footprint is laid onto the ground as a band of pieces. Everything that
            // stands up — a shell, a pillar — is one entity where it always was.
            if let Some(quad) = quad
                && draw.ground
                && used[VfxShape::Quad.index()] < MAX_BAND_PIECES
                && let Some(shape) = footprint_of(&draw)
            {
                let anchor = Vec3::new(draw.position.x, draw.position.y, draw.position.z);
                let band = band_of(&draw, shape);
                let key = BandKey {
                    shape,
                    anchor,
                    yaw: draw.yaw,
                    band,
                };
                // A footprint that has not moved lays the same pieces on the same ground, so
                // it is built once and read from here on. Burning zones and telegraphs stand
                // still for their whole life, and rebuilding a hundred pieces of trigonometry
                // per zone per frame to arrive at last frame's answer was the largest single
                // cost in this director.
                let slot_index = bands;
                bands += 1;
                if self.band_cache.len() <= slot_index {
                    // Reserved to the frame's whole piece budget, so a slot reused by a
                    // bigger footprint next frame does not grow its buffer in the frame path.
                    // `build_band` cuts to `MAX_SEGMENTS` pieces but each edge rounds up
                    // independently, so the count is near it rather than bounded by it. The
                    // whole point of the cache is to stop doing work once a frame;
                    // allocating once a frame instead would be a poor trade.
                    self.band_cache.push(CachedBand {
                        key: None,
                        pieces: Vec::with_capacity(MAX_BAND_PIECES),
                    });
                }
                let cached = &mut self.band_cache[slot_index];
                if cached.key != Some(key) {
                    ground_decal::build_band(
                        shape,
                        anchor,
                        draw.yaw,
                        band,
                        ground,
                        &mut self.band_scratch,
                        &mut cached.pieces,
                    );
                    cached.key = Some(key);
                }
                let pieces = &self.band_cache[slot_index].pieces;
                // Either the whole footprint is laid or none of it is. A band cut off halfway
                // through the frame's budget would be an outline with a hole in it, which
                // reads as "the fire stops here" — the one thing a footprint may not say.
                let slot = &mut used[VfxShape::Quad.index()];
                if *slot + pieces.len() <= MAX_BAND_PIECES {
                    for piece in 0..pieces.len() {
                        let transform = self.band_cache[slot_index].pieces[piece];
                        place_pooled(
                            world,
                            &mut self.pools[VfxShape::Quad.index()],
                            *slot + piece,
                            transform,
                            quad,
                            self.visuals.material(),
                            style,
                        );
                    }
                    *slot += self.band_cache[slot_index].pieces.len();
                    continue;
                }
            }

            let Some(binding) = self.visuals.get(draw.shape) else {
                continue;
            };
            let shape = draw.shape.index();
            place_pooled(
                world,
                &mut self.pools[shape],
                used[shape],
                transform_of(&draw),
                binding,
                self.visuals.material(),
                style,
            );
            used[shape] += 1;
        }

        for (shape, pool) in self.pools.iter().enumerate() {
            for entity in pool.iter().skip(used[shape]) {
                world.set_visible(*entity, false);
            }
        }
    }
}

/// What a laid band was built from.
///
/// Every input [`ground_decal::build_band`] reads except the ground itself, which changes
/// only when the level does — see [`VfxDirector::invalidate_ground`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct BandKey {
    shape: TargetShape,
    anchor: Vec3,
    yaw: f32,
    band: DecalBand,
}

/// One footprint's pieces and the inputs they were built from.
#[derive(Debug, Default)]
struct CachedBand {
    /// `None` until the slot has been built once.
    key: Option<BandKey>,
    pieces: Vec<Transform>,
}

/// Which state-driven effect is being synchronised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncedAura {
    Shield,
    Mastery,
    Cast,
}

/// Puts one entity at `slot` of `pool`, reusing what is there or spawning if it is short.
///
/// Free rather than a method so the caller can hold `&mut` on one pool and read the visuals
/// at the same time; the borrow checker sees two fields rather than one `self`.
fn place_pooled(
    world: &mut RenderWorld,
    pool: &mut Vec<RenderEntity>,
    slot: usize,
    transform: Transform,
    binding: ShapeBinding,
    material: MaterialHandle,
    style: InstanceStyle,
) {
    if slot < pool.len() {
        let entity = pool[slot];
        world.set_transform(entity, transform, binding.bounds);
        world.set_style(entity, style);
        world.set_visible(entity, true);
        return;
    }
    let entity = world.spawn(RenderEntityDesc {
        transform,
        mesh: binding.mesh,
        material,
        bounds: binding.bounds,
        visible: true,
        style,
        // An effect: a wedge of fire on the floor, a ring, a telegraph. It is additive light
        // rather than matter, so it casts nothing — a fireball that cast a shadow would
        // darken the ground it is supposed to be lighting — and it is not shadowed either.
        shadow: engine_render_data::ShadowCasting::NONE,
    });
    pool.push(entity);
}

/// The boundary a flat effect shape covers, in the vocabulary the band builder speaks.
///
/// The *same* vocabulary the targeting indicator uses, which is the point: a burning zone and
/// the circle that placed it are the same footprint laid the same way, so they cannot disagree
/// about where the ground is.
#[must_use]
fn footprint_of(draw: &MeshDraw) -> Option<TargetShape> {
    match draw.shape {
        VfxShape::Disc | VfxShape::Ring => Some(TargetShape::Circle {
            radius: draw.radius.max(0.05),
        }),
        // The executor's own spread, so a wedge of fire covers the wedge that burned.
        VfxShape::Cone => Some(TargetShape::Cone {
            range: draw.radius.max(0.05),
            half_angle: engine_game::sim::CONE_HALF_ANGLE,
        }),
        VfxShape::Strip => Some(TargetShape::Line {
            width: draw.radius.max(0.05),
            length: draw.length.max(0.05),
        }),
        VfxShape::Shell | VfxShape::Column | VfxShape::Quad => None,
    }
}

/// How thick a footprint's band is and how far inside its boundary it sits.
///
/// An edge keeps the thin annulus the ring mesh always was. An interior gets a wide band, so
/// the area reads as marked without the middle of it being covered. Both put their *outer*
/// rim on the boundary, which is what makes the fire's edge the edge that burned.
#[must_use]
fn band_of(draw: &MeshDraw, shape: TargetShape) -> DecalBand {
    // What the band has to fit inside. A strip is as narrow as it is wide, whatever its
    // length, so its own width is the limit — not the twenty metres it runs.
    let extent = match shape {
        TargetShape::Line { width, .. } => width * 0.5,
        other => other.extent(),
    };
    let fraction = if draw.shape.is_edge() {
        EDGE_BAND_FRACTION
    } else {
        FILL_BAND_FRACTION
    };
    let width = (extent * fraction)
        .max(MIN_BAND_WIDTH)
        .min(extent * MAX_BAND_SHARE);
    DecalBand::edged(width.max(f32::MIN_POSITIVE), GROUND_LIFT)
}

/// Where and how big one piece of effect geometry is drawn.
///
/// The scale conventions are the other half of the contract [`VfxVisuals`] documents: every
/// mesh is authored at unit size, so this is the only place that turns metres into a scale.
#[must_use]
fn transform_of(draw: &MeshDraw) -> Transform {
    let rotation = engine_core::math::Quat::from_yaw_pitch_roll(draw.yaw, 0.0, 0.0);
    let (translation, scale) = match draw.shape {
        VfxShape::Disc | VfxShape::Ring | VfxShape::Cone => (
            Vec3::new(
                draw.position.x,
                draw.position.y + GROUND_LIFT,
                draw.position.z,
            ),
            Vec3::new(draw.radius, 1.0, draw.radius),
        ),
        VfxShape::Strip => (
            Vec3::new(
                draw.position.x,
                draw.position.y + GROUND_LIFT,
                draw.position.z,
            ),
            Vec3::new(draw.radius.max(0.1), 1.0, draw.length.max(0.1)),
        ),
        VfxShape::Shell => (
            draw.position,
            Vec3::new(draw.radius, draw.height.max(draw.radius), draw.radius),
        ),
        VfxShape::Column => (
            draw.position + Vec3::new(0.0, draw.height * 0.5, 0.0),
            Vec3::new(draw.radius * 2.0, draw.height.max(0.1), draw.radius * 2.0),
        ),
        // A piece of a laid-out footprint never reaches here: the band builder produces its
        // transform outright, and no effect in the library asks for the shape. Answering with
        // the draw as it stands is what a fallback should do - nothing surprising.
        VfxShape::Quad => (
            draw.position,
            Vec3::new(draw.radius.max(0.1), 1.0, draw.length.max(0.1)),
        ),
    };
    Transform {
        translation,
        rotation,
        scale,
    }
}

/// The per-entity visual state one piece of effect geometry is drawn with.
///
/// The layer's alpha becomes brightness, because the world pass is opaque: a fading ring
/// dims rather than turning translucent. Multiplying the tint by the alpha *and* scaling the
/// emissive is what makes the fade read on both a lit surface and an unlit one.
#[must_use]
fn style_of(draw: &MeshDraw) -> InstanceStyle {
    let alpha = draw.tint[3].clamp(0.0, 1.0);
    InstanceStyle {
        tint: [
            draw.tint[0] * alpha,
            draw.tint[1] * alpha,
            draw.tint[2] * alpha,
            1.0,
        ],
        emissive: draw.emissive * alpha,
        ..InstanceStyle::NEUTRAL
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::command::PlayerCommand;

    /// Bindings for every shape, with fabricated handles: this test suite never uploads
    /// anything, and the director only ever passes a handle through to the `RenderWorld`.
    fn visuals() -> VfxVisuals {
        let mut visuals = VfxVisuals::new(MaterialHandle::new(0, 1));
        for (index, shape) in VfxShape::ALL.into_iter().enumerate() {
            visuals.bind(
                shape,
                MeshHandle::new(index as u32, 1),
                Sphere::new(Vec3::ZERO, 1.0),
            );
        }
        visuals
    }

    /// The arena floor: a level with no height field.
    fn flat_ground() -> GroundSurface<'static> {
        GroundSurface::Plane(0.0)
    }

    /// A terrain that rises `slope` metres for every metre travelled along +X.
    fn ramp(slope: f32) -> engine_environment::terrain::Terrain {
        let mut terrain = engine_environment::terrain::Terrain::flat(129, 1.0, 0.0);
        for z in 0..terrain.resolution {
            for x in 0..terrain.resolution {
                let world_x = terrain.origin.x + x as f32 * terrain.cell_size;
                terrain.heights[z * terrain.resolution + x] = world_x * slope;
            }
        }
        terrain
    }

    fn arena(dummies: usize) -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(dummies));
        simulation.step_once();
        simulation.select_nearest_target();
        simulation
    }

    /// Runs the whole frame loop for `frames`, observing and updating exactly as the app does.
    fn run(
        director: &mut VfxDirector,
        simulation: &mut Simulation,
        world: &mut RenderWorld,
        frames: u32,
    ) {
        for _ in 0..frames {
            simulation.step_once();
            director.observe(simulation);
            director.update(simulation, 1.0 / 60.0, world, &flat_ground());
        }
    }

    #[test]
    fn a_cast_that_lands_produces_a_bolt_and_then_an_impact() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);

        simulation.push_command(PlayerCommand::UseAbility(0));
        // A one-and-a-half-second cast, then the flight. Stepped one frame at a time so the
        // moment the bolt exists can be seen rather than inferred.
        let mut seen_in_flight = false;
        for _ in 0..110 {
            simulation.step_once();
            director.observe(&simulation);
            director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());
            seen_in_flight |= director.stats().bolts > 0;
        }
        assert!(seen_in_flight, "no bolt left the caster");

        run(&mut director, &mut simulation, &mut world, 40);
        assert_eq!(director.stats().bolts, 0, "the bolt never arrived");
        assert!(
            director.stats().system.spawned >= 2,
            "the arrival played nothing"
        );
    }

    #[test]
    fn an_area_effect_lives_exactly_as_long_as_the_zone_does() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(4);

        let aim = simulation.player_position() + Vec3::new(0.0, 0.0, -6.0);
        simulation
            .cast_at(FireAbility::Firestorm, aim)
            .expect("a line is always castable");
        director.observe(&simulation);
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());
        assert_eq!(director.areas.len(), 1, "the fire was not drawn");

        // Past the zone's duration: the gameplay zone ends, the event says so, and the
        // effect goes with it.
        run(&mut director, &mut simulation, &mut world, 60 * 8);
        assert!(
            director.areas.is_empty(),
            "the fire outlived the zone that was burning"
        );
    }

    /// Every visible piece of a laid-out footprint, as a world transform.
    fn laid_pieces(director: &VfxDirector, world: &RenderWorld) -> Vec<Transform> {
        director.pools[VfxShape::Quad.index()]
            .iter()
            .filter_map(|entity| {
                let slot = entity.index();
                world.visible_flags()[slot].then(|| world.transforms()[slot])
            })
            .collect()
    }

    #[test]
    fn a_burning_zone_is_laid_on_the_terrain_rather_than_on_a_plane() {
        // The defect: a zone was one horizontal disc at the effect's own y, so on a slope
        // half of it was buried and the other half hung in the air. Every piece of it now
        // sits on the ground under that piece.
        let slope = ramp(0.5);
        let ground = GroundSurface::Terrain(&slope);
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);

        director
            .system_mut()
            .spawn(VfxSpawn::at(VfxId::GroundArea, Vec3::new(4.0, 0.0, -3.0)).with_radius(6.0));
        director.update(&simulation, 1.0 / 60.0, &mut world, &ground);

        let pieces = laid_pieces(&director, &world);
        assert!(pieces.len() > 40, "only {} pieces", pieces.len());

        let mut lowest = f32::MAX;
        let mut highest = f32::MIN;
        for piece in &pieces {
            let under = ground.height_at(piece.translation.x, piece.translation.z);
            let above = piece.translation.y - under;
            assert!(
                (above - GROUND_LIFT).abs() < 0.05,
                "a piece sat {above} m over the ground rather than {GROUND_LIFT}",
            );
            lowest = lowest.min(piece.translation.y);
            highest = highest.max(piece.translation.y);
        }
        // And it actually followed the slope: a flat disc would put every piece at one height.
        assert!(
            highest - lowest > 4.0,
            "the footprint spanned only {} m of height on a slope",
            highest - lowest,
        );
    }

    #[test]
    fn a_zone_marks_its_area_without_covering_the_ground_inside_it() {
        // The other half of the brief: the interior is the terrain. The disc layer is a wide
        // band just inside the boundary now, so the middle of a zone has nothing drawn over
        // it and the fire there is the particles, which blend.
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);
        let centre = Vec3::new(0.0, 0.0, 0.0);
        let radius = 6.0_f32;

        director
            .system_mut()
            .spawn(VfxSpawn::at(VfxId::GroundArea, centre).with_radius(radius));
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());

        let pieces = laid_pieces(&director, &world);
        assert!(!pieces.is_empty(), "the zone drew nothing");
        let nearest = pieces.iter().fold(f32::MAX, |nearest, piece| {
            let x = piece.translation.x - centre.x;
            let z = piece.translation.z - centre.z;
            nearest.min((x * x + z * z).sqrt())
        });
        assert!(
            nearest > radius * 0.5,
            "something was drawn {nearest} m from the centre of a {radius} m zone",
        );
        // Nothing is left of the old plate: no flat disc, wedge or strip is visible.
        for shape in [
            VfxShape::Disc,
            VfxShape::Ring,
            VfxShape::Cone,
            VfxShape::Strip,
        ] {
            let flat = director.pools[shape.index()]
                .iter()
                .filter(|entity| world.visible_flags()[entity.index()])
                .count();
            assert_eq!(flat, 0, "{shape:?} was still drawn as a flat plate");
        }
    }

    #[test]
    fn an_enemy_telegraph_and_a_line_of_fire_are_laid_the_same_way() {
        // One code path for every footprint: a circle under an incoming rock, a wedge, and a
        // strip of burning ground all become pieces on the terrain.
        let slope = ramp(0.35);
        let ground = GroundSurface::Terrain(&slope);
        for effect in [VfxId::Telegraph, VfxId::LineFire, VfxId::ConeWave] {
            let mut director = VfxDirector::new(visuals());
            let mut world = RenderWorld::new();
            let simulation = arena(1);
            director.system_mut().spawn(
                VfxSpawn::at(effect, Vec3::new(-2.0, 0.0, 1.0))
                    .with_radius(7.0)
                    .with_length(12.0),
            );
            director.update(&simulation, 1.0 / 60.0, &mut world, &ground);

            let pieces = laid_pieces(&director, &world);
            assert!(!pieces.is_empty(), "{effect:?} drew no footprint");
            for piece in &pieces {
                let under = ground.height_at(piece.translation.x, piece.translation.z);
                assert!(
                    (piece.translation.y - under - GROUND_LIFT).abs() < 0.05,
                    "{effect:?}: a piece left the ground at {:?}",
                    piece.translation,
                );
            }
        }
    }

    #[test]
    fn a_footprint_that_has_not_moved_is_laid_where_it_was() {
        // The cache. A zone standing still must produce the same pieces on frame two as on
        // frame one — the whole saving is only allowed if the answer does not change.
        let slope = ramp(0.35);
        let ground = GroundSurface::Terrain(&slope);
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);
        director
            .system_mut()
            .spawn(VfxSpawn::at(VfxId::GroundArea, Vec3::new(-2.0, 0.0, 1.0)).with_radius(7.0));

        director.update(&simulation, 1.0 / 60.0, &mut world, &ground);
        let first = laid_pieces(&director, &world);
        assert!(!first.is_empty(), "the fixture must lay a footprint");

        for _ in 0..4 {
            director.update(&simulation, 1.0 / 60.0, &mut world, &ground);
        }
        let later = laid_pieces(&director, &world);
        assert_eq!(later.len(), first.len(), "the band changed size on its own");
        for (index, (a, b)) in later.iter().zip(&first).enumerate() {
            assert_eq!(
                (a.translation, a.scale),
                (b.translation, b.scale),
                "piece {index} moved without the footprint moving"
            );
        }
    }

    #[test]
    fn moving_the_ground_relays_every_footprint() {
        // The one input the cache key cannot see. A terrain edit moves the surface under a
        // band that has not moved itself, and the level says so rather than the director
        // hashing a height field every frame.
        let flat = flat_ground();
        let slope = ramp(0.35);
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);
        director
            .system_mut()
            .spawn(VfxSpawn::at(VfxId::GroundArea, Vec3::new(-2.0, 0.0, 1.0)).with_radius(7.0));

        director.update(&simulation, 1.0 / 60.0, &mut world, &flat);
        let on_the_plane = laid_pieces(&director, &world);

        director.invalidate_ground();
        director.update(
            &simulation,
            1.0 / 60.0,
            &mut world,
            &GroundSurface::Terrain(&slope),
        );
        let on_the_slope = laid_pieces(&director, &world);

        assert_eq!(on_the_slope.len(), on_the_plane.len());
        assert!(
            on_the_slope
                .iter()
                .zip(&on_the_plane)
                .any(|(a, b)| a.translation.y != b.translation.y),
            "every piece stayed at the plane's height after the ground moved"
        );
        for piece in &on_the_slope {
            let under = slope.height_at(Vec3::new(piece.translation.x, 0.0, piece.translation.z));
            assert!(
                (piece.translation.y - under - GROUND_LIFT).abs() < 0.05,
                "a piece was left on the old ground at {:?}",
                piece.translation,
            );
        }
    }

    #[test]
    fn a_footprint_that_moved_is_relaid() {
        // The other half of the key: a band whose anchor changed must not be reused, or a
        // zone would stay drawn where it was cast rather than where it is.
        let ground = flat_ground();
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);
        let handle = director
            .system_mut()
            .spawn(VfxSpawn::at(VfxId::GroundArea, Vec3::new(0.0, 0.0, 0.0)).with_radius(7.0))
            .expect("the effect must spawn");

        director.update(&simulation, 1.0 / 60.0, &mut world, &ground);
        let before = laid_pieces(&director, &world);

        assert!(
            director
                .system_mut()
                .set_position(handle, Vec3::new(20.0, 0.0, 0.0))
        );
        director.update(&simulation, 1.0 / 60.0, &mut world, &ground);
        let after = laid_pieces(&director, &world);

        assert_eq!(after.len(), before.len());
        assert!(
            after
                .iter()
                .zip(&before)
                .all(|(a, b)| a.translation.x != b.translation.x),
            "the band stayed where the zone used to be"
        );
    }

    #[test]
    fn a_frame_full_of_zones_falls_back_to_the_flat_shape_rather_than_to_a_broken_outline() {
        // The budget. Past it the remaining footprints are the plates they used to be, which
        // fits the ground badly; cutting a band off partway would leave a hole in an outline
        // instead, and a hole reads as "the fire stops here".
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);

        for step in 0..40 {
            let angle = step as f32 * 0.7;
            director.system_mut().spawn(
                VfxSpawn::at(
                    VfxId::GroundArea,
                    Vec3::new(angle.cos() * 30.0, 0.0, angle.sin() * 30.0),
                )
                .with_radius(8.0),
            );
        }
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());

        let laid = laid_pieces(&director, &world).len();
        assert!(
            laid <= MAX_BAND_PIECES,
            "{laid} pieces is over the frame budget of {MAX_BAND_PIECES}",
        );
        assert!(laid > MAX_BAND_PIECES / 2, "the budget went mostly unspent");
        // The zones that missed out are still drawn, as their old flat selves.
        let flat: usize = [VfxShape::Disc, VfxShape::Ring]
            .into_iter()
            .map(|shape| {
                director.pools[shape.index()]
                    .iter()
                    .filter(|entity| world.visible_flags()[entity.index()])
                    .count()
            })
            .sum();
        assert!(flat > 0, "the zones past the budget vanished instead");
    }

    #[test]
    fn laying_a_footprint_out_stops_allocating_once_the_buffers_have_grown() {
        // The cost this has to not have: a zone is rebuilt every frame it burns, and a fight
        // full of burning ground must not allocate for it.
        let slope = ramp(0.4);
        let ground = GroundSurface::Terrain(&slope);
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let simulation = arena(1);

        director.system_mut().spawn(
            VfxSpawn::at(VfxId::GroundArea, Vec3::ZERO)
                .with_radius(7.0)
                .lasting(30.0),
        );
        for _ in 0..30 {
            director.update(&simulation, 1.0 / 60.0, &mut world, &ground);
        }
        let pieces: usize = director
            .band_cache
            .iter()
            .map(|band| band.pieces.capacity())
            .sum();
        let entities = director.pools[VfxShape::Quad.index()].len();
        for _ in 0..120 {
            director.update(&simulation, 1.0 / 60.0, &mut world, &ground);
        }
        assert_eq!(
            director
                .band_cache
                .iter()
                .map(|band| band.pieces.capacity())
                .sum::<usize>(),
            pieces,
            "the piece buffers grew after they had settled",
        );
        assert_eq!(
            director.pools[VfxShape::Quad.index()].len(),
            entities,
            "the entity pool churned instead of being reused",
        );
    }

    #[test]
    fn an_area_is_drawn_at_the_size_the_gameplay_burns() {
        // The accuracy requirement: the visual's footprint is the event's `ZoneShape`, which
        // is the value the containment test uses.
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(4);
        let definition = FireAbility::Firestorm.definition();

        let aim = simulation.player_position() + Vec3::new(0.0, 0.0, -6.0);
        simulation
            .cast_at(FireAbility::Firestorm, aim)
            .expect("castable");
        director.observe(&simulation);
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());

        let strip = director
            .system
            .mesh_draws()
            .iter()
            .find(|draw| draw.shape == VfxShape::Strip)
            .copied()
            .expect("the line of fire draws a strip");
        assert!((strip.length - definition.range).abs() < 1e-3, "{strip:?}");
        assert!((strip.radius - definition.width).abs() < 1e-3, "{strip:?}");
    }

    #[test]
    fn a_burning_target_wears_one_aura_however_many_stacks_it_has() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);

        for _ in 0..4 {
            simulation.push_command(PlayerCommand::UseAbility(0));
            run(&mut director, &mut simulation, &mut world, 70);
        }
        let burning = director
            .ignited
            .iter()
            .filter(|entry| entry.is_some())
            .count();
        assert_eq!(burning, 1, "one target, one aura");
    }

    #[test]
    fn a_dead_target_stops_burning() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);
        let target = simulation.player().target.expect("a dummy is selected");

        simulation.push_command(PlayerCommand::UseAbility(0));
        run(&mut director, &mut simulation, &mut world, 130);
        assert!(director.ignited.iter().any(Option::is_some));

        simulation.world_mut().damage(target, 1e9);
        run(&mut director, &mut simulation, &mut world, 4);
        assert!(
            director.ignited.iter().all(Option::is_none),
            "a corpse is still on fire"
        );
    }

    #[test]
    fn the_defensive_state_is_shown_for_exactly_as_long_as_it_runs() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);

        simulation
            .cast(FireAbility::FlameShield)
            .expect("self cast is always allowed");
        run(&mut director, &mut simulation, &mut world, 2);
        assert!(director.shield.is_some(), "the shield is not shown");

        let duration = FireAbility::FlameShield.definition().duration;
        run(
            &mut director,
            &mut simulation,
            &mut world,
            (duration * 60.0) as u32 + 10,
        );
        assert!(director.shield.is_none(), "the shield outlived the buff");
    }

    #[test]
    fn a_cast_shows_a_tell_that_ends_with_the_cast() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);

        simulation.push_command(PlayerCommand::UseAbility(0));
        run(&mut director, &mut simulation, &mut world, 10);
        assert!(director.cast.is_some(), "a cast with no tell");

        run(&mut director, &mut simulation, &mut world, 80);
        assert!(director.cast.is_none(), "the tell outlived the cast");
    }

    #[test]
    fn a_blink_marks_both_ends() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);

        let before = director.stats().system.spawned;
        simulation.cast(FireAbility::Blink).expect("instant");
        director.observe(&simulation);
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());
        assert!(
            director.stats().system.spawned >= before + 2,
            "a teleport showed fewer than two effects"
        );
    }

    #[test]
    fn geometry_reuses_its_render_entities_rather_than_churning_slots() {
        // The reason the pools exist: a fight that spawns and despawns entities every frame
        // makes the culler's ordering and the render world's free list churn for a picture
        // that is the same size as last frame's.
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(4);

        let aim = simulation.player_position() + Vec3::new(0.0, 0.0, -6.0);
        simulation
            .cast_at(FireAbility::Firestorm, aim)
            .expect("castable");
        run(&mut director, &mut simulation, &mut world, 30);
        let slots = world.slot_count();

        run(&mut director, &mut simulation, &mut world, 120);
        assert_eq!(
            world.slot_count(),
            slots,
            "the effect churned render world slots"
        );
    }

    #[test]
    fn turning_the_director_off_ends_everything_it_was_showing() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(4);

        simulation.push_command(PlayerCommand::UseAbility(0));
        run(&mut director, &mut simulation, &mut world, 70);
        assert!(director.stats().system.instances > 0);

        director.set_enabled(false);
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());
        assert_eq!(director.stats().system.instances, 0);
        assert!(director.particles().is_empty());

        // And nothing new starts while it is off.
        simulation.push_command(PlayerCommand::UseAbility(0));
        run(&mut director, &mut simulation, &mut world, 70);
        assert_eq!(director.stats().system.instances, 0);
    }

    #[test]
    fn an_unbound_shape_draws_nothing_and_breaks_nothing() {
        // What happens if an effect's geometry fails to load: the particles still play, the
        // frame still renders, and gameplay never notices.
        let mut director = VfxDirector::new(VfxVisuals::new(MaterialHandle::new(0, 1)));
        let mut world = RenderWorld::new();
        let mut simulation = arena(4);

        let aim = simulation.player_position() + Vec3::new(0.0, 0.0, -6.0);
        simulation
            .cast_at(FireAbility::Firestorm, aim)
            .expect("castable");
        // Observed before the next step, because a step starts a new event batch and the
        // cast's own events belong to the batch it is sitting in.
        director.observe(&simulation);
        director.update(&simulation, 1.0 / 60.0, &mut world, &flat_ground());
        run(&mut director, &mut simulation, &mut world, 30);

        assert_eq!(world.len(), 0, "geometry was drawn with no binding");
        assert!(
            !director.particles().is_empty(),
            "the fire lost its particles too"
        );
    }

    #[test]
    fn a_ground_effect_is_lifted_clear_of_the_floor() {
        // Coplanar with the floor is a depth tie, which flickers across the whole area.
        // This is the *fallback* geometry now - what a footprint is drawn as when a frame has
        // run out of piece budget - so the lift still has to be right.
        let draw = MeshDraw {
            shape: VfxShape::Disc,
            position: Vec3::ZERO,
            yaw: 0.0,
            radius: 4.0,
            length: 4.0,
            height: 0.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            emissive: 1.0,
            ground: true,
        };
        let transform = transform_of(&draw);
        assert!(transform.translation.y > 0.0);
        assert_eq!(transform.scale, Vec3::new(4.0, 1.0, 4.0));
    }

    #[test]
    fn a_fading_layer_dims_rather_than_turning_translucent() {
        // The world pass is opaque, so alpha has to become brightness or it means nothing.
        let mut draw = MeshDraw {
            shape: VfxShape::Ring,
            position: Vec3::ZERO,
            yaw: 0.0,
            radius: 1.0,
            length: 1.0,
            height: 0.0,
            tint: [1.0, 1.0, 1.0, 1.0],
            emissive: 2.0,
            ground: true,
        };
        let bright = style_of(&draw);
        draw.tint[3] = 0.25;
        let faded = style_of(&draw);

        assert!(faded.tint[0] < bright.tint[0]);
        assert!(faded.emissive < bright.emissive);
        assert_eq!(faded.tint[3], 1.0, "the opaque pass must be handed alpha 1");
    }

    #[test]
    fn a_long_fight_stops_allocating() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(8);
        run(&mut director, &mut simulation, &mut world, 240);
        let capacity = director.bolts.capacity();
        let particles = director.particles().capacity();

        for _ in 0..20 {
            simulation.push_command(PlayerCommand::UseAbility(0));
            run(&mut director, &mut simulation, &mut world, 70);
        }
        assert_eq!(director.bolts.capacity(), capacity);
        assert_eq!(director.particles().capacity(), particles);
        assert_eq!(
            director.stats().refused,
            0,
            "combat asked for more effects than the system holds"
        );
    }
}
