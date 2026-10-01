//! Finding a way around things: how a monster gets to somebody it cannot walk straight at.
//!
//! ```text
//!  CollisionWorld (walls, props, terrain)
//!        │  probed lazily, one ground cell at a time, and remembered
//!        ▼
//!  Navigator ── A* over the cells ──► corners of the way there ──► NavFollow per monster
//!                                           (string-pulled)            (steers one tick)
//! ```
//!
//! # The world is the map
//!
//! There is no baked navigation mesh to keep in step with the level. The collision world —
//! the same one bodies are moved through — is asked, per cell of a regular grid, "where is
//! the ground here, and does a character fit standing on it", and the answer is cached until
//! the geometry changes ([`Navigator::invalidate`], which the simulation calls whenever the
//! collision world is handed out for editing). A level loaded from the editor, a boss gate
//! coming down and a test that builds three walls all get correct paths without a build step.
//!
//! # 2.5D
//!
//! A cell is a column; its ground is found by a ray cast *down from the height the path
//! arrived at*, so a path over a bridge stays on the bridge and one under it stays under it.
//! Two neighbouring cells are connected when a character fits standing on both and the step
//! between them is one the controller can climb — a stair step, or a slope no steeper than it
//! can stand on. That is exactly the rule the character controller moves bodies by, so a path
//! found here is a path a body can walk.
//!
//! # Cost
//!
//! A search is bounded ([`NavConfig::max_expansions`]) and runs only when a monster's route is
//! stale; between searches a monster follows the corners it already has. A search that runs
//! out of budget, or whose goal cannot be reached at all, returns the way to the reachable
//! cell nearest the goal: the monster goes as close as it can rather than standing still.
//!
//! Deterministic: the heap breaks ties by insertion order, and the caches are only ever
//! looked up, never iterated.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use engine_collision::layer::CollisionMask;
use engine_collision::ray::Ray;
use engine_collision::shape::Capsule;
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::Vec3;

/// How a navigator measures the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavConfig {
    /// Edge of one grid cell, in metres. Half a metre: narrower than any doorway a monster
    /// is meant to fit through, wide enough that a room is a few hundred cells.
    pub cell: f32,
    /// The walking body's radius.
    pub radius: f32,
    /// The walking body's height.
    pub height: f32,
    /// The tallest step it climbs without jumping.
    pub max_step: f32,
    /// The steepest slope it can stand on, in radians.
    pub max_slope: f32,
    /// How many cells one search may expand before it settles for the nearest it reached.
    pub max_expansions: usize,
}

impl NavConfig {
    /// The navigation a body of `character`'s shape can walk.
    #[must_use]
    pub fn for_character(character: &engine_collision::controller::CharacterConfig) -> Self {
        Self {
            cell: 0.5,
            radius: character.radius,
            height: character.height,
            max_step: character.max_step_height,
            max_slope: character.max_slope_angle,
            max_expansions: 3000,
        }
    }
}

impl Default for NavConfig {
    fn default() -> Self {
        Self::for_character(&engine_collision::controller::CharacterConfig::default())
    }
}

/// How far above the height a column is asked from the downward ray starts.
const PROBE_UP: f32 = 1.4;

/// How far below that height it looks for ground before calling the column a drop.
const PROBE_DOWN: f32 = 6.0;

/// Heights are grouped into bands this tall for the cache: a column asked about from roughly
/// the same height gets the same answer.
const BAND: f32 = 1.0;

/// The cache is cleared rather than allowed past this many columns.
const CACHE_LIMIT: usize = 250_000;

/// One column of the world, as a walking body sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Column {
    /// The ground's height.
    ground: f32,
    /// Whether a body fits standing on it.
    walkable: bool,
}

/// A cell and the height band of its ground: one node of the search.
type NodeKey = (i32, i32, i32);

/// What the search knows about one node.
#[derive(Debug, Clone, Copy)]
struct NodeRecord {
    /// Cost from the start.
    cost: f32,
    /// Where it was reached from.
    parent: Option<NodeKey>,
    /// Its ground's height.
    ground: f32,
    /// Whether it has been expanded.
    closed: bool,
}

/// One entry in the open list, ordered cheapest first and then by insertion.
#[derive(Debug, Clone, Copy)]
struct Open {
    estimate: f32,
    order: u32,
    key: NodeKey,
}

