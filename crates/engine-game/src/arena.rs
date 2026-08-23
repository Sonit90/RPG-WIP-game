//! The training arena: a floor, a player, a single dummy and a dummy group.
//!
//! ```text
//!                          .  .  .  .        <- dummy group (4), for area damage
//!                        .  .  .  .
//!
//!                 [ D ]                      <- single dummy, for single-target damage
//!
//!                 (P)                        <- player spawn
//! ```
//!
//! Everything here is gameplay: positions, health and factions. The floor is not an entity
//! — it is the `ground_height` the character controller stands on, and the visible plane is
//! the presentation layer's business.
//!
//! The layout is chosen so the two combat questions are answerable by *playing*: the group
//! is inside one blast radius of itself and the single dummy is well outside it, so an area
//! ability visibly hits four dummies and visibly misses the fifth.

use engine_core::math::Vec3;

use crate::entity::EntityId;
use crate::movement::MovementConfig;
use crate::sim::{SimConfig, Simulation};
use crate::world::{EntityDesc, EntityKind, GameWorld};

/// How the arena is laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct ArenaConfig {
    /// Where the player starts.
    pub player_spawn: Vec3,
    /// Where the lone dummy stands.
    pub single_dummy: Vec3,
    /// Centre of the dummy group.
    pub group_center: Vec3,
    /// How many dummies are in the group.
    pub group_count: usize,
    /// Metres between dummies in the group.
    pub group_spacing: f32,
    /// Health every dummy has.
    ///
    /// High enough that a full rotation does not delete the target before its damage-over-
    /// time has run, which is what makes the meter's numbers legible.
    pub dummy_health: f32,
    /// Where the friendly dummy the healing kit is practised on stands.
    pub healing_dummy: Vec3,
    /// The size of that dummy's health pool.
    ///
    /// Two million, and it stands at half of it: large enough that a whole healing rotation
    /// — every heal over time, a Tranquility and a Swiftmend — never tops it up, which is
    /// what makes the numbers on it legible.
    pub healing_dummy_health: f32,
    /// Half-width of the square the player is confined to.
    pub arena_half_extent: f32,
}

impl Default for ArenaConfig {
    fn default() -> Self {
        Self {
            player_spawn: Vec3::ZERO,
            single_dummy: Vec3::new(0.0, 0.0, -12.0),
            group_center: Vec3::new(14.0, 0.0, -18.0),
            group_count: 4,
            group_spacing: 3.0,
            dummy_health: 200_000.0,
            // In the main hall, beside the player's spawn and well clear of both the lone
            // dummy in front of them and the group off to the side: it is a target to stand
            // next to, not one to fight.
            healing_dummy: Vec3::new(-6.0, 0.0, -4.0),
            healing_dummy_health: 2_000_000.0,
            arena_half_extent: 40.0,
        }
    }
}

impl ArenaConfig {
    /// The configuration an environment scene implies, with `count` dummies in the group.
    ///
    /// The seam between the level and the fight. The scene owns the spawn points, because
    /// the scene is what put a boulder somewhere and therefore what knows where there is
    /// room; gameplay owns what stands on them. Nothing in this function names a mesh, and
    /// nothing in the scene names a dummy.
    ///
    /// The arena's half-extent comes from the scene too, so the collision wall and the
    /// drawn kerb cannot disagree about where the arena ends — which they would the first
    /// time one of the two numbers was changed on its own.
    #[must_use]
    pub fn from_environment(scene: &engine_environment::EnvironmentScene, count: usize) -> Self {
        let base = Self::with_group(count);
        Self {
            player_spawn: scene.spawns.player,
            single_dummy: scene.spawns.featured_dummy,
            group_center: scene.spawns.group_center,
            group_spacing: scene.spawns.group_spacing,
            // The scene's own bound, unless a benchmark asked for more dummies than fit in
            // it — in which case the group would otherwise be clamped into a heap against
            // the wall and an area ability would hit all of it.
            arena_half_extent: base.arena_half_extent.max(scene.half_extent),
            ..base
        }
    }

