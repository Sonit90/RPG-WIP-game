//! The broad phase: which static objects are worth an exact test.
//!
//! ```text
//! query box  ->  broad phase  ->  candidate indices  ->  narrow phase
//!                ^^^^^^^^^^^                             (crate::narrow)
//! ```
//!
//! The separation is the point. A character controller asks the world to move a capsule and
//! never learns how the world found the geometry in its way, so replacing the scan with a
//! grid — or later with a BVH — changes nothing above this line.
//!
//! ## Why two implementations and not one
//!
//! For the game as it stands, a linear scan *is* the right structure: the combat arena has
//! five static objects and a scan over five boxes is shorter than the code that would decide
//! which cell to look in. But "scales to 10,000 without a rewrite" has to be demonstrated
//! rather than asserted, so the alternative exists and the benchmark runs both.
//!
//! Dispatch is a `match` on an enum, not a trait object: this is the inner loop of character
//! movement, and a v-table call per query would cost more than the query.
//!
//! ## Why the grid is two-dimensional
//!
//! Level geometry is a floor plan. Walls are tall and thin, floors are wide and flat, and
//! almost nothing is stacked deeply in Y — so a third axis would multiply the cell count
//! while rejecting almost nothing that X and Z had not already rejected.

use engine_core::math::{Aabb, Vec3};

use crate::layer::{CollisionLayer, CollisionMask};
use crate::shape::aabb_overlaps;

/// One static object as the broad phase sees it: a box and a layer.
///
/// Deliberately not the shape. The broad phase never needs to know whether something is a
/// ramp, and keeping the shapes out of its array keeps the scan reading 28 bytes per object
/// instead of the whole shape union.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Proxy {
    /// The object's world-space bounds.
    ///
    /// [`Aabb::EMPTY`] marks something without finite bounds — a plane — which is always a
    /// candidate, because a plane is everywhere.
    pub bounds: Aabb,
    /// What the object is.
    pub layer: CollisionLayer,
    /// Whether the slot holds a live object at all.
    pub live: bool,
}

impl Proxy {
    /// Whether this proxy should be offered for a query with `mask` over `bounds`.
    #[must_use]
    pub fn matches(&self, query: &Aabb, mask: CollisionMask) -> bool {
        self.live
            && mask.contains(self.layer)
            && (self.bounds.is_empty() || aabb_overlaps(&self.bounds, query))
    }
}

/// How candidate objects are found.
#[derive(Debug, Clone, Default)]
pub enum BroadPhase {
    /// Test every proxy. Correct at any size, fastest below a few dozen objects.
    #[default]
    Linear,
    /// Bin proxies into a uniform grid over X and Z.
    Grid(UniformGrid),
}

/// Static objects below which a linear scan beats any structure.
///
/// Measured rather than guessed — see the benchmark table in `collision.md`. Below this,
/// binning costs more than the tests it avoids.
pub const GRID_THRESHOLD: usize = 64;

impl BroadPhase {
    /// The structure appropriate for `proxies`, built.
    ///
    /// A single decision point, so no caller has to know the threshold.
    #[must_use]
    pub fn build(proxies: &[Proxy]) -> Self {
        let live = proxies.iter().filter(|proxy| proxy.live).count();
        if live < GRID_THRESHOLD {
            Self::Linear
        } else {
            Self::Grid(UniformGrid::build(proxies))
        }
    }

    /// Short name, for a debug readout.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Linear => "LINEAR",
            Self::Grid(_) => "GRID",
        }
    }

    /// Appends the indices of every proxy that might overlap `bounds` to `out`.
    ///
    /// `out` is supplied by the caller and reused across queries — this is the allocation
    /// the character controller must not make sixty times a second.
    ///
    /// Returns nothing: the caller reads `out.len()` for the candidate count, because the
    /// count *is* the length and returning it separately invites the two to disagree.
    pub fn query(&self, proxies: &[Proxy], bounds: &Aabb, mask: CollisionMask, out: &mut Vec<u32>) {
        match self {
            Self::Linear => {
                for (index, proxy) in proxies.iter().enumerate() {
                    if proxy.matches(bounds, mask) {
                        out.push(index as u32);
                    }
                }
            }
            Self::Grid(grid) => grid.query(proxies, bounds, mask, out),
        }
    }
}