impl PartialEq for Open {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Open {}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Open {
    /// Reversed, so the max-heap pops the cheapest; ties go to whichever was pushed first.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .estimate
            .total_cmp(&self.estimate)
            .then_with(|| other.order.cmp(&self.order))
    }
}

/// Finds ways through the collision world.
#[derive(Debug)]
pub struct Navigator {
    config: NavConfig,
    columns: HashMap<(i32, i32, i32), Option<Column>>,
    nodes: HashMap<NodeKey, NodeRecord>,
    open: BinaryHeap<Open>,
    buffers: QueryBuffers,
    searches: u64,
}

impl Default for Navigator {
    fn default() -> Self {
        Self::new(NavConfig::default())
    }
}

impl Navigator {
    /// A navigator for bodies measured by `config`, knowing nothing yet.
    #[must_use]
    pub fn new(config: NavConfig) -> Self {
        Self {
            config,
            columns: HashMap::new(),
            nodes: HashMap::new(),
            open: BinaryHeap::new(),
            buffers: QueryBuffers::with_capacity(16),
            searches: 0,
        }
    }

    /// How it measures the world.
    #[must_use]
    pub fn config(&self) -> &NavConfig {
        &self.config
    }

    /// Forgets everything it has learned about the world: the geometry changed.
    pub fn invalidate(&mut self) {
        self.columns.clear();
    }

    /// How many searches it has run, for a readout.
    #[must_use]
    pub fn searches(&self) -> u64 {
        self.searches
    }

    /// Which cell `value` falls in along one axis.
    fn cell_of(&self, value: f32) -> i32 {
        (value / self.config.cell).floor() as i32
    }

    /// The centre of cell `index` along one axis.
    fn centre_of(&self, index: i32) -> f32 {
        (index as f32 + 0.5) * self.config.cell
    }

    /// The ground at cell `(x, z)`, looked for from about `near` metres up.
    fn column(&mut self, collision: &CollisionWorld, x: i32, z: i32, near: f32) -> Option<Column> {
        let band = (near / BAND).floor() as i32;
        if let Some(known) = self.columns.get(&(x, z, band)) {
            return *known;
        }
        if self.columns.len() >= CACHE_LIMIT {
            self.columns.clear();
        }
        let answer = self.probe(collision, x, z, (band as f32 + 0.5) * BAND);
        self.columns.insert((x, z, band), answer);
        answer
    }

    /// Asks the world about one column: the ground, and whether a body fits on it.
    fn probe(&mut self, collision: &CollisionWorld, x: i32, z: i32, near: f32) -> Option<Column> {
        let top = Vec3::new(self.centre_of(x), near + PROBE_UP, self.centre_of(z));
        let ray = Ray::new(top, Vec3::new(0.0, -1.0, 0.0))?;
        let hit = collision.raycast(&ray, PROBE_UP + PROBE_DOWN, CollisionMask::WORLD)?;
        let ground = hit.hit.point.y;
        // Started inside something solid: the ray reports it at once, and nothing stands in
        // a wall.
        if hit.hit.distance <= 1e-4 {
            return Some(Column {
                ground,
                walkable: false,
            });
        }
        let steep = hit.hit.normal.y < self.config.max_slope.cos() - 1e-3;
        // The body, lifted by a step so the ground it stands on — and a slope it can climb —
        // do not count against it; what is left touching is a wall, a pillar or a ceiling.
        let lift = self.config.max_step + 0.02;
        let height = (self.config.height - lift).max(self.config.radius * 2.0);
        let capsule = Capsule::upright(
            Vec3::new(top.x, ground + lift, top.z),
            self.config.radius,
            height,
        );
        let blocked = collision.capsule_overlaps(&capsule, CollisionMask::WORLD, &mut self.buffers);
        Some(Column {
            ground,
            walkable: !steep && !blocked,
        })
    }

    /// The ground under `position`, looked for from about its own height, and whether a body
    /// fits standing on it.
    ///
    /// The same column a search reads, cache included, so a spot judged standable here is one
    /// a monster put there can also walk away from. `None` where there is no ground at all.
    pub fn ground_at(&mut self, collision: &CollisionWorld, position: Vec3) -> Option<(f32, bool)> {
        let x = self.cell_of(position.x);
        let z = self.cell_of(position.z);
        self.column(collision, x, z, position.y)
            .map(|column| (column.ground, column.walkable))
    }

