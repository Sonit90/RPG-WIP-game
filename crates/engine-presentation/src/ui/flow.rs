//! A grid of equal items in a rectangle, filled in either direction on either axis: the one
//! layout container every multi-item HUD element is placed with.
//!
//! ```text
//!  LeftToRight, TopToBottom        RightToLeft, BottomToTop
//!  ┌───┬───┬───┬───┐               ┌───┬───┬───┬───┐
//!  │ 0 │ 1 │ 2 │ 3 │               │   │   │ 5 │ 4 │
//!  ├───┼───┼───┼───┤               ├───┼───┼───┼───┤
//!  │ 4 │ 5 │   │   │               │ 3 │ 2 │ 1 │ 0 │
//!  └───┴───┴───┴───┘               └───┴───┴───┴───┘
//! ```
//!
//! The buff bar, the raid frames, the action bars and the party list all ask this where item
//! `n` goes, and hit-testing asks the same question back. Direction is applied when a cell is
//! *placed*, never by reversing a list after layout: with three buffs in a right-to-left bar
//! the first buff is in the right-hand corner and the third is two cells in from it, whatever
//! the bar's capacity.

use engine_render_data::ui::UiRect;
use engine_settings::hud::{HudFlowX, HudFlowY};

/// Which axis items fill first before wrapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowAxis {
    /// Along a row, then the next row: buffs, a horizontal action bar.
    Rows,
    /// Down a column, then the next column: raid groups, a vertical action bar.
    Columns,
}

/// A resolved direction: `Auto` has already been replaced by the element's own default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowDirection {
    /// Whether the first item is at the right edge.
    pub right_to_left: bool,
    /// Whether the first item is at the bottom edge.
    pub bottom_to_top: bool,
}

impl FlowDirection {
    /// Left to right, top to bottom: reading order.
    pub const READING: Self = Self {
        right_to_left: false,
        bottom_to_top: false,
    };

    /// The stored directions, with `Auto` falling back to `natural`.
    #[must_use]
    pub fn resolve(x: HudFlowX, y: HudFlowY, natural: Self) -> Self {
        Self {
            right_to_left: match x {
                HudFlowX::Auto => natural.right_to_left,
                HudFlowX::LeftToRight => false,
                HudFlowX::RightToLeft => true,
            },
            bottom_to_top: match y {
                HudFlowY::Auto => natural.bottom_to_top,
                HudFlowY::TopToBottom => false,
                HudFlowY::BottomToTop => true,
            },
        }
    }

    /// The stored form of this direction.
    #[must_use]
    pub fn stored(self) -> (HudFlowX, HudFlowY) {
        (
            if self.right_to_left {
                HudFlowX::RightToLeft
            } else {
                HudFlowX::LeftToRight
            },
            if self.bottom_to_top {
                HudFlowY::BottomToTop
            } else {
                HudFlowY::TopToBottom
            },
        )
    }
}

/// A grid of `columns × rows` equal cells inside `rect`, `gap` apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flow {
    /// The rectangle the grid fills exactly.
    pub rect: UiRect,
    /// Cells across.
    pub columns: usize,
    /// Cells down.
    pub rows: usize,
    /// Which axis fills first.
    pub axis: FlowAxis,
    /// Which corner the first item is in.
    pub direction: FlowDirection,
    /// The gap between cells as a fraction of a cell's own size along that axis.
    ///
    /// A ratio rather than pixels, so a grid scaled by the HUD keeps its proportions — the
    /// rule the action bar has always followed.
    pub gap_ratio: f32,
}

impl Flow {
    /// The design size of a grid: `columns × rows` cells of `cell`, `gap` apart.
    #[must_use]
    pub fn design_size(cell: (f32, f32), gap: f32, columns: usize, rows: usize) -> (f32, f32) {
        let (columns, rows) = (columns.max(1) as f32, rows.max(1) as f32);
        (
            columns * cell.0 + (columns - 1.0) * gap,
            rows * cell.1 + (rows - 1.0) * gap,
        )
    }