    /// A configuration with `count` dummies in the group.
    ///
    /// The arena grows with the group so a benchmark's dummies are not clamped into a heap
    /// against the wall by the character controller's confinement — which would make an
    /// area ability at 1,000 dummies hit all of them and measure the wrong thing.
    #[must_use]
    pub fn with_group(count: usize) -> Self {
        let base = Self::default();
        let spacing = base.group_spacing;
        let columns = grid_columns(count);
        let half_span = columns as f32 * spacing * 0.5;
        Self {
            group_count: count,
            arena_half_extent: base.arena_half_extent.max(half_span + 20.0),
            ..base
        }
    }

    /// The movement configuration this arena implies.
    #[must_use]
    pub fn movement(&self) -> MovementConfig {
        MovementConfig {
            arena_half_extent: self.arena_half_extent,
            ..MovementConfig::default()
        }
    }
}

/// Columns in the grid a group of `count` dummies is laid out in.
///
/// A square-ish grid, so a thousand dummies is a field rather than a line a kilometre long.
fn grid_columns(count: usize) -> usize {
    let mut columns = 1;
    while columns * columns < count {
        columns += 1;
    }
    columns.max(1)
}

/// Everything the arena spawned, so a caller can name the parts without searching.
#[derive(Debug, Clone)]
pub struct Arena {
    /// The player's entity.
    pub player: EntityId,
    /// The lone dummy.
    pub single_dummy: EntityId,
    /// The dummies in the group, in spawn order.
    pub group: Vec<EntityId>,
    /// The friendly dummy the healing kit is practised on, at half health.
    pub healing_dummy: EntityId,
}

/// Builds a simulation holding the training arena.
#[must_use]
pub fn training_arena(config: &ArenaConfig) -> Simulation {
    let mut simulation = Simulation::new(SimConfig {
        movement: config.movement(),
        ..SimConfig::default()
    });
    let arena = populate(simulation.world_mut(), config);
    build_collision(simulation.collision_world_mut(), config);
    simulation.set_player_entity(arena.player);
    // In the party, because that is what a party heal reaches and what the party frames
    // draw: a friendly target standing in the room that no frame showed would be a target
    // the player could only find by clicking on it.
    simulation.party_mut().join(arena.healing_dummy);
    simulation.select_nearest_target();
    simulation
}

/// Height of the arena's boundary wall, in metres.
///
/// Taller than a jump, so the wall is a wall rather than a hurdle.
const WALL_HEIGHT: f32 = 4.0;

/// Thickness of the arena's boundary wall, in metres.
const WALL_THICKNESS: f32 = 1.0;

/// Gives the arena its floor and its four boundary walls.
///
/// The floor was previously a number (`ground_height`) and the walls were a clamp on X and
/// Z. They are now geometry, which is what makes the boundary behave like a boundary:
/// running into it at an angle slides along it instead of stopping dead on one axis.
///
/// The clamp is still there behind this, in [`crate::movement::step_collided`]. It costs
/// four comparisons and it is what guarantees a 1,000-dummy benchmark scenario stays inside
/// the arena it was measured in.
pub fn build_collision(world: &mut engine_collision::world::CollisionWorld, config: &ArenaConfig) {
    world.clear();
    world.add_ground(0.0, "ARENA FLOOR");

    let limit = config.arena_half_extent;
    let outer = limit + WALL_THICKNESS;
    let top = WALL_HEIGHT;
    // Each wall spans the full outer square, so the four of them overlap at the corners
    // rather than leaving a gap the width of a wall at each one.
    world.add_box(
        Vec3::new(limit, 0.0, -outer),
        Vec3::new(outer, top, outer),
        "ARENA WALL +X",
    );
    world.add_box(
        Vec3::new(-outer, 0.0, -outer),
        Vec3::new(-limit, top, outer),
        "ARENA WALL -X",
    );
    world.add_box(
        Vec3::new(-outer, 0.0, limit),
        Vec3::new(outer, top, outer),
        "ARENA WALL +Z",
    );
    world.add_box(
        Vec3::new(-outer, 0.0, -outer),
        Vec3::new(outer, top, -limit),
        "ARENA WALL -Z",
    );
    world.rebuild_broad_phase();
}