    /// Whether a body can step from ground `from` to ground `to`, `distance` metres apart.
    fn climbable(&self, from: f32, to: f32, distance: f32) -> bool {
        (to - from).abs() <= self.config.max_step + distance * self.config.max_slope.tan()
    }

    /// Whether a body can walk the straight line from `from` to `to`.
    ///
    /// Sampled every half cell, each sample a column that has to be walkable and reachable
    /// from the one before: the same rule a search's edges follow, so a line judged walkable
    /// here is one the search would also have found.
    pub fn walkable_line(&mut self, collision: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
        let flat = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
        let length = flat.length();
        let spacing = self.config.cell * 0.5;
        let samples = (length / spacing).ceil().max(1.0) as usize;
        let mut ground = from.y;
        let mut previous: Option<(i32, i32)> = None;
        for index in 0..=samples {
            let along = index as f32 / samples as f32;
            let x = self.cell_of(from.x + flat.x * along);
            let z = self.cell_of(from.z + flat.z * along);
            if previous == Some((x, z)) {
                continue;
            }
            let Some(column) = self.column(collision, x, z, ground) else {
                return false;
            };
            if !column.walkable {
                // The body's own cell may graze a wall it is standing against; only the rest of
                // the line has to be clear.
                if index == 0 {
                    ground = column.ground;
                    previous = Some((x, z));
                    continue;
                }
                return false;
            }
            let step = if previous.is_some() {
                self.config.cell * std::f32::consts::SQRT_2
            } else {
                self.config.cell
            };
            if !self.climbable(ground, column.ground, step) {
                return false;
            }
            ground = column.ground;
            previous = Some((x, z));
        }
        true
    }

    /// The way from `from` to `to`, as the corners to walk through, `to` last if it is
    /// reachable; `into` is cleared first and its storage reused.
    ///
    /// Returns whether the goal itself was reached; `false` leaves the way to the nearest
    /// reachable cell instead, and an empty list when not even a first step could be found.
    pub fn find_path(
        &mut self,
        collision: &CollisionWorld,
        from: Vec3,
        to: Vec3,
        into: &mut Vec<Vec3>,
    ) -> bool {
        into.clear();
        self.searches += 1;
        if self.walkable_line(collision, from, to) {
            into.push(to);
            return true;
        }

        let start_x = self.cell_of(from.x);
        let start_z = self.cell_of(from.z);
        let goal_x = self.cell_of(to.x);
        let goal_z = self.cell_of(to.z);
        let Some(start) = self.column(collision, start_x, start_z, from.y) else {
            return false;
        };
        let start_key = (start_x, start_z, (start.ground / BAND).floor() as i32);

        self.nodes.clear();
        self.open.clear();
        self.nodes.insert(
            start_key,
            NodeRecord {
                cost: 0.0,
                parent: None,
                ground: start.ground,
                closed: false,
            },
        );
        let heuristic = |x: i32, z: i32| -> f32 {
            let dx = (x - goal_x).unsigned_abs() as f32;
            let dz = (z - goal_z).unsigned_abs() as f32;
            (dx.max(dz) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dz)) * 0.5
        };
        let mut order = 0_u32;
        self.open.push(Open {
            estimate: heuristic(start_x, start_z),
            order,
            key: start_key,
        });
        let mut best = (heuristic(start_x, start_z), start_key);
        let mut reached = None;
        let mut expansions = 0;

        const NEIGHBOURS: [(i32, i32); 8] = [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ];

