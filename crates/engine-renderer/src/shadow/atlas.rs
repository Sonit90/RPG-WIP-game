//! Where each shadow view lives inside the one shadow texture.
//!
//! # Why an atlas, and not a texture per light
//!
//! §24 of the shadow plan asks the question and asks for the answer to be justified. For
//! this backend it is an atlas, for three reasons that are all properties of *this*
//! renderer rather than general truths:
//!
//! * **One shader-visible descriptor heap.** The renderer binds a single CBV/SRV/UAV heap
//!   once per frame and never switches it (see `Renderer::begin_recording`). A texture per
//!   light means a descriptor per light and a table bind per light in the lighting pass;
//!   one atlas means one SRV, bound with the material table that is already there.
//! * **Root arguments are scarce.** A shadow map per local light would need either a
//!   descriptor table per light or an unbounded array. One atlas needs neither: the
//!   per-light data is four floats of UV rectangle in a constant buffer the pass already
//!   binds.
//! * **One barrier.** The whole shadow pass ends with a single transition from
//!   `DEPTH_WRITE` to `PIXEL_SHADER_RESOURCE` rather than one per map.
//!
//! The cost is that every view shares one depth format and one resolution class, and that a
//! cascade cannot be resized without resizing everything. Neither matters here: the cascades
//! are all one size by construction, and a resolution change already recreates the resource.
//!
//! # The layout is fixed, not packed
//!
//! ```text
//! tile = one cascade's resolution
//!
//! +--------+--------+
//! | casc 0 | casc 1 |     slots are allocated in a fixed order:
//! +--------+--------+     cascades first, then local views into whatever
//! | casc 2 |LLLL LLLL|    slots are left, each subdivided into local tiles
//! +--------+--------+
//! ```
//!
//! A cleverer packer would waste less. It would also move a light's tile between frames as
//! the set of shadow-casting lights changed, and a shadow map that moves is a shadow map
//! whose filtering bleeds across a neighbour's edge for one frame. Fixed slots mean a view's
//! rectangle depends only on its index.

/// One view's rectangle inside the shadow atlas, in texels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasRect {
    /// Left edge, in texels.
    pub x: u32,
    /// Top edge, in texels.
    pub y: u32,
    /// Side length, in texels. Every shadow view is square.
    pub size: u32,
}

impl AtlasRect {
    /// The scale and bias that turn a view's [0, 1] shadow coordinate into an atlas
    /// coordinate.
    ///
    /// `[scale_x, scale_y, bias_x, bias_y]`, which the shader applies as
    /// `uv * scale + bias`. Computed on the CPU because it is per view and per frame, not
    /// per pixel.
    #[must_use]
    pub fn uv_scale_bias(&self, atlas_width: u32, atlas_height: u32) -> [f32; 4] {
        if atlas_width == 0 || atlas_height == 0 {
            return [0.0; 4];
        }
        let width = atlas_width as f32;
        let height = atlas_height as f32;
        [
            self.size as f32 / width,
            self.size as f32 / height,
            self.x as f32 / width,
            self.y as f32 / height,
        ]
    }

    /// Whether this rectangle overlaps `other`.
    ///
    /// For the test that proves the layout never hands two views the same texels — which
    /// would render as one light's shadow appearing under another.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.x < other.x + other.size
            && other.x < self.x + self.size
            && self.y < other.y + other.size
            && other.y < self.y + self.size
    }
}

/// Which rectangle of the atlas each shadow view gets.
///
/// Pure arithmetic over the settings — it owns no texture and needs no device. The resource
/// that *is* the atlas is created from [`ShadowAtlasLayout::size`] in
/// [`super::resources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowAtlasLayout {
    width: u32,
    height: u32,
    /// One cascade's side, and the size of a whole slot.
    tile: u32,
    /// One local view's side.
    local_tile: u32,
    /// Slots per row.
    columns: u32,
    /// Slot rows holding cascades.
    cascade_rows: u32,
    cascade_count: u32,
    local_views: u32,
}