/// A uniform grid over the X and Z axes, stored as one flat array with per-cell offsets.
///
/// Compressed-sparse-row rather than a `Vec` per cell: 10,000 objects over a 64x64 grid is
/// two allocations here and 4,096 there, and a query walks a contiguous slice instead of
/// chasing a pointer per cell.
#[derive(Debug, Clone, Default)]
pub struct UniformGrid {
    /// World position of the grid's `(0, 0)` corner.
    origin: Vec3,
    /// Width of one cell, in metres.
    cell_size: f32,
    /// Cells along X.
    columns: u32,
    /// Cells along Z.
    rows: u32,
    /// Where each cell's slice of `items` starts. Length is `columns * rows + 1`.
    cell_start: Vec<u32>,
    /// Proxy indices, grouped by cell.
    items: Vec<u32>,
    /// Proxies that are in every query: planes, and anything spanning most of the grid.
    unbounded: Vec<u32>,
}

/// Cells the grid aims for along its longer axis.
///
/// A target rather than a fixed size, so a level a kilometre across and one ten metres
/// across both end up with cells of a useful size relative to what is in them.
const TARGET_CELLS_PER_AXIS: f32 = 48.0;

/// Cells a single object may occupy before it goes in the always-candidate list.
///
/// An object spanning half the level would otherwise be written into thousands of cells,
/// and every one of those writes is a query that will find it again.
const MAX_CELLS_PER_PROXY: u32 = 64;

impl UniformGrid {
    /// Bins every live proxy.
    #[must_use]
    pub fn build(proxies: &[Proxy]) -> Self {
        let mut extent = Aabb::EMPTY;
        for proxy in proxies.iter().filter(|proxy| proxy.live) {
            if !proxy.bounds.is_empty() {
                extent.insert(proxy.bounds.min);
                extent.insert(proxy.bounds.max);
            }
        }

        if extent.is_empty() {
            // Nothing with finite bounds: every live proxy is unbounded, and the grid is a
            // list. Still correct, and it costs one branch in `query`.
            return Self {
                unbounded: live_indices(proxies),
                ..Self::default()
            };
        }

        let size = extent.max - extent.min;
        let span = size.x.max(size.z).max(1e-3);
        let cell_size = (span / TARGET_CELLS_PER_AXIS).max(0.5);
        let columns = ((size.x / cell_size).ceil() as u32 + 1).max(1);
        let rows = ((size.z / cell_size).ceil() as u32 + 1).max(1);

        let mut grid = Self {
            origin: extent.min,
            cell_size,
            columns,
            rows,
            cell_start: vec![0; (columns as usize * rows as usize) + 1],
            items: Vec::new(),
            unbounded: Vec::new(),
        };

        // Counting pass, then a prefix sum, then a placement pass: the standard way to fill
        // a CSR without a per-cell vector.
        let mut counts = vec![0_u32; columns as usize * rows as usize];
        for (index, proxy) in proxies.iter().enumerate() {
            if !proxy.live {
                continue;
            }
            match grid.cell_span(&proxy.bounds) {
                Some(span) => {
                    for cell in span.cells(columns) {
                        counts[cell as usize] += 1;
                    }
                }
                None => grid.unbounded.push(index as u32),
            }
        }

        let mut total = 0;
        for (cell, count) in counts.iter().enumerate() {
            grid.cell_start[cell] = total;
            total += count;
        }
        grid.cell_start[counts.len()] = total;

        grid.items = vec![u32::MAX; total as usize];
        let mut cursor = grid.cell_start.clone();
        for (index, proxy) in proxies.iter().enumerate() {
            if !proxy.live {
                continue;
            }
            if let Some(span) = grid.cell_span(&proxy.bounds) {
                for cell in span.cells(columns) {
                    grid.items[cursor[cell as usize] as usize] = index as u32;
                    cursor[cell as usize] += 1;
                }
            }
        }

        grid
    }

