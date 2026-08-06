//! Stress-test geometry.
//!
//! Generates N triangles arranged in a grid that fills the clip volume, so a stress level
//! covers the same screen area regardless of triangle count. Only the triangle *size*
//! changes: 1 triangle is large, 100,000 triangles are a few pixels each. That keeps the
//! comparison about geometry throughput rather than about how much of the screen is lit,
//! which would otherwise confound fill rate with triangle count.
//!
//! Generation is pure and deterministic, so the same level always produces the same
//! scene and the numbers between runs are comparable.

use crate::pipeline::Vertex;

/// Triangle counts the stress test steps through.
pub const STRESS_LEVELS: [u32; 5] = [1, 100, 1_000, 10_000, 100_000];

/// Fraction of its grid cell a triangle occupies.
///
/// Below 1.0 so neighbouring triangles do not touch, which keeps each one a separate
/// primitive rather than a seam-free surface.
const CELL_FILL: f32 = 0.85;

/// Returns the stress level at `index`, saturating at the ends.
#[must_use]
pub fn level_at(index: usize) -> u32 {
    STRESS_LEVELS[index.min(STRESS_LEVELS.len() - 1)]
}

/// Returns the index of the level nearest to `count`, for reporting a custom count.
#[must_use]
pub fn nearest_level_index(count: u32) -> usize {
    STRESS_LEVELS
        .iter()
        .enumerate()
        .min_by_key(|(_, level)| level.abs_diff(count))
        .map_or(0, |(index, _)| index)
}

/// Grid dimensions used for `count` triangles: as square as possible.
#[must_use]
pub fn grid_dimensions(count: u32) -> (u32, u32) {
    if count == 0 {
        return (0, 0);
    }
    let columns = (count as f64).sqrt().ceil() as u32;
    let rows = count.div_ceil(columns);
    (columns.max(1), rows.max(1))
}

/// Writes `count` triangles into `vertices`, replacing its contents.
///
/// Reuses the buffer's allocation, so repeated calls at the same level do not allocate.
/// Every triangle is counter-clockwise in clip space, matching the rasterizer's
/// front-face setting.
pub fn build_grid(count: u32, vertices: &mut Vec<Vertex>) {
    vertices.clear();
    if count == 0 {
        return;
    }
    vertices.reserve(count as usize * 3);

    let (columns, rows) = grid_dimensions(count);
    let cell_width = 2.0 / columns as f32;
    let cell_height = 2.0 / rows as f32;
    let half_width = cell_width * CELL_FILL * 0.5;
    let half_height = cell_height * CELL_FILL * 0.5;

    for index in 0..count {
        let column = index % columns;
        let row = index / columns;

        // Centre of this cell in clip space.
        let centre_x = -1.0 + (column as f32 + 0.5) * cell_width;
        let centre_y = 1.0 - (row as f32 + 0.5) * cell_height;

        let color = color_for(index, count);

        // Apex on top, then bottom-left, then bottom-right: counter-clockwise with Y up.
        vertices.push(Vertex {
            position: [centre_x, centre_y + half_height, 0.0],
            color,
        });
        vertices.push(Vertex {
            position: [centre_x - half_width, centre_y - half_height, 0.0],
            color,
        });
        vertices.push(Vertex {
            position: [centre_x + half_width, centre_y - half_height, 0.0],
            color,
        });
    }
}

/// A deterministic colour ramp, so the scene is visually informative rather than flat.
fn color_for(index: u32, count: u32) -> [f32; 4] {
    let t = if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32
    };
    // A simple three-phase ramp: red -> green -> blue as the grid is traversed.
    let red = (1.0 - t).clamp(0.0, 1.0);
    let green = (1.0 - (t - 0.5).abs() * 2.0).clamp(0.0, 1.0);
    let blue = t.clamp(0.0, 1.0);
    [
        0.15 + red * 0.85,
        0.15 + green * 0.85,
        0.15 + blue * 0.85,
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(count: u32) -> Vec<Vertex> {
        let mut vertices = Vec::new();
        build_grid(count, &mut vertices);
        vertices
    }

    #[test]
    fn levels_cover_the_required_range() {
        assert_eq!(STRESS_LEVELS, [1, 100, 1_000, 10_000, 100_000]);
    }

    #[test]
    fn level_lookup_saturates_instead_of_panicking() {
        assert_eq!(level_at(0), 1);
        assert_eq!(level_at(4), 100_000);
        assert_eq!(level_at(99), 100_000);
    }

    #[test]
    fn nearest_level_index_finds_the_closest_step() {
        assert_eq!(nearest_level_index(1), 0);
        assert_eq!(nearest_level_index(90), 1);
        assert_eq!(nearest_level_index(1_200), 2);
        assert_eq!(nearest_level_index(500_000), 4);
    }

    #[test]
    fn each_triangle_contributes_three_vertices() {
        for count in [1, 2, 100, 1_000] {
            assert_eq!(grid(count).len(), count as usize * 3, "count {count}");
        }
    }

    #[test]
    fn zero_triangles_produce_no_vertices() {
        assert!(grid(0).is_empty());
    }

    #[test]
    fn grid_is_as_square_as_possible_and_holds_every_triangle() {
        for count in [1, 2, 5, 100, 999, 1_000, 10_000, 100_000] {
            let (columns, rows) = grid_dimensions(count);
            assert!(
                columns * rows >= count,
                "{columns}x{rows} cannot hold {count} triangles"
            );
            assert!(
                columns.abs_diff(rows) <= columns.max(rows) / 2 + 1,
                "{columns}x{rows} is not roughly square for {count}"
            );
        }
    }

    #[test]
    fn every_vertex_stays_inside_the_clip_volume() {
        for count in [1, 100, 10_000] {
            for vertex in grid(count) {
                assert!(
                    (-1.0..=1.0).contains(&vertex.position[0]),
                    "x outside clip space at count {count}: {vertex:?}"
                );
                assert!(
                    (-1.0..=1.0).contains(&vertex.position[1]),
                    "y outside clip space at count {count}: {vertex:?}"
                );
            }
        }
    }

    #[test]
    fn every_triangle_is_counter_clockwise() {
        for count in [1, 7, 100] {
            for triangle in grid(count).chunks_exact(3) {
                let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
                let area = (b.position[0] - a.position[0]) * (c.position[1] - a.position[1])
                    - (c.position[0] - a.position[0]) * (b.position[1] - a.position[1]);
                assert!(area > 0.0, "count {count} produced a culled triangle");
            }
        }
    }

    #[test]
    fn triangles_do_not_overlap_their_neighbours() {
        // Two triangles side by side must leave a gap, or the "count" being measured
        // would not correspond to distinct primitives on screen.
        let vertices = grid(4);
        let first_right = vertices[2].position[0];
        let second_left = vertices[4].position[0];
        assert!(
            second_left > first_right,
            "cells overlap: {first_right} then {second_left}"
        );
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(grid(500), grid(500));
    }

    #[test]
    fn rebuilding_reuses_the_existing_allocation() {
        let mut vertices = Vec::new();
        build_grid(1_000, &mut vertices);
        let capacity = vertices.capacity();
        build_grid(1_000, &mut vertices);
        assert_eq!(vertices.capacity(), capacity, "rebuild must not reallocate");
    }

    #[test]
    fn colours_stay_in_range() {
        for vertex in grid(1_000) {
            for channel in vertex.color {
                assert!((0.0..=1.0).contains(&channel), "{vertex:?}");
            }
        }
    }
}
