//! Where everything goes: the toolbar, the three docked panels, the viewport between them
//! and the status bar — and every rectangle inside them that a click can land in.
//!
//! ```text
//!  ┌──────────────────────────── toolbar ─────────────────────────────┐
//!  ├─────────────┬──────────────────────────────────┬─────────────────┤
//!  │ ASSETS      │                                  │ HIERARCHY       │
//!  │ [search]    │                                  │ [search]        │
//!  │ rows / tiles│            viewport              ├─────────────────┤
//!  │             │                                  │ INSPECTOR       │
//!  │ footer      │                                  │                 │
//!  ├─────────────┴──────────────────────────────────┴─────────────────┤
//!  └──────────────────────────── status bar ──────────────────────────┘
//! ```
//!
//! One function computes all of it, from the window, the interface scale and the widths the
//! author has dragged the splitters to. The painter and the input code both ask it, which is
//! what keeps a row that is drawn and a row that is clicked the same row at every window
//! size.

use engine_render_data::ui::{UiRect, UiScale};

use crate::theme::{Type, metrics};

/// The sizes an author can drag, in logical pixels, and the share of the right-hand column
/// the hierarchy takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelSizes {
    /// The browser's width.
    pub left: f32,
    /// The hierarchy and inspector column's width.
    pub right: f32,
    /// How much of the right-hand column is hierarchy, `0..1`.
    pub split: f32,
}

impl Default for PanelSizes {
    fn default() -> Self {
        Self {
            left: metrics::LEFT_DEFAULT,
            right: metrics::RIGHT_DEFAULT,
            split: 0.48,
        }
    }
}

/// Which splitter a drag is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Splitter {
    /// Between the browser and the viewport.
    Left,
    /// Between the viewport and the right-hand column.
    Right,
    /// Between the hierarchy and the inspector.
    Split,
}

/// Every rectangle the editor draws or tests, for one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The scale everything below was measured at.
    pub scale: UiScale,
    /// The text size at that scale.
    pub ty: Type,
    /// The window.
    pub window: (u32, u32),
    /// The toolbar along the top.
    pub toolbar: UiRect,
    /// The status bar along the bottom.
    pub status: UiRect,
    /// The content browser, down the left.
    pub left: UiRect,
    /// The hierarchy and the inspector, down the right.
    pub right: UiRect,
    /// The hierarchy panel.
    pub hierarchy: UiRect,
    /// The inspector panel.
    pub inspector: UiRect,
    /// The part of the window that shows the level and nothing else.
    pub viewport: UiRect,
    /// The browser's header strip.
    pub browser_header: UiRect,
    /// The browser's search field.
    pub browser_search: UiRect,
    /// Where the browser's rows or tiles go.
    pub browser_list: UiRect,
    /// The line under them: counts, and anything that could not be read.
    pub browser_footer: UiRect,
    /// The hierarchy's header strip.
    pub hierarchy_header: UiRect,
    /// The hierarchy's search field.
    pub hierarchy_search: UiRect,
    /// Where the hierarchy's rows go.
    pub hierarchy_list: UiRect,
    /// The inspector's header strip.
    pub inspector_header: UiRect,
    /// Where the inspector's properties go.
    pub inspector_body: UiRect,
}