impl ShadowAtlasLayout {
    /// The layout `settings` asks for.
    ///
    /// Mirrors [`ShadowSettings::atlas_size`](super::settings::ShadowSettings::atlas_size),
    /// which is asserted by a test: the size the resource is created at and the size the
    /// rectangles are computed against must be the same number.
    #[must_use]
    pub fn new(settings: &super::settings::ShadowSettings) -> Self {
        let (width, height) = settings.atlas_size();
        let columns = if settings.cascade_count > 1 { 2 } else { 1 } as u32;
        Self {
            width,
            height,
            tile: settings.resolution,
            local_tile: settings.local.resolution.min(settings.resolution),
            columns,
            cascade_rows: (settings.cascade_count as u32).div_ceil(columns),
            cascade_count: settings.cascade_count as u32,
            local_views: settings.local_view_budget() as u32,
        }
    }

    /// The atlas texture's size in texels, `(0, 0)` when shadows are off.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether there is any atlas at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// One cascade's side in texels.
    #[must_use]
    pub fn cascade_resolution(&self) -> u32 {
        self.tile
    }

    /// One local view's side in texels.
    #[must_use]
    pub fn local_resolution(&self) -> u32 {
        self.local_tile
    }

    /// How many local views this layout has room for.
    #[must_use]
    pub fn local_view_capacity(&self) -> u32 {
        self.local_views
    }

    /// Where cascade `index` is drawn.
    #[must_use]
    pub fn cascade_rect(&self, index: usize) -> Option<AtlasRect> {
        let index = index as u32;
        if self.is_empty() || index >= self.cascade_count {
            return None;
        }
        Some(self.slot_rect(index))
    }

    /// Where local shadow view `index` is drawn.
    ///
    /// Local views fill the slots the cascades left empty first — with three cascades in a
    /// two-by-two grid that is a whole quarter of the atlas — and then the rows beneath.
    #[must_use]
    pub fn local_rect(&self, index: usize) -> Option<AtlasRect> {
        let index = index as u32;
        if self.is_empty() || index >= self.local_views || self.local_tile == 0 {
            return None;
        }
        let per_slot_side = (self.tile / self.local_tile).max(1);
        let per_slot = per_slot_side * per_slot_side;
        let free_slots = self.columns * self.cascade_rows - self.cascade_count;

        if index < free_slots * per_slot {
            // Inside a slot the cascades did not use: pick the slot, then the sub-tile.
            let slot = self.cascade_count + index / per_slot;
            let within = index % per_slot;
            let base = self.slot_rect(slot);
            return Some(AtlasRect {
                x: base.x + (within % per_slot_side) * self.local_tile,
                y: base.y + (within / per_slot_side) * self.local_tile,
                size: self.local_tile,
            });
        }

        // Below the cascade grid, packed row by row across the full width.
        let remaining = index - free_slots * per_slot;
        let per_row = (self.width / self.local_tile).max(1);
        Some(AtlasRect {
            x: (remaining % per_row) * self.local_tile,
            y: self.cascade_rows * self.tile + (remaining / per_row) * self.local_tile,
            size: self.local_tile,
        })
    }