        while let Some(Open { key, .. }) = self.open.pop() {
            let Some(record) = self.nodes.get(&key).copied() else {
                continue;
            };
            if record.closed {
                continue;
            }
            if let Some(node) = self.nodes.get_mut(&key) {
                node.closed = true;
            }
            let (x, z, _) = key;
            let remaining = heuristic(x, z);
            if remaining < best.0 {
                best = (remaining, key);
            }
            if (x - goal_x).abs() <= 1 && (z - goal_z).abs() <= 1 {
                reached = Some(key);
                break;
            }
            expansions += 1;
            if expansions > self.config.max_expansions {
                break;
            }
            for (dx, dz) in NEIGHBOURS {
                let (nx, nz) = (x + dx, z + dz);
                let diagonal = dx != 0 && dz != 0;
                let step = if diagonal {
                    self.config.cell * std::f32::consts::SQRT_2
                } else {
                    self.config.cell
                };
                let Some(column) = self.column(collision, nx, nz, record.ground) else {
                    continue;
                };
                if !column.walkable || !self.climbable(record.ground, column.ground, step) {
                    continue;
                }
                // No cutting a corner: both cells a diagonal passes between must be open too.
                if diagonal {
                    let side_a = self.column(collision, x + dx, z, record.ground);
                    let side_b = self.column(collision, x, z + dz, record.ground);
                    if !side_a.is_some_and(|side| side.walkable)
                        || !side_b.is_some_and(|side| side.walkable)
                    {
                        continue;
                    }
                }
                let next = (nx, nz, (column.ground / BAND).floor() as i32);
                let cost = record.cost + step + (column.ground - record.ground).abs() * 0.5;
                let known = self.nodes.get(&next).copied();
                if known.is_some_and(|known| known.closed || known.cost <= cost) {
                    continue;
                }
                self.nodes.insert(
                    next,
                    NodeRecord {
                        cost,
                        parent: Some(key),
                        ground: column.ground,
                        closed: false,
                    },
                );
                order = order.wrapping_add(1);
                self.open.push(Open {
                    estimate: cost + heuristic(nx, nz) * 2.0 * self.config.cell,
                    order,
                    key: next,
                });
            }
        }

        let (end, arrived) = match reached {
            Some(key) => (key, true),
            None => (best.1, false),
        };
        // Walk back from the end to the start.
        let mut cursor = Some(end);
        while let Some(key) = cursor {
            let Some(record) = self.nodes.get(&key) else {
                break;
            };
            into.push(Vec3::new(
                self.centre_of(key.0),
                record.ground,
                self.centre_of(key.1),
            ));
            cursor = record.parent;
        }
        into.reverse();
        // The first cell is where the body already stands.
        if !into.is_empty() {
            into.remove(0);
        }
        if arrived {
            into.push(to);
        }
        self.smooth(collision, from, into);
        arrived
    }

    /// Pulls the string: drops every corner the body can see past to a later one.
    fn smooth(&mut self, collision: &CollisionWorld, from: Vec3, path: &mut Vec<Vec3>) {
        if path.len() < 2 {
            return;
        }
        let mut kept = 0;
        let mut anchor = from;
        let mut index = 0;
        while index < path.len() {
            // The furthest corner visible from the anchor.
            let mut furthest = index;
            for candidate in (index + 1..path.len()).rev() {
                if self.walkable_line(collision, anchor, path[candidate]) {
                    furthest = candidate;
                    break;
                }
            }
            path[kept] = path[furthest];
            kept += 1;
            anchor = path[furthest];
            index = furthest + 1;
        }
        path.truncate(kept);
    }
}

/// One walker's way somewhere: the corners left to walk through, and when to look again.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NavFollow {
    /// The corners, nearest first; the last is the goal when it could be reached.
    corners: Vec<Vec3>,
    /// Which corner is next.
    next: usize,
    /// Where the way was found to.
    goal: Option<Vec3>,
    /// Seconds before it is looked for again.
    stale_in: f32,
    /// Whether the way reaches the goal, or only as close as could be got.
    reaches: bool,
}

/// Seconds a way is followed before it is looked for again, even if nothing moved.
const REPATH_SECONDS: f32 = 0.6;

/// How far the goal may move before the way to it is looked for again at once, in metres.
const REPATH_DISTANCE: f32 = 1.5;

/// How close a corner counts as reached, in metres.
const ARRIVE_DISTANCE: f32 = 0.45;

impl NavFollow {
    /// Forgets the way: the walker has stopped going anywhere.
    pub fn clear(&mut self) {
        self.corners.clear();
        self.next = 0;
        self.goal = None;
        self.stale_in = 0.0;
        self.reaches = false;
    }

    /// Whether the way needs looking for again: it is old, it has run out, or the goal left.
    #[must_use]
    pub fn is_stale(&self, goal: Vec3) -> bool {
        self.stale_in <= 0.0
            // Out of corners on a way that reached the goal: the goal moved on. One that could
            // only get close waits for its clock instead of searching again every tick.
            || (self.next >= self.corners.len() && self.reaches)
            || self.goal.is_none_or(|known| {
                crate::world::horizontal_distance(known, goal) > REPATH_DISTANCE
            })
    }