impl Layout {
    /// The layout of a `window`-sized editor at `scale`, with the panels at `sizes` — clamped
    /// so that no panel is unusably narrow and the viewport never disappears.
    #[must_use]
    pub fn new(scale: UiScale, window: (u32, u32), sizes: PanelSizes) -> Self {
        let px = |logical: f32| scale.px(logical);
        let (width, height) = (window.0 as f32, window.1 as f32);
        let toolbar = UiRect::new(0.0, 0.0, width, px(metrics::TOOLBAR_HEIGHT));
        let status = UiRect::new(
            0.0,
            height - px(metrics::STATUS_HEIGHT),
            width,
            px(metrics::STATUS_HEIGHT),
        );
        let top = toolbar.bottom();
        let body = (status.y - top).max(0.0);

        let (left_width, right_width) = Self::widths(scale, width, sizes);
        let left = UiRect::new(0.0, top, left_width, body);
        let right = UiRect::new(width - right_width, top, right_width, body);
        let viewport = UiRect::new(left.right(), top, (right.x - left.right()).max(0.0), body);

        let header = px(metrics::HEADER_HEIGHT);
        let min_panel = header + px(metrics::ROW_HEIGHT * 3.0);
        let hierarchy_height = (body * sizes.split.clamp(0.15, 0.85)).clamp(
            min_panel.min(body * 0.5),
            (body - min_panel).max(body * 0.5),
        );
        let hierarchy = UiRect::new(right.x, top, right.width, hierarchy_height.round());
        let inspector = UiRect::new(
            right.x,
            hierarchy.bottom(),
            right.width,
            body - hierarchy.height,
        );

        let pad = px(metrics::PADDING);
        let field = px(metrics::FIELD_HEIGHT);
        let row = px(metrics::ROW_HEIGHT);

        let browser_header = UiRect::new(left.x, left.y, left.width, header);
        let browser_search = UiRect::new(
            left.x + pad,
            browser_header.bottom() + pad,
            left.width - pad * 2.0,
            field,
        );
        let browser_footer = UiRect::new(left.x, left.bottom() - row, left.width, row);
        let list_top = browser_search.bottom() + pad;
        let browser_list = UiRect::new(
            left.x,
            list_top,
            left.width,
            (browser_footer.y - list_top).max(0.0),
        );

        let hierarchy_header = UiRect::new(hierarchy.x, hierarchy.y, hierarchy.width, header);
        let hierarchy_search = UiRect::new(
            hierarchy.x + pad,
            hierarchy_header.bottom() + pad,
            hierarchy.width - pad * 2.0,
            field,
        );
        let hierarchy_top = hierarchy_search.bottom() + pad;
        let hierarchy_list = UiRect::new(
            hierarchy.x,
            hierarchy_top,
            hierarchy.width,
            (hierarchy.bottom() - hierarchy_top - px(2.0)).max(0.0),
        );

        let inspector_header = UiRect::new(inspector.x, inspector.y, inspector.width, header);
        let inspector_body = UiRect::new(
            inspector.x,
            inspector_header.bottom(),
            inspector.width,
            (inspector.bottom() - inspector_header.bottom()).max(0.0),
        );

        Self {
            scale,
            ty: Type::at(scale),
            window,
            toolbar,
            status,
            left,
            right,
            hierarchy,
            inspector,
            viewport,
            browser_header,
            browser_search,
            browser_list,
            browser_footer,
            hierarchy_header,
            hierarchy_search,
            hierarchy_list,
            inspector_header,
            inspector_body,
        }
    }

    /// The two side panels' widths in pixels, clamped to what the window can hold.
    ///
    /// On a narrow window both sides give way together, down to their minimums, so the
    /// viewport keeps the room it needs; only past that does the viewport shrink.
    fn widths(scale: UiScale, width: f32, sizes: PanelSizes) -> (f32, f32) {
        let px = |logical: f32| scale.px(logical);
        let left_min = px(metrics::LEFT_MIN);
        let right_min = px(metrics::RIGHT_MIN);
        let mut left = px(sizes.left).clamp(left_min, (width * 0.4).max(left_min));
        let mut right = px(sizes.right).clamp(right_min, (width * 0.4).max(right_min));
        let room = width - px(metrics::VIEWPORT_MIN);
        let over = left + right - room;
        if over > 0.0 {
            let give_left = (left - left_min).max(0.0);
            let give_right = (right - right_min).max(0.0);
            let give = give_left + give_right;
            if give > 0.0 {
                let take = over.min(give);
                left -= take * give_left / give;
                right -= take * give_right / give;
            }
        }
        (left.round(), right.round())
    }

    /// One logical pixel size in physical pixels.
    #[must_use]
    pub fn px(&self, logical: f32) -> f32 {
        self.scale.px(logical)
    }

    /// The height of a list row.
    #[must_use]
    pub fn row_height(&self) -> f32 {
        self.px(metrics::ROW_HEIGHT)
    }

    /// How many whole rows fit in `list`.
    #[must_use]
    pub fn rows_in(&self, list: UiRect) -> usize {
        (list.height / self.row_height()).floor().max(0.0) as usize
    }

    /// Row `slot` of `list`, counting from the top of what is visible.
    #[must_use]
    pub fn row_in(&self, list: UiRect, slot: usize) -> UiRect {
        let height = self.row_height();
        UiRect::new(list.x, list.y + slot as f32 * height, list.width, height)
    }

    /// How many browser rows fit.
    #[must_use]
    pub fn browser_rows(&self) -> usize {
        self.rows_in(self.browser_list)
    }

    /// Browser row `slot`.
    #[must_use]
    pub fn browser_row(&self, slot: usize) -> UiRect {
        self.row_in(self.browser_list, slot)
    }

