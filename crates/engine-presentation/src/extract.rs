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
use engine_render_data::{MaterialHandle, MeshHandle, RenderEntity, RenderEntityDesc, RenderWorld};

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
    marker: RenderEntity,
    stats: VisualStats,
    /// Half the height of the target marker, so it sits on the floor rather than in it.
    marker_height: f32,
}

/// Height above the floor the target marker floats at, in metres.
const MARKER_HEIGHT: f32 = 3.0;

/// Half-extent of the marker, in metres.
const MARKER_SIZE: f32 = 0.4;

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
        });
        let marker = world.spawn(RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh: bindings.marker_mesh,
            material: bindings.marker_material,
            bounds: bindings.marker_bounds,
            visible: false,
        });

        Self {
            world,
            bindings,
            mapped: Vec::with_capacity(64),
            ground,
            marker,
            stats: VisualStats::default(),
            marker_height: MARKER_HEIGHT,
        }
    }

    /// The world the renderer draws.
    #[must_use]
    pub fn render_world(&self) -> &RenderWorld {
        &self.world
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
                    let stale = self.mapped[slot].is_some_and(|mapped| {
                        mapped.generation != id.generation() || mapped.alive != alive
                    });
                    if stale {
                        // Either another entity took the slot, or this one died: both mean a
                        // different material, and a `RenderWorld` entity's material is fixed
                        // at spawn.
                        if let Some(mapped) = self.mapped[slot].take() {
                            self.world.despawn(mapped.entity);
                            stats.despawned += 1;
                            if game.contains(id) {
                                stats.restyled += 1;
                            }
                        }
                    }

                    let (mesh, bounds, material) = self.appearance(simulation, id, alive);
                    let transform = self.transform_for(simulation, id, alive);

                    match self.mapped[slot] {
                        Some(mapped) => {
                            self.world.set_transform(mapped.entity, transform, bounds);
                        }
                        None => {
                            let entity = self.world.spawn(RenderEntityDesc {
                                transform,
                                mesh,
                                material,
                                bounds,
                                visible: true,
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

        if alive {
            Transform::from_translation(body.position + Vec3::new(0.0, height * 0.5, 0.0))
                .with_rotation(Quat::from_yaw_pitch_roll(body.facing_yaw, 0.0, 0.0))
                .with_scale(Vec3::new(width, height, width))
        } else {
            // A dead dummy lies down. A material change alone reads as a recolour; falling
            // over is what makes a kill legible from across the arena.
            Transform::from_translation(body.position + Vec3::new(0.0, width * 0.5, 0.0))
                .with_rotation(
                    Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2)
                        * Quat::from_yaw_pitch_roll(body.facing_yaw, 0.0, 0.0),
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
        let Some(position) = simulation.world().position(target) else {
            self.world.set_visible(self.marker, false);
            return;
        };

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

        assert!(
            (transform.translation.y - height * 0.5).abs() < 1e-4,
            "the mesh is not standing on the floor: {transform:?}"
        );
        assert!((transform.translation.z - body.position.z).abs() < 1e-4);
        assert!((transform.scale.y - height).abs() < 1e-4);
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

        simulation.world_mut().damage(target, 100_000.0);
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