    /// Looks for the way from `position` to `goal` again.
    pub fn repath(
        &mut self,
        navigator: &mut Navigator,
        collision: &CollisionWorld,
        position: Vec3,
        goal: Vec3,
        stagger: f32,
    ) {
        self.reaches = navigator.find_path(collision, position, goal, &mut self.corners);
        self.next = 0;
        self.goal = Some(goal);
        self.stale_in = REPATH_SECONDS + stagger;
    }

    /// The direction to walk this tick from `position`, and whether the walker is going round
    /// something (more than one corner left) rather than straight at the goal.
    ///
    /// `None` when there is no way at all — the caller keeps whatever it would have done. A
    /// zero direction when the walker has got as close as it can to a goal it cannot reach:
    /// it waits there, rather than walking into the wall between them.
    pub fn steer(&mut self, position: Vec3, step: f32) -> Option<(Vec3, bool)> {
        self.stale_in -= step;
        while self.next < self.corners.len()
            && crate::world::horizontal_distance(position, self.corners[self.next])
                <= ARRIVE_DISTANCE
        {
            self.next += 1;
        }
        if self.next >= self.corners.len() && !self.reaches && self.goal.is_some() {
            return Some((Vec3::ZERO, false));
        }
        let corner = *self.corners.get(self.next)?;
        let offset = Vec3::new(corner.x - position.x, 0.0, corner.z - position.z);
        let length = offset.length();
        if length <= f32::EPSILON {
            return None;
        }
        Some((offset * (1.0 / length), self.corners.len() - self.next > 1))
    }

