//! Gameplay state to `RenderWorld`.
//!
//! ```text
//! GameWorld                         RenderWorld
//! ---------                         -----------
//! position, facing, height   --->   Transform
//! kind, alive                --->   MeshHandle, MaterialHandle
//! radius, height             --->   Sphere bounds
//! health, statuses, cooldowns  X    (never crosses)
//! ```
//!
//! Extraction runs once per rendered frame, after the simulation has advanced. It is a
//! copy, not a synchronization: the `RenderWorld` is rebuilt from gameplay state every
//! frame and holds nothing gameplay reads back.
//!
//! ## Why entities are not respawned every frame
//!
//! Clearing and refilling the `RenderWorld` would be simpler and would also throw away the
//! culler's stable slot ordering and make every render entity's handle change every frame.
//! Instead each gameplay slot keeps its render entity for as long as the gameplay entity
//! lives, and a slot whose generation changed is despawned and respawned — which is exactly
//! the case a stale handle exists to catch.

use engine_core::math::{Quat, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_game::entity::EntityId;
use engine_game::sim::Simulation;
use engine_game::world::EntityKind;
use engine_render_data::{
    InstanceStyle, MaterialHandle, MeshHandle, RenderEntity, RenderEntityDesc, RenderWorld,
};

/// Which mesh and material each kind of thing is drawn with.
///
/// Supplied by the application, because uploading a mesh needs a device and this crate does
/// not have one. Everything here is a handle the renderer issued plus the local bounds it
/// reported, so extraction can place the bounds in world space without asking the renderer
/// anything.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualBindings {
    /// Mesh for the player character.
    pub player_mesh: MeshHandle,
    /// Local bounds of [`VisualBindings::player_mesh`].
    pub player_bounds: Sphere,
    /// Material for the player character.
    pub player_material: MaterialHandle,
    /// Mesh for a training dummy.
    pub dummy_mesh: MeshHandle,
    /// Local bounds of [`VisualBindings::dummy_mesh`].
    pub dummy_bounds: Sphere,
    /// Material for a living dummy.
    pub dummy_material: MaterialHandle,
    /// Material for a dummy that has been killed.
    ///
    /// A second material rather than a colour on the entity: `RenderWorld` carries a
    /// material handle per entity, so "it died" is a different handle, and changing it is
    /// what a respawn of the render entity does.
    pub dead_material: MaterialHandle,
    /// Mesh for the arena floor.
    pub ground_mesh: MeshHandle,
    /// Local bounds of [`VisualBindings::ground_mesh`].
    pub ground_bounds: Sphere,
    /// Material for the arena floor.
    pub ground_material: MaterialHandle,
    /// Mesh used for the target marker under the selected dummy.
    pub marker_mesh: MeshHandle,
    /// Local bounds of [`VisualBindings::marker_mesh`].
    pub marker_bounds: Sphere,
    /// Material for the target marker.
    pub marker_material: MaterialHandle,
}

/// What the last extraction did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VisualStats {
    /// Gameplay entities extracted.
    pub extracted: usize,
    /// Render entities spawned this frame.
    pub spawned: usize,
    /// Render entities despawned this frame.
    pub despawned: usize,
    /// Entities whose material changed because they died or were revived.
    pub restyled: usize,
}

/// One gameplay slot's rendering counterpart.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mapped {
    entity: RenderEntity,
    /// The gameplay generation this mapping was made for. A different one means the slot
    /// has been reused by another entity.
    generation: u32,
    /// Whether the mapping was made for a living entity.
    alive: bool,
}

/// Builds and maintains a `RenderWorld` from a [`Simulation`].
#[derive(Debug)]
pub struct RenderExtractor {
    world: RenderWorld,
    bindings: VisualBindings,
    /// Gameplay slot to render entity. Grows with the world and is never freed.
    mapped: Vec<Option<Mapped>>,
    ground: RenderEntity,
    /// How wide the drawn floor is, so it can be moved without being re-measured.
    ground_size: f32,
    marker: RenderEntity,
    stats: VisualStats,
    /// Half the height of the target marker, so it sits on the floor rather than in it.
    marker_height: f32,
    /// When each gameplay slot was last damaged. Grows with the world; never freed.
    hits: Vec<Option<Hit>>,
}

/// Height above the floor the target marker floats at, in metres.
const MARKER_HEIGHT: f32 = 3.0;

/// Half-extent of the marker, in metres.
const MARKER_SIZE: f32 = 0.4;

/// Darkest a full-health-to-dead tint gets.
///
/// Not zero: a tint that reached black would make a nearly-dead dummy invisible against the
/// arena floor, which reads as a rendering bug rather than as low health.
const HEALTH_TINT_FLOOR: f32 = 0.45;