    /// How many items fit.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.columns.max(1) * self.rows.max(1)
    }

    /// The cell item `index` is placed in, or `None` past the capacity.
    #[must_use]
    pub fn cell(&self, index: usize) -> Option<UiRect> {
        let (columns, rows) = (self.columns.max(1), self.rows.max(1));
        if index >= columns * rows {
            return None;
        }
        let (column, row) = match self.axis {
            FlowAxis::Rows => (index % columns, index / columns),
            FlowAxis::Columns => (index / rows, index % rows),
        };
        let column = if self.direction.right_to_left {
            columns - 1 - column
        } else {
            column
        };
        let row = if self.direction.bottom_to_top {
            rows - 1 - row
        } else {
            row
        };
        let ratio = self.gap_ratio.max(0.0);
        // cells * w + (cells - 1) * w * ratio = extent
        let width = self.rect.width / (columns as f32 + (columns as f32 - 1.0) * ratio);
        let height = self.rect.height / (rows as f32 + (rows as f32 - 1.0) * ratio);
        // Not rounded: a cell must never poke out of the rectangle it was cut from, and the
        // painters snap what they draw.
        Some(UiRect::new(
            self.rect.x + column as f32 * width * (1.0 + ratio),
            self.rect.y + row as f32 * height * (1.0 + ratio),
            width,
            height,
        ))
    }

    /// Which item's cell contains `point`, among the first `count`.
    #[must_use]
    pub fn index_at(&self, count: usize, point: (f32, f32)) -> Option<usize> {
        if !self.rect.contains(point.0, point.1) {
            return None;
        }
        (0..count.min(self.capacity())).find(|index| {
            self.cell(*index)
                .is_some_and(|cell| cell.contains(point.0, point.1))
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn grid(direction: FlowDirection) -> Flow {
        Flow {
            rect: UiRect::new(0.0, 0.0, 400.0, 200.0),
            columns: 4,
            rows: 2,
            axis: FlowAxis::Rows,
            direction,
            gap_ratio: 0.0,
        }
    }

    #[test]
    fn reading_order_starts_top_left() {
        let flow = grid(FlowDirection::READING);
        assert_eq!(flow.cell(0), Some(UiRect::new(0.0, 0.0, 100.0, 100.0)));
        assert_eq!(flow.cell(4), Some(UiRect::new(0.0, 100.0, 100.0, 100.0)));
        assert_eq!(flow.cell(8), None);
    }

    #[test]
    fn right_to_left_puts_the_first_item_in_the_right_corner_whatever_the_count() {
        let flow = grid(FlowDirection {
            right_to_left: true,
            bottom_to_top: false,
        });
        // The first of three is at the right edge: placed there, not reversed afterwards.
        assert_eq!(flow.cell(0).map(|cell| cell.right()), Some(400.0));
        assert_eq!(flow.cell(2).map(|cell| cell.x), Some(100.0));
    }

    #[test]
    fn bottom_to_top_starts_on_the_bottom_row() {
        let flow = grid(FlowDirection {
            right_to_left: false,
            bottom_to_top: true,
        });
        assert_eq!(flow.cell(0).map(|cell| cell.bottom()), Some(200.0));
        assert_eq!(flow.cell(4).map(|cell| cell.y), Some(0.0));
    }

    #[test]
    fn columns_fill_downwards_first() {
        let flow = Flow {
            axis: FlowAxis::Columns,
            ..grid(FlowDirection::READING)
        };
        assert_eq!(flow.cell(1), Some(UiRect::new(0.0, 100.0, 100.0, 100.0)));
        assert_eq!(flow.cell(2), Some(UiRect::new(100.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn the_gap_is_a_share_of_the_cell_and_the_grid_still_fills_the_rect() {
        let flow = Flow {
            gap_ratio: 0.25,
            ..grid(FlowDirection::READING)
        };
        let last = flow.cell(7).expect("in the grid");
        assert!((last.right() - 400.0).abs() <= 1.0, "{last:?}");
        let first = flow.cell(0).expect("in the grid");
        let second = flow.cell(1).expect("in the grid");
        assert!(
            second.x - first.right() > 1.0,
            "no gap: {first:?} {second:?}"
        );
    }

    #[test]
    fn a_point_finds_the_item_drawn_under_it() {
        for right_to_left in [false, true] {
            for bottom_to_top in [false, true] {
                let flow = grid(FlowDirection {
                    right_to_left,
                    bottom_to_top,
                });
                for index in 0..8 {
                    let cell = flow.cell(index).expect("in the grid");
                    let centre = (cell.x + cell.width * 0.5, cell.y + cell.height * 0.5);
                    assert_eq!(flow.index_at(8, centre), Some(index));
                }
                // Past the count there is nothing, even inside the rectangle.
                let unused = flow.cell(7).expect("in the grid");
                assert_eq!(flow.index_at(7, (unused.x + 1.0, unused.y + 1.0)), None);
            }
        }
    }
}