    /// The corners left, nearest first. For a debug view.
    #[must_use]
    pub fn corners(&self) -> &[Vec3] {
        self.corners.get(self.next..).unwrap_or(&[])
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// A floor, and a wall across the middle with one gap at its far end.
    ///
    /// ```text
    ///   z=+6 ┌─────────────────────────┐
    ///        │          goal           │
    ///   z=0  ███████████████████   ····│  wall from x=-10 to x=6, gap beyond
    ///        │          start          │
    ///   z=-6 └─────────────────────────┘
    /// ```
    fn walled() -> CollisionWorld {
        let mut world = CollisionWorld::new();
        world.add_ground(0.0, "FLOOR");
        world.add_box(
            Vec3::new(-10.0, 0.0, -0.3),
            Vec3::new(6.0, 3.0, 0.3),
            "WALL",
        );
        world.rebuild_broad_phase();
        world
    }

    fn walk(path: &[Vec3], from: Vec3) -> Vec<Vec3> {
        std::iter::once(from).chain(path.iter().copied()).collect()
    }

    #[test]
    fn an_open_floor_is_walked_straight() {
        let mut world = CollisionWorld::new();
        world.add_ground(0.0, "FLOOR");
        world.rebuild_broad_phase();
        let mut navigator = Navigator::default();
        let mut path = Vec::new();
        let goal = Vec3::new(8.0, 0.0, 5.0);
        assert!(navigator.find_path(&world, Vec3::ZERO, goal, &mut path));
        assert_eq!(path, vec![goal]);
    }

    #[test]
    fn a_wall_is_walked_round_through_its_gap() {
        let world = walled();
        let mut navigator = Navigator::default();
        let mut path = Vec::new();
        let from = Vec3::new(0.0, 0.0, -4.0);
        let goal = Vec3::new(0.0, 0.0, 4.0);
        assert!(navigator.find_path(&world, from, goal, &mut path));
        assert!(
            path.len() >= 2,
            "it went straight through the wall: {path:?}"
        );
        assert_eq!(path.last().copied(), Some(goal));
        // Every leg of the way is walkable, and the way passes the end of the wall.
        let points = walk(&path, from);
        for leg in points.windows(2) {
            assert!(navigator.walkable_line(&world, leg[0], leg[1]), "{leg:?}");
        }
        assert!(path.iter().any(|corner| corner.x > 6.0), "{path:?}");
        // String-pulled: a handful of corners, not a staircase of cells.
        assert!(path.len() <= 4, "{path:?}");
    }

    #[test]
    fn a_closed_room_leads_as_close_as_it_can_get() {
        let mut world = walled();
        // Close the gap.
        world.add_box(Vec3::new(5.0, 0.0, -0.3), Vec3::new(30.0, 3.0, 0.3), "PLUG");
        world.add_box(
            Vec3::new(-30.0, 0.0, -0.3),
            Vec3::new(-9.0, 3.0, 0.3),
            "PLUG",
        );
        world.rebuild_broad_phase();
        let mut navigator = Navigator::new(NavConfig {
            max_expansions: 800,
            ..NavConfig::default()
        });
        let mut path = Vec::new();
        let from = Vec3::new(0.0, 0.0, -4.0);
        let goal = Vec3::new(0.0, 0.0, 4.0);
        assert!(!navigator.find_path(&world, from, goal, &mut path));
        let end = path.last().copied().expect("a way towards it");
        assert!(
            end.z < 0.0 && end.z > -1.5,
            "it should end against the wall: {end:?}"
        );
    }

    #[test]
    fn a_step_is_climbed_and_a_cliff_is_not() {
        let mut world = CollisionWorld::new();
        world.add_ground(0.0, "FLOOR");
        // A low step to the east, a tall ledge to the west.
        world.add_box(Vec3::new(2.0, 0.0, -5.0), Vec3::new(6.0, 0.3, 5.0), "STEP");
        world.add_box(
            Vec3::new(-6.0, 0.0, -5.0),
            Vec3::new(-2.0, 2.0, 5.0),
            "LEDGE",
        );
        world.rebuild_broad_phase();
        let mut navigator = Navigator::default();
        assert!(navigator.walkable_line(&world, Vec3::ZERO, Vec3::new(4.0, 0.3, 0.0)));
        assert!(!navigator.walkable_line(&world, Vec3::ZERO, Vec3::new(-4.0, 2.0, 0.0)));
    }

    #[test]
    fn terrain_is_walked_over_and_a_steep_face_is_gone_round() {
        use engine_collision::heightfield::Heightfield;
        use engine_collision::shape::Shape;
        use engine_collision::world::StaticDesc;
        // 21 points on a 1 m grid: a gentle rise everywhere, and a sheer 6 m block of
        // terrain in the middle columns 8..12 that stops short of the north edge.
        let resolution = 21;
        let mut heights = vec![0.0; resolution * resolution];
        for z in 0..resolution {
            for x in 0..resolution {
                let mut height = x as f32 * 0.1;
                if (8..=12).contains(&x) && z < 16 {
                    height = 6.0;
                }
                heights[z * resolution + x] = height;
            }
        }
        let field = Heightfield::new(Vec3::ZERO, 1.0, resolution, heights).expect("a field");
        let mut world = CollisionWorld::new();
        world.add_static(StaticDesc::new(Shape::heightfield(field)));
        world.rebuild_broad_phase();
        let mut navigator = Navigator::default();
        let mut path = Vec::new();
        let from = Vec3::new(3.0, 0.3, 5.0);
        let goal = Vec3::new(17.0, 1.7, 5.0);
        assert!(
            navigator.find_path(&world, from, goal, &mut path),
            "{path:?}"
        );
        assert!(
            path.iter().any(|corner| corner.z > 15.0),
            "it should go round the north end of the block: {path:?}"
        );
        assert!(
            path.iter().all(|corner| corner.y < 3.0),
            "it climbed the block: {path:?}"
        );
    }

    #[test]
    fn the_same_question_gets_the_same_answer() {
        let world = walled();
        let from = Vec3::new(-3.0, 0.0, -4.0);
        let goal = Vec3::new(2.0, 0.0, 5.0);
        let mut first = Vec::new();
        let mut second = Vec::new();
        Navigator::default().find_path(&world, from, goal, &mut first);
        let mut warm = Navigator::default();
        warm.find_path(&world, goal, from, &mut second);
        warm.find_path(&world, from, goal, &mut second);
        assert_eq!(first, second, "a warm cache changed the answer");
    }

    #[test]
    fn a_follower_steers_corner_to_corner_and_looks_again_when_the_goal_moves() {
        let world = walled();
        let mut navigator = Navigator::default();
        let mut follow = NavFollow::default();
        let from = Vec3::new(0.0, 0.0, -4.0);
        let goal = Vec3::new(0.0, 0.0, 4.0);
        assert!(follow.is_stale(goal));
        follow.repath(&mut navigator, &world, from, goal, 0.0);
        assert!(!follow.is_stale(goal));
        let (direction, rounding) = follow.steer(from, 1.0 / 60.0).expect("a way");
        assert!(rounding, "the first corner is not the goal");
        assert!(
            direction.x > 0.5,
            "it heads for the gap first: {direction:?}"
        );
        assert!(follow.is_stale(goal + Vec3::new(3.0, 0.0, 0.0)));
    }
}