/// Emissive added to the current target, so selection is visible on the enemy itself.
const TARGET_GLOW: f32 = 0.22;

/// How long a hit flash lasts, in simulated seconds.
///
/// Short enough to read as an impact rather than a state, long enough to survive a frame at
/// any frame rate the engine reaches.
const HIT_FLASH_SECONDS: f32 = 0.18;

/// Emissive added at the peak of a hit flash.
const HIT_FLASH_STRENGTH: f32 = 0.9;

/// How much a hit flash lifts the tint towards white at its peak.
const HIT_FLASH_LIFT: f32 = 0.6;

/// Blends two positions by `alpha`.
///
/// A plain lerp. Positions are metres and a tick is 16.67 ms, so nothing an entity can do in
/// one step makes the straight line between two samples visibly wrong.
#[must_use]
pub fn lerp_position(previous: Vec3, current: Vec3, alpha: f32) -> Vec3 {
    let alpha = alpha.clamp(0.0, 1.0);
    previous + (current - previous) * alpha
}

/// Blends two yaw angles by `alpha`, taking the short way round.
///
/// A plain lerp on angles spins a character the long way round whenever it crosses the
/// wrap point — a turn from +179 to −179 degrees is two degrees of movement and 358 degrees
/// of lerp. Wrapping the difference into (−pi, pi] first is what makes a character that
/// turns past south not whip around through north.
#[must_use]
pub fn lerp_angle(previous: f32, current: f32, alpha: f32) -> f32 {
    let alpha = alpha.clamp(0.0, 1.0);
    let mut difference = (current - previous) % std::f32::consts::TAU;
    if difference > std::f32::consts::PI {
        difference -= std::f32::consts::TAU;
    } else if difference < -std::f32::consts::PI {
        difference += std::f32::consts::TAU;
    }
    previous + difference * alpha
}

/// Where the player is drawn this frame, interpolated between the last two ticks.
///
/// The camera follows this rather than `Simulation::player_position`, so the character and
/// the camera agree about where the character is. Following the raw simulation position
/// would make the camera step at 60 Hz while the character it is watching moved smoothly.
#[must_use]
pub fn interpolated_player_position(simulation: &Simulation) -> Vec3 {
    let world = simulation.world();
    let player = simulation.player().entity;
    let Some(body) = world.body(player) else {
        return Vec3::ZERO;
    };
    let previous = world.previous_body(player).unwrap_or(body);
    lerp_position(
        previous.position,
        body.position,
        simulation.timestep().alpha(),
    )
}

/// When an entity was last damaged, for the hit flash.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hit {
    /// The generation that was hit, so a recycled slot does not inherit the flash.
    generation: u32,
    /// Simulation time of the hit.
    at: f32,
}

impl RenderExtractor {
    /// Creates the extractor and spawns the parts of the scene that are not entities.
    ///
    /// The floor and the target marker exist for the lifetime of the world: the floor never
    /// changes, and the marker is hidden rather than despawned when nothing is targeted,
    /// because a despawn per target change would churn slots for no reason.
    #[must_use]
    pub fn new(bindings: VisualBindings, ground_size: f32) -> Self {
        let mut world = RenderWorld::with_capacity(64);
        let ground = world.spawn(RenderEntityDesc {
            transform: Transform::from_scale(Vec3::new(ground_size, 1.0, ground_size)),
            mesh: bindings.ground_mesh,
            material: bindings.ground_material,
            bounds: bindings.ground_bounds,
            visible: true,
            style: InstanceStyle::NEUTRAL,
        });
        let marker = world.spawn(RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh: bindings.marker_mesh,
            material: bindings.marker_material,
            bounds: bindings.marker_bounds,
            visible: false,
            style: InstanceStyle::NEUTRAL,
        });