/// Adds every blocking environment object to a collision world.
///
/// ```text
/// EnvironmentObject                        CollisionWorld
///   collision: Box { centre, half }  ──►   Shape::Aabb   (unrotated placement)
///   transform: position + yaw + scale ─►   Shape::Obb    (yawed placement)
///   collision: None                  ──►   nothing at all
/// ```
///
/// # Why the shape is chosen here rather than authored
///
/// An axis-aligned box is a cheaper narrow-phase test than an oriented one, and most of the
/// arena is unrotated. Which of the two a placement needs is a consequence of its transform,
/// not a decision an author should have to make — so it is made once, here, by comparing the
/// placement's rotation against an epsilon. A quarter of a degree of yaw is not worth an
/// oriented box.
///
/// # Why the broad phase is not rebuilt
///
/// It is left dirty, exactly as [`build_collision`] leaves it: rebuilding after every insert
/// makes building a level quadratic. The caller rebuilds once when the whole world is
/// assembled, and forgetting costs speed rather than correctness — a dirty world answers
/// with a linear scan.
///
/// Returns how many shapes were added.
pub fn add_environment_collision(
    world: &mut engine_collision::world::CollisionWorld,
    scene: &engine_environment::EnvironmentScene,
) -> usize {
    use engine_collision::shape::{Obb, Shape};
    use engine_collision::world::StaticDesc;

    /// Below this, a placement's rotation is not worth an oriented box.
    ///
    /// The magnitude of a quaternion's vector part is `sin(angle / 2)`, so this is about a
    /// quarter of a degree of yaw.
    const YAW_EPSILON: f32 = 2.0e-3;

    let mut added = 0;
    for object in &scene.objects {
        let Some(placed) = object.world_collision() else {
            continue;
        };
        // A degenerate box would be a shape every query has to test and no query can ever
        // touch. It means an author wrote a zero extent, which is worth a line in the log.
        if placed.half_extents.x <= 0.0
            || placed.half_extents.y <= 0.0
            || placed.half_extents.z <= 0.0
        {
            log::warn!(
                "environment object {} has a collision box with no volume: {:?}",
                object.asset,
                placed.half_extents
            );
            continue;
        }

        let shape = if placed.rotation_magnitude() < YAW_EPSILON {
            let (min, max) = placed.min_max();
            Shape::box_between(min, max)
        } else {
            Shape::Obb(Obb::new(
                placed.center,
                placed.half_extents,
                placed.rotation,
            ))
        };
        // The asset's name, so the collision debug view and a contact report say *what* was
        // hit rather than "a box". `&'static str` because an asset id already is one.
        world.add_static(StaticDesc::new(shape).labelled(object.asset.name()));
        added += 1;
    }
    added
}

/// Builds a simulation holding an environment scene and the fight that happens in it.
///
/// The whole of "load a level": the scene supplies the layout, the collision and the spawn
/// points; [`ArenaConfig::from_environment`] turns its spawns into gameplay's; and the
/// arena's own floor and boundary wall are added underneath. Adding a second arena is
/// calling this with a different scene — see `phase_environment.md`.
#[must_use]
pub fn environment_arena(
    scene: &engine_environment::EnvironmentScene,
    group_count: usize,
) -> Simulation {
    let config = ArenaConfig::from_environment(scene, group_count);
    let mut simulation = Simulation::new(SimConfig {
        movement: config.movement(),
        ..SimConfig::default()
    });
    let arena = populate(simulation.world_mut(), &config);

    let world = simulation.collision_world_mut();
    // The floor and the four boundary walls first, then everything the scene brought. The
    // wall is what confines the player; the scene's kerb stands just inside it and is what
    // the player sees. Two descriptions of the boundary, and they agree because
    // `ArenaConfig::from_environment` took its half-extent from the same scene.
    build_collision(world, &config);
    let statics = add_environment_collision(world, scene);
    world.rebuild_broad_phase();
    log::info!(
        "environment '{}': {} objects, {statics} collision shapes",
        scene.name,
        scene.len()
    );

    simulation.set_player_entity(arena.player);
    simulation.party_mut().join(arena.healing_dummy);
    simulation.select_nearest_target();
    simulation
}