    /// Cells along X.
    #[must_use]
    pub fn columns(&self) -> u32 {
        self.columns
    }

    /// Cells along Z.
    #[must_use]
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Width of one cell, in metres.
    #[must_use]
    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }

    /// Proxies that are candidates for every query.
    #[must_use]
    pub fn unbounded(&self) -> &[u32] {
        &self.unbounded
    }

    /// The rectangle of cells a box covers, or `None` if it belongs in `unbounded`.
    fn cell_span(&self, bounds: &Aabb) -> Option<CellSpan> {
        if bounds.is_empty() || self.columns == 0 || self.rows == 0 {
            return None;
        }
        let min_x = self.axis(bounds.min.x - self.origin.x, self.columns);
        let max_x = self.axis(bounds.max.x - self.origin.x, self.columns);
        let min_z = self.axis(bounds.min.z - self.origin.z, self.rows);
        let max_z = self.axis(bounds.max.z - self.origin.z, self.rows);

        let span = CellSpan {
            min_x,
            max_x,
            min_z,
            max_z,
        };
        if span.count() > MAX_CELLS_PER_PROXY {
            return None;
        }
        Some(span)
    }

    /// Which cell a distance along an axis falls in, clamped to the grid.
    fn axis(&self, distance: f32, count: u32) -> u32 {
        if !distance.is_finite() {
            return 0;
        }
        let cell = (distance / self.cell_size).floor();
        if cell <= 0.0 {
            0
        } else {
            (cell as u32).min(count - 1)
        }
    }

    /// Appends candidate indices for `bounds` to `out`.
    fn query(&self, proxies: &[Proxy], bounds: &Aabb, mask: CollisionMask, out: &mut Vec<u32>) {
        let first = out.len();

        for &index in &self.unbounded {
            if proxies[index as usize].matches(bounds, mask) {
                out.push(index);
            }
        }

        if let Some(span) = self.cell_span(bounds) {
            for cell in span.cells(self.columns) {
                let start = self.cell_start[cell as usize] as usize;
                let end = self.cell_start[cell as usize + 1] as usize;
                for &index in &self.items[start..end] {
                    if proxies[index as usize].matches(bounds, mask) {
                        out.push(index);
                    }
                }
            }
        } else {
            // A query box big enough to be "unbounded" is rare — it happens only for a
            // deliberately huge sweep — and answering it with a scan is still correct.
            for (index, proxy) in proxies.iter().enumerate() {
                if proxy.matches(bounds, mask) {
                    out.push(index as u32);
                }
            }
        }

        // An object straddling a cell boundary is in both cells and would otherwise be
        // tested — and reported as a contact — twice. Sorting a handful of indices is
        // cheaper than the duplicate narrow-phase tests it prevents, and it makes the
        // candidate order independent of the grid, which is what keeps movement
        // deterministic across a rebuild.
        let candidates = &mut out[first..];
        candidates.sort_unstable();
        let unique = dedup_len(candidates);
        out.truncate(first + unique);
    }
}

/// Removes adjacent duplicates in place and returns the new length.
///
/// `Vec::dedup` on a slice is not available, and copying the slice out to use it would be an
/// allocation in the one path that must not have any.
fn dedup_len(values: &mut [u32]) -> usize {
    if values.is_empty() {
        return 0;
    }
    let mut write = 1;
    for read in 1..values.len() {
        if values[read] != values[write - 1] {
            values[write] = values[read];
            write += 1;
        }
    }
    write
}

