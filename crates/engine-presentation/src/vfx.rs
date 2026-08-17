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

/// How far above the ground a flat effect is drawn, in metres.
///
/// Coplanar with the floor is a depth-buffer tie and flickers. Slightly higher than the
/// targeting indicator's own lift, so an area that is burning draws over the circle that
/// placed it rather than fighting it.
const GROUND_LIFT: f32 = 0.06;

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

/// Colour of the mage's own defensive shell.
const SHIELD_TINT: [f32; 4] = [1.0, 0.62, 0.30, 1.0];

/// Colour of the Fire Mastery aura, which reuses the shell at a different size and hue.
const MASTERY_TINT: [f32; 4] = [1.0, 0.90, 0.45, 1.0];

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
    shapes: [Option<ShapeBinding>; 6],
    material: MaterialHandle,
}

impl VfxVisuals {
    /// An empty binding set drawing with `material`.
    #[must_use]
    pub fn new(material: MaterialHandle) -> Self {
        Self {
            shapes: [None; 6],
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
    pools: [Vec<RenderEntity>; 6],
    /// This frame's particles, rebuilt every frame and never freed.
    particles: ParticleDrawList,
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
            pools: [const { Vec::new() }; 6],
            particles: ParticleDrawList::default(),
            bolts: Vec::with_capacity(32),
            ignited: Vec::with_capacity(64),
            areas: Vec::with_capacity(16),
            telegraphs: Vec::with_capacity(16),
            enemy_casts: Vec::with_capacity(16),
            shield: None,
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
                source, ability, ..
            } => {
                self.observe_ability(source, ability, simulation);
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
                ..
            } => {
                let spawn = match shape {
                    TelegraphShape::Circle { radius } => Some(
                        VfxSpawn::at(VfxId::Telegraph, anchor)
                            .with_radius(radius.max(0.5))
                            .lasting(delay.max(0.05)),
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
            CombatEvent::Damage(_) | CombatEvent::Healing { .. } | CombatEvent::Jumped { .. } => {}
        }
    }

    /// The effects an ability produces at the moment it is used.
    fn observe_ability(
        &mut self,
        source: EntityId,
        ability: engine_game::AbilityId,
        simulation: &Simulation,
    ) {
        let Some(fire) = ability.fire() else {
            return;
        };
        let definition = fire.definition();
        let origin = simulation.world().position(source).unwrap_or(Vec3::ZERO);
        let yaw = simulation
            .world()
            .body(source)
            .map_or(0.0, |body| body.facing_yaw);

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
    pub fn update(&mut self, simulation: &Simulation, delta: f32, world: &mut RenderWorld) {
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
        self.system.update(delta, |key| {
            let id = game.id_at(key as usize)?;
            if !game.is_alive(id) {
                return None;
            }
            game.position(id)
        });

        self.particles.clear();
        self.system.write_particles(&mut self.particles);
        self.sync_entities(world);
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
    fn sync_entities(&mut self, world: &mut RenderWorld) {
        let mut used = [0_usize; 6];
        for index in 0..self.system.mesh_draws().len() {
            let draw = self.system.mesh_draws()[index];
            let shape = draw.shape.index();
            let Some(binding) = self.visuals.get(draw.shape) else {
                continue;
            };
            let transform = transform_of(&draw);
            let style = style_of(&draw);

            let slot = used[shape];
            if slot < self.pools[shape].len() {
                let entity = self.pools[shape][slot];
                world.set_transform(entity, transform, binding.bounds);
                world.set_style(entity, style);
                world.set_visible(entity, true);
            } else {
                let entity = world.spawn(RenderEntityDesc {
                    transform,
                    mesh: binding.mesh,
                    material: self.visuals.material(),
                    bounds: binding.bounds,
                    visible: true,
                    style,
                });
                self.pools[shape].push(entity);
            }
            used[shape] += 1;
        }

        for (shape, pool) in self.pools.iter().enumerate() {
            for entity in pool.iter().skip(used[shape]) {
                world.set_visible(*entity, false);
            }
        }
    }
}

/// Which state-driven effect is being synchronised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncedAura {
    Shield,
    Mastery,
    Cast,
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
            director.update(simulation, 1.0 / 60.0, world);
        }
    }

    #[test]
    fn a_cast_that_lands_produces_a_bolt_and_then_an_impact() {
        let mut director = VfxDirector::new(visuals());
        let mut world = RenderWorld::new();
        let mut simulation = arena(1);

        simulation.push_command(PlayerCommand::UseAbility(0));
        // A one-second cast, then the flight. Stepped one frame at a time so the moment the
        // bolt exists can be seen rather than inferred.
        let mut seen_in_flight = false;
        for _ in 0..70 {
            simulation.step_once();
            director.observe(&simulation);
            director.update(&simulation, 1.0 / 60.0, &mut world);
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
        director.update(&simulation, 1.0 / 60.0, &mut world);
        assert_eq!(director.areas.len(), 1, "the fire was not drawn");

        // Past the zone's duration: the gameplay zone ends, the event says so, and the
        // effect goes with it.
        run(&mut director, &mut simulation, &mut world, 60 * 8);
        assert!(
            director.areas.is_empty(),
            "the fire outlived the zone that was burning"
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
        director.update(&simulation, 1.0 / 60.0, &mut world);

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
        run(&mut director, &mut simulation, &mut world, 70);
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
        director.update(&simulation, 1.0 / 60.0, &mut world);
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
        director.update(&simulation, 1.0 / 60.0, &mut world);
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
        director.update(&simulation, 1.0 / 60.0, &mut world);
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