/// Fills an existing world with the arena and returns what it spawned.
///
/// Separate from [`training_arena`] so a benchmark can rebuild the world of a simulation it
/// already owns, and so a test can build a world with no simulation at all.
/// An arena with an encounter in it instead of a group of dummies.
///
/// The same floor, the same player, the same collision - and a fight. Built here rather than
/// in the application for the same reason `training_arena` is: a scene is gameplay data, and
/// a headless test has to be able to build the identical one.
#[must_use]
pub fn encounter_arena(config: &ArenaConfig, encounter: crate::encounter::Encounter) -> Simulation {
    let mut simulation = Simulation::new(SimConfig {
        movement: config.movement(),
        ..SimConfig::default()
    });
    let arena = populate(simulation.world_mut(), config);
    simulation.set_player_entity(arena.player);
    simulation.party_mut().join(arena.healing_dummy);
    build_collision(simulation.collision_world_mut(), config);
    simulation.set_encounter(encounter);
    simulation
}

/// The encounter the demo scene and the tests use: one melee enemy and one caster.
///
/// Placed so that walking forward pulls the brute first and the caster second, which is what
/// makes the two behaviours legible side by side rather than at once.
#[must_use]
pub fn demo_encounter(config: &ArenaConfig) -> crate::encounter::Encounter {
    use crate::encounter::{Encounter, EnemySpawn};
    use crate::enemy::EnemyKind;

    let origin = config.player_spawn;
    Encounter::with_spawns(
        "SKIRMISH",
        vec![
            EnemySpawn::new(EnemyKind::Melee, origin + Vec3::new(-3.0, 0.0, -16.0)),
            EnemySpawn::new(EnemyKind::Caster, origin + Vec3::new(6.0, 0.0, -24.0)),
        ],
    )
}

/// A pack, for the stress scenarios: `count` enemies alternating melee and caster.
///
/// Laid out on a grid behind the same spacing the dummy group uses, so a hundred enemies
/// stand in a block the player can see all of rather than a line to the horizon.
#[must_use]
pub fn pack_encounter(config: &ArenaConfig, count: usize) -> crate::encounter::Encounter {
    use crate::encounter::{Encounter, EnemySpawn};
    use crate::enemy::EnemyKind;

    let columns = grid_columns(count);
    let spacing = config.group_spacing.max(2.5);
    let mut spawns = Vec::with_capacity(count);
    for index in 0..count {
        let column = index % columns;
        let row = index / columns;
        let offset = Vec3::new(
            (column as f32 - (columns as f32 - 1.0) * 0.5) * spacing,
            0.0,
            -18.0 - row as f32 * spacing,
        );
        let kind = if index % 2 == 0 {
            EnemyKind::Melee
        } else {
            EnemyKind::Caster
        };
        spawns.push(EnemySpawn::new(kind, config.player_spawn + offset));
    }
    Encounter::with_spawns("PACK", spawns)
}

/// Fills an existing world with the arena and returns what it spawned.
///
/// Separate from [`training_arena`] so a benchmark can rebuild the world of a simulation it
/// already owns, and so a test can build a world with no simulation at all.
pub fn populate(world: &mut GameWorld, config: &ArenaConfig) -> Arena {
    world.clear();

    let player = world.spawn(EntityDesc::player(config.player_spawn));
    let single_dummy = world.spawn(EntityDesc::dummy(config.single_dummy, config.dummy_health));
    let healing_dummy = spawn_healing_dummy(world, config.healing_dummy, config.healing_dummy_health);
    let mut group = Vec::with_capacity(config.group_count);
    spawn_group(world, config, &mut group);

    Arena {
        player,
        single_dummy,
        group,
        healing_dummy,
    }
}

/// Spawns the friendly dummy and puts it at half health.
///
/// The health is set through [`GameWorld::restore`] rather than by writing a number here, so
/// the state it starts in and the state a reset key puts it back to are the same rule in the
/// same place.
pub fn spawn_healing_dummy(world: &mut GameWorld, position: Vec3, max_health: f32) -> EntityId {
    let dummy = world.spawn(EntityDesc::healing_dummy(position, max_health));
    world.restore(dummy);
    dummy
}