/// Every live proxy's index.
fn live_indices(proxies: &[Proxy]) -> Vec<u32> {
    proxies
        .iter()
        .enumerate()
        .filter(|(_, proxy)| proxy.live)
        .map(|(index, _)| index as u32)
        .collect()
}

/// A rectangle of cells, inclusive at both ends.
#[derive(Debug, Clone, Copy)]
struct CellSpan {
    min_x: u32,
    max_x: u32,
    min_z: u32,
    max_z: u32,
}

impl CellSpan {
    /// How many cells the span covers.
    fn count(&self) -> u32 {
        (self.max_x - self.min_x + 1) * (self.max_z - self.min_z + 1)
    }

    /// The flat index of every cell in the span.
    fn cells(self, columns: u32) -> impl Iterator<Item = u32> {
        (self.min_z..=self.max_z)
            .flat_map(move |z| (self.min_x..=self.max_x).map(move |x| z * columns + x))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn box_proxy(x: f32, z: f32) -> Proxy {
        Proxy {
            bounds: Aabb::new(
                Vec3::new(x - 0.5, 0.0, z - 0.5),
                Vec3::new(x + 0.5, 2.0, z + 0.5),
            ),
            layer: CollisionLayer::WORLD,
            live: true,
        }
    }

    fn plane_proxy() -> Proxy {
        Proxy {
            bounds: Aabb::EMPTY,
            layer: CollisionLayer::WORLD,
            live: true,
        }
    }

    /// A grid of `count` boxes on a lattice, plus one plane.
    fn lattice(count: usize) -> Vec<Proxy> {
        let mut proxies = vec![plane_proxy()];
        let columns = (count as f32).sqrt().ceil() as usize;
        for index in 0..count {
            let x = (index % columns) as f32 * 4.0;
            let z = (index / columns) as f32 * 4.0;
            proxies.push(box_proxy(x, z));
        }
        proxies
    }

    fn query(broad: &BroadPhase, proxies: &[Proxy], bounds: Aabb) -> Vec<u32> {
        let mut out = Vec::new();
        broad.query(proxies, &bounds, CollisionMask::ALL, &mut out);
        out.sort_unstable();
        out
    }

    #[test]
    fn a_linear_scan_offers_everything_that_overlaps() {
        let proxies = lattice(9);
        let found = query(
            &BroadPhase::Linear,
            &proxies,
            Aabb::new(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
        );
        // The plane, which is everywhere, and the box at the origin.
        assert_eq!(found, vec![0, 1]);
    }

    #[test]
    fn a_query_that_touches_nothing_still_finds_the_plane() {
        let proxies = lattice(9);
        let found = query(
            &BroadPhase::Linear,
            &proxies,
            Aabb::new(Vec3::splat(500.0), Vec3::splat(501.0)),
        );
        assert_eq!(found, vec![0], "a plane is a candidate everywhere");
    }

    #[test]
    fn the_mask_filters_before_the_bounds_test() {
        let mut proxies = lattice(4);
        proxies[1].layer = CollisionLayer::DUMMY;
        let bounds = Aabb::new(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0));

        let mut world_only = Vec::new();
        BroadPhase::Linear.query(&proxies, &bounds, CollisionMask::WORLD, &mut world_only);
        assert_eq!(
            world_only,
            vec![0],
            "asking for world geometry offered the dummy"
        );

        let mut dummies_only = Vec::new();
        BroadPhase::Linear.query(
            &proxies,
            &bounds,
            CollisionMask::NONE.with(CollisionLayer::DUMMY),
            &mut dummies_only,
        );
        assert_eq!(dummies_only, vec![1], "the plane is not on the dummy layer");
    }

    #[test]
    fn a_dead_proxy_is_never_a_candidate() {
        let mut proxies = lattice(4);
        proxies[1].live = false;
        let found = query(
            &BroadPhase::Linear,
            &proxies,
            Aabb::new(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 1.0, 1.0)),
        );
        assert_eq!(found, vec![0]);
    }