    /// How many hierarchy rows fit.
    #[must_use]
    pub fn hierarchy_rows(&self) -> usize {
        self.rows_in(self.hierarchy_list)
    }

    /// Hierarchy row `slot`.
    #[must_use]
    pub fn hierarchy_row(&self, slot: usize) -> UiRect {
        self.row_in(self.hierarchy_list, slot)
    }

    /// The breadcrumb strip at the top of the browser's list in tile view.
    #[must_use]
    pub fn crumbs(&self) -> UiRect {
        UiRect::new(
            self.browser_list.x,
            self.browser_list.y,
            self.browser_list.width,
            self.row_height(),
        )
    }

    /// The area tiles are laid out in: the list, under the breadcrumb.
    #[must_use]
    pub fn tile_area(&self) -> UiRect {
        let crumbs = self.crumbs();
        UiRect::new(
            self.browser_list.x + self.px(metrics::PADDING),
            crumbs.bottom() + self.px(2.0),
            self.browser_list.width - self.px(metrics::PADDING) * 2.0,
            (self.browser_list.bottom() - crumbs.bottom() - self.px(2.0)).max(0.0),
        )
    }

    /// How many tiles fit across, and how many rows of them fit down.
    #[must_use]
    pub fn tile_grid(&self) -> (usize, usize) {
        let area = self.tile_area();
        let side = self.px(metrics::TILE);
        (
            ((area.width / side).floor() as usize).max(1),
            ((area.height / side).floor() as usize).max(1),
        )
    }

    /// Tile `slot`, counting across and then down from the top of what is visible.
    #[must_use]
    pub fn tile(&self, slot: usize) -> UiRect {
        let area = self.tile_area();
        let (columns, _) = self.tile_grid();
        let side = (area.width / columns as f32).floor();
        let height = self.px(metrics::TILE);
        UiRect::new(
            area.x + (slot % columns) as f32 * side,
            area.y + (slot / columns) as f32 * height,
            side,
            height,
        )
    }

    /// The zone a splitter can be grabbed in.
    #[must_use]
    pub fn splitter(&self, which: Splitter) -> UiRect {
        let grab = self.px(metrics::SPLITTER);
        match which {
            Splitter::Left => UiRect::new(
                self.left.right() - grab * 0.5,
                self.left.y,
                grab,
                self.left.height,
            ),
            Splitter::Right => UiRect::new(
                self.right.x - grab * 0.5,
                self.right.y,
                grab,
                self.right.height,
            ),
            Splitter::Split => UiRect::new(
                self.right.x,
                self.hierarchy.bottom() - grab * 0.5,
                self.right.width,
                grab,
            ),
        }
    }

    /// Whether `point` is over any of the editor's own interface rather than the level.
    #[must_use]
    pub fn is_chrome(&self, point: (f32, f32)) -> bool {
        !self.viewport.contains(point.0, point.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panels_tile_the_window_and_the_viewport_keeps_its_room() {
        for window in [
            (1280, 720),
            (1920, 1080),
            (2560, 1440),
            (3840, 2160),
            (900, 600),
        ] {
            let scale = crate::theme::scale_for(UiScale::new(1.0), window);
            let layout = Layout::new(scale, window, PanelSizes::default());
            assert_eq!(layout.left.x, 0.0);
            assert_eq!(layout.left.right(), layout.viewport.x, "{window:?}");
            assert_eq!(layout.viewport.right(), layout.right.x, "{window:?}");
            assert_eq!(layout.right.right(), window.0 as f32, "{window:?}");
            assert!(
                layout.viewport.width >= scale.px(metrics::VIEWPORT_MIN) - 1.0 || window.0 < 1000,
                "{window:?}: the viewport is {} wide",
                layout.viewport.width
            );
            assert!(layout.browser_rows() > 3, "{window:?}");
            assert!(layout.hierarchy_rows() > 2, "{window:?}");
            assert_eq!(layout.hierarchy.bottom(), layout.inspector.y);
        }
    }

    #[test]
    fn a_splitter_dragged_too_far_stops_at_the_minimum() {
        let scale = UiScale::new(1.0);
        let layout = Layout::new(
            scale,
            (1920, 1080),
            PanelSizes {
                left: 10.0,
                right: 5000.0,
                split: 0.5,
            },
        );
        assert_eq!(layout.left.width, metrics::LEFT_MIN);
        assert!(layout.right.width <= 1920.0 * 0.4 + 1.0);
    }
}