/// Spawns the dummy group into `out`.
///
/// A deterministic grid centred on `group_center`: the same count always produces the same
/// arena, so two benchmark runs measure the same scene rather than two different ones.
pub fn spawn_group(world: &mut GameWorld, config: &ArenaConfig, out: &mut Vec<EntityId>) {
    let columns = grid_columns(config.group_count);
    let rows = config.group_count.div_ceil(columns);

    for index in 0..config.group_count {
        let column = index % columns;
        let row = index / columns;
        let offset = Vec3::new(
            (column as f32 - (columns as f32 - 1.0) * 0.5) * config.group_spacing,
            0.0,
            (row as f32 - (rows as f32 - 1.0) * 0.5) * config.group_spacing,
        );
        out.push(world.spawn(EntityDesc::dummy(
            config.group_center + offset,
            config.dummy_health,
        )));
    }
}

/// Every dummy in the world, in slot order.
///
/// Used by the reset control and by the presentation layer's summary line.
pub fn dummies(world: &GameWorld) -> impl Iterator<Item = EntityId> + '_ {
    world
        .entities()
        .filter(|id| world.kind(*id) == Some(EntityKind::Dummy))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::world::horizontal_distance;

    #[test]
    fn the_arena_has_a_player_a_lone_dummy_and_a_group() {
        let config = ArenaConfig::default();
        let simulation = training_arena(&config);
        let world = simulation.world();

        // The player, the lone dummy, the friendly one, and the group.
        assert_eq!(world.len(), 3 + config.group_count);
        assert_eq!(dummies(world).count(), 1 + config.group_count);
        // The friendly dummy is not a dummy for the purposes of the reset key: it is in the
        // player's own faction, in the party, and standing at half health.
        let healing = simulation
            .world()
            .entities()
            .find(|id| world.kind(*id) == Some(EntityKind::HealingDummy))
            .expect("the main hall has a healing dummy");
        assert!(simulation.party().contains(healing));
        let health = world.health(healing).expect("it has health");
        assert!((health.max() - config.healing_dummy_health).abs() < 1.0);
        assert!((health.fraction() - 0.5).abs() < 1e-3, "{}", health.fraction());
        assert_eq!(
            world.kind(simulation.player().entity),
            Some(EntityKind::Player)
        );
        assert_eq!(world.position(simulation.player().entity), Some(Vec3::ZERO));
    }

    #[test]
    fn the_arena_starts_with_the_nearest_dummy_targeted() {
        let simulation = training_arena(&ArenaConfig::default());
        let target = simulation.player().target.expect("a dummy is targeted");
        assert_eq!(
            simulation.world().position(target),
            Some(ArenaConfig::default().single_dummy),
            "the lone dummy is the nearest"
        );
    }

    #[test]
    fn the_group_is_inside_one_blast_of_itself_and_the_lone_dummy_is_not() {
        // The layout property the arena exists for: an area ability must visibly hit the
        // group and visibly miss the fifth dummy.
        let config = ArenaConfig::default();
        let mut world = GameWorld::new();
        let arena = populate(&mut world, &config);
        let radius = crate::firemage::FireAbility::FlameCone.definition().range;

        let centre = world
            .position(arena.group[0])
            .expect("the group was spawned");
        for member in &arena.group {
            let position = world.position(*member).expect("the group was spawned");
            assert!(
                horizontal_distance(centre, position) <= radius,
                "{member:?} is outside the blast that is meant to include it"
            );
        }

        let single = world
            .position(arena.single_dummy)
            .expect("the lone dummy was spawned");
        assert!(
            horizontal_distance(centre, single) > radius,
            "the lone dummy is inside the group's blast radius"
        );
    }

    #[test]
    fn every_dummy_is_hostile_alive_and_on_the_floor() {
        let mut world = GameWorld::new();
        populate(&mut world, &ArenaConfig::default());
        for dummy in dummies(&world) {
            assert!(world.is_alive(dummy));
            assert_eq!(world.faction(dummy), Some(crate::combat::Faction::Hostile));
            assert_eq!(world.position(dummy).map(|p| p.y), Some(0.0));
        }
    }

    #[test]
    fn the_layout_is_deterministic() {
        let config = ArenaConfig::default();
        let first = training_arena(&config);
        let second = training_arena(&config);

        let positions = |simulation: &Simulation| -> Vec<Vec3> {
            simulation
                .world()
                .entities()
                .filter_map(|id| simulation.world().position(id))
                .collect()
        };
        assert_eq!(positions(&first), positions(&second));
    }

    #[test]
    fn a_large_group_is_a_grid_that_fits_inside_the_arena() {
        // The benchmark scenario: a thousand dummies must be a field, not a heap clamped
        // against the arena wall.
        let config = ArenaConfig::with_group(1_000);
        let mut world = GameWorld::new();
        let arena = populate(&mut world, &config);

        assert_eq!(arena.group.len(), 1_000);
        for dummy in &arena.group {
            let position = world.position(*dummy).expect("the group was spawned");
            assert!(
                position.x.abs() <= config.arena_half_extent
                    && position.z.abs() <= config.arena_half_extent,
                "{position:?} is outside the arena"
            );
        }

        // And no two dummies occupy the same point, which a grid with a wrong stride does.
        let mut points: Vec<(i32, i32)> = arena
            .group
            .iter()
            .filter_map(|id| world.position(*id))
            .map(|position| ((position.x * 100.0) as i32, (position.z * 100.0) as i32))
            .collect();
        points.sort_unstable();
        let total = points.len();
        points.dedup();
        assert_eq!(points.len(), total, "two dummies share a position");
    }

    #[test]
    fn an_environment_scene_becomes_a_playable_arena() {
        let scene = engine_environment::arena::combat_arena();
        let simulation = environment_arena(&scene, 4);

        assert_eq!(simulation.world().len(), 3 + 4);
        assert_eq!(
            simulation.world().position(simulation.player().entity),
            Some(scene.spawns.player)
        );
        assert!(simulation.player().target.is_some());
        assert!(
            !simulation.collision_world().is_dirty(),
            "a scene must not hand over a world that still has to be rebuilt"
        );
    }

    #[test]
    fn only_the_objects_that_block_become_collision_shapes() {
        let scene = engine_environment::arena::combat_arena();
        let mut world = engine_collision::world::CollisionWorld::new();
        let added = add_environment_collision(&mut world, &scene);

        assert_eq!(added, scene.blocking_count());
        assert_eq!(world.len(), added);
        assert!(
            added < scene.len() / 2,
            "{added} of {} objects block, which is most of them",
            scene.len()
        );
        // Every shape is named after the asset that produced it, so a contact report says
        // what was hit.
        for id in world.statics() {
            assert!(!world.label(id).is_empty());
        }
    }

    #[test]
    fn a_yawed_placement_becomes_an_oriented_box_and_a_straight_one_does_not() {
        use engine_collision::shape::Shape;
        use engine_environment::object::{EnvironmentCollision, EnvironmentObject};

        let mut scene = engine_environment::EnvironmentScene::with_capacity("test", 2);
        let block = EnvironmentObject::decoration(
            engine_environment::asset::WALL_SEGMENT,
            Vec3::new(5.0, 0.0, 0.0),
        )
        .with_collision(EnvironmentCollision::standing(4.0, 1.2, 0.5));
        scene.push(block);
        scene.push(block.with_yaw(std::f32::consts::FRAC_PI_2));

        let mut world = engine_collision::world::CollisionWorld::new();
        assert_eq!(add_environment_collision(&mut world, &scene), 2);
        let shapes: Vec<_> = world
            .statics()
            .filter_map(|id| world.shape(id).copied())
            .collect();
        assert!(
            matches!(shapes[0], Shape::Aabb(_)),
            "an unrotated box should not pay for an oriented test: {:?}",
            shapes[0]
        );
        assert!(
            matches!(shapes[1], Shape::Obb(_)),
            "a yawed box has to be oriented or it blocks the wrong ground: {:?}",
            shapes[1]
        );
    }

    #[test]
    fn a_collision_box_with_no_volume_is_refused_rather_than_added() {
        use engine_environment::object::{EnvironmentCollision, EnvironmentObject};

        let mut scene = engine_environment::EnvironmentScene::with_capacity("test", 1);
        scene.push(
            EnvironmentObject::decoration(engine_environment::asset::PILLAR, Vec3::ZERO)
                .with_collision(EnvironmentCollision::Box {
                    center: Vec3::Y,
                    half_extents: Vec3::new(0.5, 0.0, 0.5),
                }),
        );
        let mut world = engine_collision::world::CollisionWorld::new();
        assert_eq!(add_environment_collision(&mut world, &scene), 0);
        assert!(world.is_empty());
    }

    #[test]
    fn the_player_spawns_somewhere_the_environment_left_room_for_it() {
        // The layout rule and the gameplay spawn are in two crates, and this is where they
        // meet: a spawn point inside a boulder is a character that pops sideways on its
        // first tick.
        use engine_collision::CollisionMask;
        use engine_collision::controller::CharacterConfig;
        use engine_collision::world::QueryBuffers;

        let scene = engine_environment::arena::combat_arena();
        let simulation = environment_arena(&scene, 16);
        let config = CharacterConfig::default();
        let mut buffers = QueryBuffers::new();

        let mut points = vec![scene.spawns.player, scene.spawns.featured_dummy];
        points.extend(
            simulation
                .world()
                .entities()
                .filter_map(|id| simulation.world().position(id)),
        );
        for point in points {
            assert!(
                !simulation.collision_world().capsule_overlaps(
                    &config.capsule_at(point + Vec3::new(0.0, 0.05, 0.0)),
                    CollisionMask::WORLD,
                    &mut buffers,
                ),
                "{point:?} is inside the level"
            );
        }
    }

    #[test]
    fn the_environments_kerb_stands_inside_the_wall_that_actually_confines_the_player() {
        // Two descriptions of the boundary. They have to agree, and the way they agree is
        // that the gameplay config takes its half-extent from the scene.
        let scene = engine_environment::arena::combat_arena();
        let config = ArenaConfig::from_environment(&scene, 4);
        assert_eq!(config.arena_half_extent, scene.half_extent);

        for object in &scene.objects {
            let Some(placed) = object.world_collision() else {
                continue;
            };
            // The box's eight corners, carried through its own rotation. `min_max` is the
            // unrotated case only, and half the kerb is turned a quarter turn — using it
            // here would measure a box that is not the box in the world.
            let half = placed.half_extents;
            let mut reach = 0.0_f32;
            for sx in [-1.0_f32, 1.0] {
                for sy in [-1.0_f32, 1.0] {
                    for sz in [-1.0_f32, 1.0] {
                        let corner = placed.center
                            + placed.rotation.rotate(Vec3::new(
                                sx * half.x,
                                sy * half.y,
                                sz * half.z,
                            ));
                        reach = reach.max(corner.x.abs()).max(corner.z.abs());
                    }
                }
            }
            assert!(
                reach <= config.arena_half_extent + 0.01,
                "{} reaches {reach:.2} m, past the wall at {}",
                object.asset,
                config.arena_half_extent
            );
        }
    }

    #[test]
    fn an_environment_arena_is_built_identically_every_time() {
        let scene = engine_environment::arena::combat_arena();
        let describe = || {
            let simulation = environment_arena(&scene, 8);
            let world = simulation.collision_world();
            let shapes: Vec<_> = world
                .statics()
                .map(|id| (world.label(id), world.bounds(id)))
                .collect();
            let positions: Vec<_> = simulation
                .world()
                .entities()
                .filter_map(|id| simulation.world().position(id))
                .collect();
            (shapes, positions)
        };
        assert_eq!(describe(), describe());
    }

    #[test]
    fn an_empty_group_is_still_a_valid_arena() {
        let config = ArenaConfig {
            group_count: 0,
            ..ArenaConfig::default()
        };
        let simulation = training_arena(&config);
        assert_eq!(dummies(simulation.world()).count(), 1);
        assert!(simulation.player().target.is_some(), "the lone dummy");
    }
}