    #[test]
    fn the_grid_and_the_scan_agree_on_every_query() {
        // The property that makes the structure swappable: identical answers, whatever is
        // underneath. A disagreement here is a character that collides differently in a big
        // level than in a small one.
        let proxies = lattice(400);
        let grid = BroadPhase::Grid(UniformGrid::build(&proxies));

        for step in 0..40 {
            let x = step as f32 * 2.0 - 8.0;
            let z = step as f32 * 1.5 - 4.0;
            let bounds = Aabb::new(
                Vec3::new(x - 0.6, 0.0, z - 0.6),
                Vec3::new(x + 0.6, 2.0, z + 0.6),
            );
            assert_eq!(
                query(&grid, &proxies, bounds),
                query(&BroadPhase::Linear, &proxies, bounds),
                "the two broad phases disagreed at {x}, {z}"
            );
        }
    }

    #[test]
    fn the_grid_rejects_the_overwhelming_majority_of_a_large_level() {
        let proxies = lattice(10_000);
        let grid = BroadPhase::Grid(UniformGrid::build(&proxies));
        let found = query(
            &grid,
            &proxies,
            Aabb::new(Vec3::new(-0.6, 0.0, -0.6), Vec3::new(0.6, 2.0, 0.6)),
        );
        assert!(
            found.len() < 20,
            "the grid offered {} candidates out of 10,001",
            found.len()
        );
    }

    #[test]
    fn an_object_straddling_a_boundary_is_offered_once() {
        // It lives in several cells; a query covering those cells must not report it more
        // than once, or the narrow phase resolves the same contact twice and the character
        // is pushed out twice as far.
        let mut proxies = lattice(200);
        proxies.push(Proxy {
            bounds: Aabb::new(Vec3::new(-2.0, 0.0, -2.0), Vec3::new(30.0, 2.0, 2.0)),
            layer: CollisionLayer::WORLD,
            live: true,
        });
        let grid = BroadPhase::Grid(UniformGrid::build(&proxies));
        let found = query(
            &grid,
            &proxies,
            Aabb::new(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(25.0, 2.0, 1.0)),
        );

        let mut unique = found.clone();
        unique.dedup();
        assert_eq!(unique.len(), found.len(), "a candidate was offered twice");
    }

    #[test]
    fn the_structure_is_chosen_by_size() {
        assert!(matches!(BroadPhase::build(&lattice(4)), BroadPhase::Linear));
        assert!(matches!(
            BroadPhase::build(&lattice(1_000)),
            BroadPhase::Grid(_)
        ));
        assert_eq!(BroadPhase::Linear.name(), "LINEAR");
    }

    #[test]
    fn a_world_of_nothing_but_planes_still_builds_and_answers() {
        let proxies = vec![plane_proxy(), plane_proxy(), plane_proxy()];
        let grid = UniformGrid::build(&proxies);
        assert_eq!(grid.unbounded().len(), 3);
        let found = query(
            &BroadPhase::Grid(grid),
            &proxies,
            Aabb::new(Vec3::ZERO, Vec3::ONE),
        );
        assert_eq!(found, vec![0, 1, 2]);
    }

    #[test]
    fn an_empty_world_answers_with_nothing_rather_than_panicking() {
        let proxies: Vec<Proxy> = Vec::new();
        let grid = BroadPhase::build(&proxies);
        assert!(query(&grid, &proxies, Aabb::new(Vec3::ZERO, Vec3::ONE)).is_empty());
    }

    #[test]
    fn deduplication_keeps_one_of_each_in_order() {
        let mut values = [1, 1, 2, 3, 3, 3, 9];
        let length = dedup_len(&mut values);
        assert_eq!(&values[..length], &[1, 2, 3, 9]);
        assert_eq!(dedup_len(&mut []), 0);
    }
}