        Self {
            world,
            bindings,
            mapped: Vec::with_capacity(64),
            ground,
            ground_size,
            marker,
            stats: VisualStats::default(),
            marker_height: MARKER_HEIGHT,
            hits: Vec::with_capacity(64),
        }
    }

    /// The world the renderer draws.
    #[must_use]
    pub fn render_world(&self) -> &RenderWorld {
        &self.world
    }

    /// Moves the drawn floor to height `y`.
    ///
    /// The floor is a visual, not a collision surface — the simulation's floor is whatever
    /// is in its [`engine_collision::world::CollisionWorld`]. A scene whose own geometry
    /// provides the ground has to be able to move this one out of the way, because two
    /// coplanar surfaces at the same height are a depth-buffer tie and a tie flickers.
    pub fn set_ground_height(&mut self, y: f32) {
        let transform = Transform::from_translation(Vec3::new(0.0, y, 0.0)).with_scale(Vec3::new(
            self.ground_size,
            1.0,
            self.ground_size,
        ));
        self.world
            .set_transform(self.ground, transform, self.bindings.ground_bounds);
    }

    /// The world the renderer draws, for something that adds to it.
    ///
    /// The developer collision view spawns its own entities here rather than being given a
    /// second `RenderWorld` to draw: two worlds would be two culls, two instance buffers
    /// and two draw passes for something that is a handful of boxes.
    pub fn render_world_mut(&mut self) -> &mut RenderWorld {
        &mut self.world
    }

    /// The floor's render entity.
    #[must_use]
    pub fn ground(&self) -> RenderEntity {
        self.ground
    }

    /// What the last extraction did.
    #[must_use]
    pub fn stats(&self) -> VisualStats {
        self.stats
    }

    /// The render entity standing in for a gameplay entity, if it has one.
    #[must_use]
    pub fn render_entity(&self, id: EntityId) -> Option<RenderEntity> {
        self.mapped
            .get(id.index())
            .copied()
            .flatten()
            .filter(|mapped| mapped.generation == id.generation())
            .map(|mapped| mapped.entity)
    }

    /// Rebuilds the `RenderWorld` from the simulation's current state.
    ///
    /// Allocation: after the first frame in which the world reached its size, this only
    /// writes into existing storage — the mapping vector and the `RenderWorld`'s arrays are
    /// both grown, never cleared.
    pub fn extract(&mut self, simulation: &Simulation) -> &RenderWorld {
        let game = simulation.world();
        let mut stats = VisualStats::default();

        if self.mapped.len() < game.slot_count() {
            self.mapped.resize(game.slot_count(), None);
        }

        for slot in 0..self.mapped.len() {
            let id = game.id_at(slot);
            match id {
                Some(id) => {
                    let alive = game.is_alive(id);
                    // Only a *different entity* in this slot justifies a respawn. Dying no
                    // longer does: Milestone 7 gave `RenderWorld` a mutable material and a
                    // per-entity style, so a visual change is two stores rather than a slot
                    // churn. The generation is the only thing a handle cannot survive.
                    let reused = self.mapped[slot]
                        .is_some_and(|mapped| mapped.generation != id.generation());
                    if reused && let Some(mapped) = self.mapped[slot].take() {
                        self.world.despawn(mapped.entity);
                        stats.despawned += 1;
                    }

                    let (mesh, bounds, material) = self.appearance(simulation, id, alive);
                    let transform = self.transform_for(simulation, id, alive);
                    let style = self.style_for(simulation, id, alive);

                    match self.mapped[slot] {
                        Some(mapped) => {
                            self.world.set_transform(mapped.entity, transform, bounds);
                            self.world.set_style(mapped.entity, style);
                            // Cheap and unconditional: comparing first would cost a load and
                            // a branch to avoid a store of the same width.
                            self.world.set_material(mapped.entity, material);
                            if mapped.alive != alive {
                                stats.restyled += 1;
                                self.mapped[slot] = Some(Mapped { alive, ..mapped });
                            }
                        }
                        None => {
                            let entity = self.world.spawn(RenderEntityDesc {
                                transform,
                                mesh,
                                material,
                                bounds,
                                visible: true,
                                style,
                            });
                            self.mapped[slot] = Some(Mapped {
                                entity,
                                generation: id.generation(),
                                alive,
                            });
                            stats.spawned += 1;
                        }
                    }
                    stats.extracted += 1;
                }
                None => {
                    if let Some(mapped) = self.mapped[slot].take() {
                        self.world.despawn(mapped.entity);
                        stats.despawned += 1;
                    }
                }
            }
        }

        self.update_marker(simulation);
        self.stats = stats;
        &self.world
    }

    /// Which mesh, bounds and material an entity is drawn with.
    fn appearance(
        &self,
        simulation: &Simulation,
        id: EntityId,
        alive: bool,
    ) -> (MeshHandle, Sphere, MaterialHandle) {
        match simulation.world().kind(id) {
            Some(EntityKind::Player) => (
                self.bindings.player_mesh,
                self.bindings.player_bounds,
                self.bindings.player_material,
            ),
            _ => (
                self.bindings.dummy_mesh,
                self.bindings.dummy_bounds,
                if alive {
                    self.bindings.dummy_material
                } else {
                    self.bindings.dead_material
                },
            ),
        }
    }

    /// The per-entity visual state an entity is drawn with.
    ///
    /// This is where "what is happening to this one right now" turns into pixels, and it is
    /// the reason Milestone 7 exists: every rule below used to be inexpressible, because the
    /// only per-entity visual channel was the material handle and changing it meant
    /// respawning the render entity.
    ///
    /// Gameplay is not consulted about any of it. `engine-game` has no idea a tint exists.
    fn style_for(&self, simulation: &Simulation, id: EntityId, alive: bool) -> InstanceStyle {
        if !alive {
            return InstanceStyle::NEUTRAL;
        }

        let mut style = InstanceStyle::NEUTRAL;

        // Wounded enemies darken towards their material's own colour rather than towards
        // black: a health tint that reached zero would make a nearly-dead dummy invisible
        // against the arena floor.
        if let Some(health) = simulation.world().health(id) {
            let fraction = health.fraction().clamp(0.0, 1.0);
            let shade = HEALTH_TINT_FLOOR + (1.0 - HEALTH_TINT_FLOOR) * fraction;
            style.tint = [shade, shade * 0.94, shade * 0.90, 1.0];
        }

        // The current target glows. This is what replaces "the marker is the only thing that
        // says which one you are hitting", and it costs no draw call and no respawn.
        if simulation.player().target == Some(id) {
            style.emissive += TARGET_GLOW;
        }

        // A recent hit flashes white. Driven from simulation time, so the flash lasts the
        // same wall-clock duration at 60 FPS and at 1,400.
        if let Some(hit) = self.hits.get(id.index()).copied().flatten()
            && hit.generation == id.generation()
        {
            let age = (simulation.time() as f32 - hit.at).max(0.0);
            if age < HIT_FLASH_SECONDS {
                let strength = 1.0 - age / HIT_FLASH_SECONDS;
                style.emissive += HIT_FLASH_STRENGTH * strength;
                let lift = 1.0 + HIT_FLASH_LIFT * strength;
                style.tint = [
                    (style.tint[0] * lift).min(1.0),
                    (style.tint[1] * lift).min(1.0),
                    (style.tint[2] * lift).min(1.0),
                    style.tint[3],
                ];
            }
        }

        style
    }

    /// Records that an entity was damaged at a simulation time, for the hit flash.
    ///
    /// Called from the combat-event reader rather than from gameplay: the simulation
    /// announces damage as an event and presentation observes it, which is the same
    /// arrangement the damage meter uses.
    pub fn note_hit(&mut self, id: EntityId, at: f32) {
        if self.hits.len() <= id.index() {
            self.hits.resize(id.index() + 1, None);
        }
        self.hits[id.index()] = Some(Hit {
            generation: id.generation(),
            at,
        });
    }

    /// Where and how an entity is drawn.
    ///
    /// Gameplay stores the position of an entity's **feet**; a mesh is centred on its own
    /// origin. Lifting by half the height is the entire difference between the two spaces,
    /// and it lives here rather than in the simulation because it is a property of the
    /// model, not of the character.
    fn transform_for(&self, simulation: &Simulation, id: EntityId, alive: bool) -> Transform {
        let game = simulation.world();
        let Some(body) = game.body(id) else {
            return Transform::IDENTITY;
        };
        let height = game.height(id);
        let width = game.radius(id) * 2.0;

        // Where the entity is *now*, for a value of "now" between the last two ticks.
        let alpha = simulation.timestep().alpha();
        let previous = game.previous_body(id).unwrap_or(body);
        let position = lerp_position(previous.position, body.position, alpha);
        let yaw = lerp_angle(previous.facing_yaw, body.facing_yaw, alpha);

        if alive {
            Transform::from_translation(position + Vec3::new(0.0, height * 0.5, 0.0))
                .with_rotation(Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0))
                .with_scale(Vec3::new(width, height, width))
        } else {
            // A dead dummy lies down. A material change alone reads as a recolour; falling
            // over is what makes a kill legible from across the arena.
            Transform::from_translation(position + Vec3::new(0.0, width * 0.5, 0.0))
                .with_rotation(
                    Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2)
                        * Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0),
                )
                .with_scale(Vec3::new(width, height, width))
        }
    }

    /// Moves the target marker over the selected target, or hides it.
    fn update_marker(&mut self, simulation: &Simulation) {
        let target = simulation
            .player()
            .target
            .filter(|id| simulation.world().is_alive(*id));

        let Some(target) = target else {
            self.world.set_visible(self.marker, false);
            return;
        };
        let Some(body) = simulation.world().body(target) else {
            self.world.set_visible(self.marker, false);
            return;
        };
        // Interpolated like everything else, or the marker would step at 60 Hz above a
        // target that moves smoothly.
        let previous = simulation
            .world()
            .previous_body(target)
            .unwrap_or(body)
            .position;
        let position = lerp_position(previous, body.position, simulation.timestep().alpha());

        let height = simulation.world().height(target);
        let transform = Transform::from_translation(
            position + Vec3::new(0.0, height + self.marker_height, 0.0),
        )
        .with_scale(Vec3::splat(MARKER_SIZE));
        self.world
            .set_transform(self.marker, transform, self.bindings.marker_bounds);
        self.world.set_visible(self.marker, true);
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
    use engine_game::sim::Simulation;

    fn bindings() -> VisualBindings {
        let unit = Sphere::new(Vec3::ZERO, 0.87);
        VisualBindings {
            player_mesh: MeshHandle::new(0, 1),
            player_bounds: unit,
            player_material: MaterialHandle::new(0, 1),
            dummy_mesh: MeshHandle::new(1, 1),
            dummy_bounds: unit,
            dummy_material: MaterialHandle::new(1, 1),
            dead_material: MaterialHandle::new(2, 1),
            ground_mesh: MeshHandle::new(2, 1),
            ground_bounds: Sphere::new(Vec3::ZERO, 1.0),
            ground_material: MaterialHandle::new(3, 1),
            marker_mesh: MeshHandle::new(3, 1),
            marker_bounds: unit,
            marker_material: MaterialHandle::new(4, 1),
        }
    }

    fn extractor() -> RenderExtractor {
        RenderExtractor::new(bindings(), 200.0)
    }

    fn arena_sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    #[test]
    fn every_gameplay_entity_becomes_something_drawable() {
        let simulation = arena_sim();
        let mut extractor = extractor();
        let world = extractor.extract(&simulation);

        // Every entity, plus the floor and the target marker.
        assert_eq!(world.len(), simulation.world().len() + 2);
        assert_eq!(extractor.stats().extracted, simulation.world().len());
        assert_eq!(extractor.stats().spawned, simulation.world().len());
    }

    #[test]
    fn the_player_is_drawn_standing_on_the_floor_facing_where_it_moves() {
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);

        let mut extractor = extractor();
        extractor.extract(&simulation);

        let player = simulation.player().entity;
        let entity = extractor
            .render_entity(player)
            .expect("the player was extracted");
        let transform = extractor.render_world().transforms()[entity.index()];
        let body = simulation.player_body().expect("the player exists");
        let height = simulation.world().height(player);

        // Within the controller's skin width of standing on the floor. Collision keeps the
        // capsule a few millimetres clear of every surface it rests on, and the mesh is
        // drawn where the capsule is rather than where the floor is.
        assert!(
            (transform.translation.y - height * 0.5).abs() < 0.02,
            "the mesh is not standing on the floor: {transform:?}"
        );
        assert!((transform.scale.y - height).abs() < 1e-4);

        // Since Milestone 9 the drawn position is *interpolated*, so it is the blend of the
        // last two ticks rather than the latest one. At alpha 0 — which is where
        // `step_many` leaves the accumulator — that is the previous tick exactly. This is
        // the deliberate one-tick presentation lag; extrapolating instead would overshoot
        // and snap back whenever the character changed direction.
        let previous = simulation
            .world()
            .previous_body(player)
            .expect("history exists");
        let drawn = lerp_position(
            previous.position,
            body.position,
            simulation.timestep().alpha(),
        );
        assert!((transform.translation.z - drawn.z).abs() < 1e-4);
        assert!(
            transform.translation.z < 0.0,
            "moving forward should still have carried it down -Z: {transform:?}"
        );
    }

    #[test]
    fn gameplay_state_does_not_leak_into_the_render_world() {
        // The architectural assertion: `RenderWorld` carries transforms, handles, bounds and
        // visibility, and there is nowhere in it for health to go. What this test can check
        // is that damage alone changes nothing about what is drawn.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let target = simulation.player().target.expect("a dummy is targeted");
        let before = extractor.render_world().transforms()[extractor
            .render_entity(target)
            .expect("the dummy was extracted")
            .index()];

        simulation.world_mut().damage(target, 100.0);
        simulation.step_once();
        extractor.extract(&simulation);

        let after = extractor.render_world().transforms()[extractor
            .render_entity(target)
            .expect("the dummy is still extracted")
            .index()];
        assert_eq!(before, after, "a wounded dummy is drawn no differently");
    }

    #[test]
    fn a_dead_dummy_falls_over_and_changes_material() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let upright = extractor
            .render_entity(target)
            .expect("the dummy was extracted");
        assert_eq!(
            extractor.render_world().materials()[upright.index()],
            bindings().dummy_material
        );

        simulation.world_mut().damage(target, 1_000_000.0);
        simulation.step_once();
        extractor.extract(&simulation);

        let fallen = extractor
            .render_entity(target)
            .expect("the corpse is still drawn");
        assert_eq!(
            extractor.render_world().materials()[fallen.index()],
            bindings().dead_material
        );
        assert_eq!(extractor.stats().restyled, 1);

        let transform = extractor.render_world().transforms()[fallen.index()];
        assert!(
            transform.translation.y < 1.0,
            "the corpse is still standing: {transform:?}"
        );
    }

    #[test]
    fn dying_changes_how_a_dummy_is_drawn_without_respawning_its_render_entity() {
        // Milestone 7, stated as the property it exists for. Phase 3 expressed "it died" by
        // despawning the render entity and respawning it with a different material, which
        // churned a slot for a colour change and invalidated every outstanding handle.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let before = extractor.render_entity(target).expect("it was extracted");

        simulation.world_mut().damage(target, 1_000_000.0);
        simulation.step_once();
        extractor.extract(&simulation);

        let after = extractor.render_entity(target).expect("it is still drawn");
        assert_eq!(after, before, "the render entity survived the death");
        assert_eq!(extractor.stats().spawned, 0, "nothing was respawned");
        assert_eq!(extractor.stats().despawned, 0, "nothing was despawned");
        assert_eq!(
            extractor.stats().restyled,
            1,
            "the change was still noticed"
        );
        assert_eq!(
            extractor.render_world().materials()[after.index()],
            bindings().dead_material
        );
    }

    #[test]
    fn a_wounded_dummy_darkens_without_any_render_churn() {
        // The health tint: a per-entity visual that changes every time damage lands, and
        // therefore the one that would have been unaffordable as a respawn.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let entity = extractor.render_entity(target).expect("it was extracted");
        let healthy = extractor.render_world().styles()[entity.index()].tint[0];

        let max = simulation
            .world()
            .health(target)
            .expect("the dummy has health")
            .max();
        simulation.world_mut().damage(target, max * 0.8);
        extractor.extract(&simulation);

        let wounded = extractor.render_world().styles()[entity.index()].tint[0];
        assert!(
            wounded < healthy,
            "a wounded dummy should darken: {wounded} vs {healthy}"
        );
        assert!(
            wounded >= HEALTH_TINT_FLOOR,
            "it must not fade into the floor: {wounded}"
        );
        assert_eq!(extractor.stats().spawned, 0);
        assert_eq!(extractor.stats().despawned, 0);
    }

    #[test]
    fn the_current_target_glows_and_stops_glowing_when_it_is_dropped() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let entity = extractor.render_entity(target).expect("it was extracted");
        assert!(
            extractor.render_world().styles()[entity.index()].emissive > 0.0,
            "the target should be lit up"
        );

        simulation.player_mut().target = None;
        extractor.extract(&simulation);
        assert_eq!(
            extractor.render_world().styles()[entity.index()].emissive,
            0.0,
            "an unselected dummy stops glowing"
        );
        assert_eq!(extractor.stats().spawned, 0, "selection is not a respawn");
    }

    #[test]
    fn a_hit_flashes_and_fades_on_simulation_time() {
        // Driven from simulated seconds rather than frames, so the flash lasts the same
        // wall-clock time at 60 FPS and at 1,400.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let entity = extractor.render_entity(target).expect("it was extracted");
        let resting = extractor.render_world().styles()[entity.index()].emissive;

        extractor.note_hit(target, simulation.time() as f32);
        extractor.extract(&simulation);
        let flashing = extractor.render_world().styles()[entity.index()].emissive;
        assert!(
            flashing > resting,
            "the hit should brighten it: {flashing} vs {resting}"
        );

        // Past the flash's lifetime it is back to where it started.
        simulation.step_many(60);
        extractor.extract(&simulation);
        let faded = extractor.render_world().styles()[entity.index()].emissive;
        assert!(
            (faded - resting).abs() < 1e-4,
            "the flash should have faded: {faded} vs {resting}"
        );
    }

    #[test]
    fn a_recycled_slot_does_not_inherit_the_previous_occupants_flash() {
        // The generation check on a hit. Without it a fresh dummy would spawn mid-flash.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let old = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        extractor.note_hit(old, simulation.time() as f32);
        simulation.world_mut().despawn(old);
        let new = simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(5.0, 0.0, -5.0),
                100.0,
            ));
        assert_eq!(new.index(), old.index(), "the slot was reused");

        extractor.extract(&simulation);
        let entity = extractor
            .render_entity(new)
            .expect("the new dummy is drawn");
        let style = extractor.render_world().styles()[entity.index()];
        assert_eq!(
            style.emissive, 0.0,
            "a new entity must not inherit a flash aimed at the last one"
        );
    }

    #[test]
    fn a_long_fight_causes_no_render_entity_churn_at_all() {
        // The benchmark-shaped assertion Milestone 7 asks for: visual state changes
        // constantly for hundreds of ticks and the render world never gains or loses a slot.
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(32));
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let slots = extractor.render_world().slot_count();
        let live = extractor.render_world().len();

        let mut churn = 0;
        for tick in 0..600 {
            // Damage something every tick, cycle the target, and kill a few outright.
            if let Some(target) = simulation.player().target {
                simulation.world_mut().damage(target, 3.0);
                extractor.note_hit(target, simulation.time() as f32);
            }
            if tick % 15 == 0 {
                simulation.push_command(PlayerCommand::CycleTarget);
            }
            simulation.step_once();
            extractor.extract(&simulation);
            churn += extractor.stats().spawned + extractor.stats().despawned;
        }

        assert_eq!(churn, 0, "no entity was spawned or despawned in 600 ticks");
        assert_eq!(extractor.render_world().slot_count(), slots);
        assert_eq!(extractor.render_world().len(), live);
    }

    #[test]
    fn a_position_is_blended_by_the_alpha_and_clamped_outside_it() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(10.0, 4.0, -2.0);
        assert_eq!(lerp_position(a, b, 0.0), a);
        assert_eq!(lerp_position(a, b, 1.0), b);
        assert_eq!(lerp_position(a, b, 0.5), Vec3::new(5.0, 2.0, -1.0));
        // An alpha outside [0, 1) would extrapolate, which shows up as an entity that
        // overshoots and snaps back.
        assert_eq!(lerp_position(a, b, 2.0), b);
        assert_eq!(lerp_position(a, b, -1.0), a);
    }

    #[test]
    fn a_yaw_takes_the_short_way_round_the_wrap_point() {
        // The classic interpolation bug: a character turning past south whips around
        // through north because the lerp went the long way.
        use std::f32::consts::PI;
        let just_under = PI - 0.1;
        let just_over = -PI + 0.1;

        let halfway = lerp_angle(just_under, just_over, 0.5);
        // The short way is 0.2 radians across the wrap, so halfway is 0.1 past +pi.
        let expected = just_under + 0.1;
        assert!(
            (halfway - expected).abs() < 1e-4,
            "took the long way: {halfway} rather than {expected}"
        );

        // And the ordinary case is unchanged.
        assert!((lerp_angle(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(lerp_angle(0.4, 0.9, 0.0), 0.4);
    }

    #[test]
    fn a_moving_character_is_drawn_between_its_last_two_ticks() {
        // The property the whole milestone exists for. Two extractions with the same
        // simulation state but different alphas must place the character differently.
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);

        let player = simulation.player().entity;
        let world = simulation.world();
        let previous = world
            .previous_body(player)
            .expect("history exists")
            .position;
        let current = world.body(player).expect("the player exists").position;
        assert_ne!(
            previous, current,
            "a moving character must have moved between ticks"
        );

        // Interpolation covers the whole gap: the drawn position at alpha 0 is the previous
        // tick and at alpha 1 is the current one.
        assert_eq!(lerp_position(previous, current, 0.0), previous);
        assert_eq!(lerp_position(previous, current, 1.0), current);
        let middle = lerp_position(previous, current, 0.5);
        assert!(middle.z > current.z && middle.z < previous.z || middle.z < current.z);
    }

    #[test]
    fn a_still_character_is_drawn_in_exactly_one_place() {
        // Interpolation must not introduce jitter when nothing is moving.
        let mut simulation = arena_sim();
        simulation.step_many(30);
        let player = simulation.player().entity;
        let world = simulation.world();
        let previous = world.previous_body(player).expect("history").position;
        let current = world.body(player).expect("the player").position;
        // "Still" is now still to within an ulp rather than exactly. A grounded character
        // is pushed a skin width out of the floor every tick, and the arithmetic that does
        // it lands on one of two adjacent `f32` values — twenty nanometres apart, which is
        // four orders of magnitude below anything a pixel can show.
        assert!(
            (previous - current).length() < 1e-6,
            "{previous:?} {current:?}"
        );
        for alpha in [0.0, 0.25, 0.5, 0.99] {
            assert!((lerp_position(previous, current, alpha) - current).length() < 1e-6);
        }
    }

    #[test]
    fn the_camera_follows_the_interpolated_position_not_the_stepped_one() {
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);
        // Advance a partial tick so alpha is non-zero without a tick running.
        simulation.advance(std::time::Duration::from_micros(8_000));

        let alpha = simulation.timestep().alpha();
        assert!(alpha > 0.0, "a partial step should leave a remainder");

        let drawn = interpolated_player_position(&simulation);
        let stepped = simulation.player_position();
        assert_ne!(
            drawn, stepped,
            "mid-tick, the drawn position is ahead of the last simulated one"
        );
    }

    #[test]
    fn a_freshly_spawned_entity_does_not_slide_in_from_its_slots_previous_occupant() {
        // Without per-spawn history reset, a recycled slot would interpolate the new entity
        // in from wherever the old one stood.
        let mut simulation = arena_sim();
        let old = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        simulation.world_mut().despawn(old);
        let new = simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(50.0, 0.0, -50.0),
                100.0,
            ));

        let world = simulation.world();
        let previous = world.previous_body(new).expect("history").position;
        let current = world.body(new).expect("the dummy").position;
        assert_eq!(
            previous, current,
            "a new entity starts where it is, not where the last one was"
        );
    }

    #[test]
    fn interpolation_is_deterministic() {
        // Two identical runs must place the character identically, or a replay would render
        // differently from the run it recorded.
        let build = || {
            let mut simulation = arena_sim();
            for _ in 0..120 {
                simulation.push_command(PlayerCommand::Move {
                    forward: 1.0,
                    right: 0.4,
                    yaw: 0.3,
                    face_camera: false,
                });
                simulation.step_once();
            }
            simulation.advance(std::time::Duration::from_micros(5_000));
            interpolated_player_position(&simulation)
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn the_drawn_floor_can_be_moved_out_of_a_scene_that_brings_its_own() {
        // A scene whose geometry has a face at y = 0 must be able to move this plane away
        // from y = 0. Two coplanar surfaces are a depth-buffer tie, and a tie flickers.
        let mut extractor = extractor();
        let ground = extractor.ground();
        assert_eq!(
            extractor.render_world().transforms()[ground.index()].translation,
            Vec3::ZERO
        );

        extractor.set_ground_height(-15.0);
        let moved = extractor.render_world().transforms()[ground.index()];
        assert_eq!(moved.translation, Vec3::new(0.0, -15.0, 0.0));
        assert_eq!(
            moved.scale,
            Vec3::new(200.0, 1.0, 200.0),
            "moving the floor must not resize it"
        );

        // And extraction, which rewrites every entity it owns, leaves it where it was put.
        extractor.extract(&arena_sim());
        assert_eq!(
            extractor.render_world().transforms()[ground.index()].translation,
            Vec3::new(0.0, -15.0, 0.0)
        );
    }

    #[test]
    fn a_despawned_entity_stops_being_drawn() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let before = extractor.render_world().len();

        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        simulation.world_mut().despawn(dummy);
        extractor.extract(&simulation);

        assert_eq!(extractor.render_world().len(), before - 1);
        assert_eq!(extractor.stats().despawned, 1);
        assert_eq!(extractor.render_entity(dummy), None);
    }

    #[test]
    fn a_reused_gameplay_slot_gets_its_own_render_entity() {
        // The stale-handle case, in the one place both handle spaces meet: a new dummy in a
        // recycled slot must not inherit the previous occupant's render entity.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let old = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        let old_render = extractor.render_entity(old).expect("it was extracted");
        simulation.world_mut().despawn(old);
        let new = simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(5.0, 0.0, -5.0),
                100.0,
            ));
        assert_eq!(new.index(), old.index(), "the slot was reused");

        extractor.extract(&simulation);
        assert_eq!(extractor.render_entity(old), None, "the old id is stale");
        let new_render = extractor
            .render_entity(new)
            .expect("the new dummy is drawn");
        assert_ne!(new_render, old_render);
    }

    #[test]
    fn the_target_marker_follows_the_target_and_hides_when_there_is_none() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let marker = extractor.marker;
        assert!(extractor.render_world().is_drawable(marker.index()));
        let target = simulation.player().target.expect("a dummy is targeted");
        let target_position = simulation
            .world()
            .position(target)
            .expect("the dummy exists");
        let marker_position = extractor.render_world().transforms()[marker.index()].translation;
        assert!((marker_position.x - target_position.x).abs() < 1e-4);
        assert!(marker_position.y > target_position.y, "it floats above");

        // Kill every dummy: nothing is targetable and the marker goes away.
        let dummies: Vec<_> = arena::dummies(simulation.world()).collect();
        for dummy in dummies {
            simulation.world_mut().damage(dummy, 1_000_000.0);
        }
        simulation.step_once();
        extractor.extract(&simulation);
        assert!(!extractor.render_world().is_drawable(marker.index()));
    }

    #[test]
    fn a_steady_scene_stops_allocating() {
        // Milestone 25: extraction is a copy into storage that already exists.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        for _ in 0..10 {
            extractor.extract(&simulation);
        }
        let mapped = extractor.mapped.capacity();
        let slots = extractor.render_world().slot_count();

        for _ in 0..1_000 {
            simulation.push_command(PlayerCommand::Move {
                forward: 1.0,
                right: 0.3,
                yaw: 0.2,
                face_camera: false,
            });
            simulation.step_once();
            extractor.extract(&simulation);
        }

        assert_eq!(extractor.mapped.capacity(), mapped);
        assert_eq!(extractor.render_world().slot_count(), slots);
        assert_eq!(extractor.stats().spawned, 0);
        assert_eq!(extractor.stats().despawned, 0);
    }

    #[test]
    fn a_thousand_dummies_extract_without_a_device() {
        let simulation = arena::training_arena(&ArenaConfig::with_group(1_000));
        let mut extractor = extractor();
        let world = extractor.extract(&simulation);
        assert_eq!(world.len(), 1_002 + 2);
    }
}