    /// The rectangle of whole slot `index`, in slot-major order.
    fn slot_rect(&self, index: u32) -> AtlasRect {
        AtlasRect {
            x: (index % self.columns) * self.tile,
            y: (index / self.columns) * self.tile,
            size: self.tile,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::settings::{ShadowPreset, ShadowSettings};

    fn every_rect(layout: &ShadowAtlasLayout) -> Vec<AtlasRect> {
        let mut rects = Vec::new();
        for index in 0..8 {
            if let Some(rect) = layout.cascade_rect(index) {
                rects.push(rect);
            }
        }
        for index in 0..32 {
            if let Some(rect) = layout.local_rect(index) {
                rects.push(rect);
            }
        }
        rects
    }

    #[test]
    fn no_two_views_share_a_texel() {
        // Two views in one rectangle is one light's shadow appearing under another, and it
        // is exactly the kind of bug a screenshot makes look like a shading problem.
        for preset in ShadowPreset::ALL {
            let settings = preset.settings();
            let layout = ShadowAtlasLayout::new(&settings);
            let rects = every_rect(&layout);
            for (index, rect) in rects.iter().enumerate() {
                for other in &rects[index + 1..] {
                    assert!(
                        !rect.overlaps(other),
                        "{}: {rect:?} overlaps {other:?}",
                        preset.as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn every_view_is_inside_the_texture_the_settings_allocate() {
        for preset in ShadowPreset::ALL {
            let settings = preset.settings();
            let layout = ShadowAtlasLayout::new(&settings);
            let (width, height) = layout.size();
            assert_eq!(
                (width, height),
                settings.atlas_size(),
                "the layout and the resource disagree about the atlas size"
            );
            for rect in every_rect(&layout) {
                assert!(
                    rect.x + rect.size <= width && rect.y + rect.size <= height,
                    "{}: {rect:?} falls outside {width}x{height}",
                    preset.as_str()
                );
            }
        }
    }

    #[test]
    fn every_configured_view_gets_a_rectangle() {
        for preset in ShadowPreset::ALL {
            let settings = preset.settings();
            let layout = ShadowAtlasLayout::new(&settings);
            if !settings.enabled {
                assert!(layout.is_empty());
                assert_eq!(layout.cascade_rect(0), None);
                continue;
            }
            for index in 0..settings.cascade_count {
                assert!(layout.cascade_rect(index).is_some(), "cascade {index}");
            }
            assert_eq!(layout.cascade_rect(settings.cascade_count), None);
            for index in 0..settings.local_view_budget() {
                assert!(layout.local_rect(index).is_some(), "local view {index}");
            }
            assert_eq!(layout.local_rect(settings.local_view_budget()), None);
        }
    }

    #[test]
    fn a_views_uv_rectangle_maps_its_corners_into_the_atlas() {
        let settings = ShadowPreset::High.settings();
        let layout = ShadowAtlasLayout::new(&settings);
        let (width, height) = layout.size();
        let rect = layout.cascade_rect(1).expect("cascade 1 exists");
        let [scale_x, scale_y, bias_x, bias_y] = rect.uv_scale_bias(width, height);

        // The view's own (0,0) and (1,1) must land on the rectangle's corners.
        assert!((bias_x - rect.x as f32 / width as f32).abs() < 1e-6);
        assert!((bias_y - rect.y as f32 / height as f32).abs() < 1e-6);
        assert!(((scale_x + bias_x) - (rect.x + rect.size) as f32 / width as f32).abs() < 1e-6);
        assert!(((scale_y + bias_y) - (rect.y + rect.size) as f32 / height as f32).abs() < 1e-6);
    }

    #[test]
    fn a_single_cascade_uses_one_column() {
        let settings = ShadowSettings {
            cascade_count: 1,
            ..ShadowPreset::High.settings()
        }
        .validated();
        let layout = ShadowAtlasLayout::new(&settings);
        let rect = layout.cascade_rect(0).expect("one cascade");
        assert_eq!(
            rect,
            AtlasRect {
                x: 0,
                y: 0,
                size: settings.resolution
            }
        );
    }

    #[test]
    fn the_ultra_layout_puts_local_views_under_a_full_cascade_grid() {
        // Four cascades fill the two-by-two grid exactly, so every local view has to go in
        // a row of its own beneath it. That is the branch the other presets never take.
        let settings = ShadowPreset::Ultra.settings();
        let layout = ShadowAtlasLayout::new(&settings);
        let grid_bottom = 2 * settings.resolution;
        for index in 0..settings.local_view_budget() {
            let rect = layout.local_rect(index).expect("a budgeted view");
            assert!(
                rect.y >= grid_bottom,
                "local view {index} at {rect:?} overlaps the cascade grid"
            );
        }
    }
}
