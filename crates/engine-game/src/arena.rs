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
            arena_half_extent: 40.0,
        }
    }
}

impl ArenaConfig {
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

/// Fills an existing world with the arena and returns what it spawned.
///
/// Separate from [`training_arena`] so a benchmark can rebuild the world of a simulation it
/// already owns, and so a test can build a world with no simulation at all.
pub fn populate(world: &mut GameWorld, config: &ArenaConfig) -> Arena {
    world.clear();

    let player = world.spawn(EntityDesc::player(config.player_spawn));
    let single_dummy = world.spawn(EntityDesc::dummy(config.single_dummy, config.dummy_health));
    let mut group = Vec::with_capacity(config.group_count);
    spawn_group(world, config, &mut group);

    Arena {
        player,
        single_dummy,
        group,
    }
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

        assert_eq!(world.len(), 2 + config.group_count);
        assert_eq!(dummies(world).count(), 1 + config.group_count);
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
        let radius = crate::firemage::FireAbility::FlameWave.definition().range;

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
