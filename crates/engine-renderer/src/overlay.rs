//! Debug performance overlay.
//!
//! Text is drawn as one small quad per lit font cell, through the *same* root signature
//! and pipeline state as the triangle: position and colour, no texture, no sampler, no
//! shader-visible descriptor heap and no second shader. That keeps the whole overlay to a
//! vertex generator plus a per-frame upload buffer, which is the point — a debug readout
//! must not justify a UI framework.
//!
//! The entire module is behind the `debug-overlay` feature and is absent from a build
//! made with `--no-default-features`.
//!
//! Allocation: the vertex scratch buffer and the per-slot upload buffers are allocated
//! once. Building a frame's geometry reuses the scratch, so the frame path allocates
//! nothing as long as the text fits the reserved capacity.

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_UPLOAD, D3D12_RANGE,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_STATE_GENERIC_READ,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, D3D12_VERTEX_BUFFER_VIEW, ID3D12Device, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};

use crate::error::RendererError;
use crate::font::{GLYPH_ADVANCE, GLYPH_WIDTH, LINE_ADVANCE, glyph};
use crate::pipeline::Vertex;

/// Vertices per quad: two triangles, no index buffer.
const VERTICES_PER_QUAD: usize = 6;

/// Upper bound on quads per frame, and therefore on the upload buffer size.
///
/// The showcase overlay is about twenty lines of thirty characters plus a 120-sample frame
/// graph; at up to 35 lit cells per character that is roughly 21,000 quads. 32,768 leaves
/// headroom without being a meaningful amount of memory (32,768 * 6 * 28 bytes ≈ 5.4 MiB
/// per frame slot).
const MAX_QUADS: usize = 32_768;

/// Screen-space position of the overlay's top-left corner, in pixels.
const MARGIN_PIXELS: f32 = 12.0;

/// Text colour: a bright, slightly warm white that stays readable over dark backgrounds.
const TEXT_COLOR: [f32; 4] = [0.95, 0.95, 0.85, 1.0];

/// Colour of a frame-graph bar that is inside its budget.
const GRAPH_GOOD_COLOR: [f32; 4] = [0.35, 0.85, 0.45, 1.0];

/// Colour of a frame-graph bar that is over its budget.
///
/// A single number cannot show a spike that lasts three frames out of a hundred; a graph
/// can, and colouring the overrun is what makes it visible without reading the axis.
const GRAPH_BAD_COLOR: [f32; 4] = [0.95, 0.45, 0.25, 1.0];

/// Colour of the frame graph's background panel, and of the text backdrop.
///
/// Opaque, not translucent: the overlay draws through the scene's blend state, which has
/// blending switched off because everything else it draws is opaque. A readable overlay is
/// worth one dark rectangle; it is not worth a second blend state.
const GRAPH_BACKGROUND_COLOR: [f32; 4] = [0.12, 0.14, 0.18, 1.0];

/// Clip-space depth of everything the overlay draws in front.
///
/// Zero is the near plane, so an overlay quad passes the `LESS` depth test against any
/// scene geometry and lands on top of the frame.
const FOREGROUND_DEPTH: f32 = 0.0;

/// Clip-space depth of the frame graph's background panel.
///
/// Behind the bars, and this is load-bearing rather than cosmetic: the overlay shares the
/// scene's depth-testing pipeline, so a panel drawn at the same depth as the bars *writes*
/// that depth first and the bars then fail `LESS` and vanish. They are separated instead of
/// the test being disabled, because the same pipeline draws the scene.
const GRAPH_BACKGROUND_DEPTH: f32 = 0.5;

/// Builds overlay geometry in clip space.
///
/// Reused between frames; `begin` clears it without releasing its capacity.
pub struct OverlayGeometry {
    vertices: Vec<Vertex>,
    /// Cell size in pixels, so the overlay stays legible as resolution changes.
    cell_pixels: f32,
}

impl OverlayGeometry {
    /// Creates a generator with capacity for [`MAX_QUADS`] quads.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vertices: Vec::with_capacity(MAX_QUADS * VERTICES_PER_QUAD),
            cell_pixels: 2.0,
        }
    }

    /// Discards the previous frame's geometry, keeping the allocation.
    pub fn begin(&mut self) {
        self.vertices.clear();
    }

    /// Appends one line of text, with `line` counting down from the top.
    ///
    /// Coordinates are in pixels of a `width` x `height` render target, converted to clip
    /// space here. Cells beyond [`MAX_QUADS`] are dropped rather than growing the buffer,
    /// which keeps the frame path allocation-free.
    pub fn push_line(&mut self, line: usize, text: &str, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let cell = self.cell_pixels;
        let origin_y = MARGIN_PIXELS + line as f32 * LINE_ADVANCE as f32 * cell;

        for (index, character) in text.chars().enumerate() {
            let Some(rows) = glyph(character) else {
                continue;
            };
            let origin_x = MARGIN_PIXELS + index as f32 * GLYPH_ADVANCE as f32 * cell;

            for (row_index, row) in rows.into_iter().enumerate() {
                for column in 0..GLYPH_WIDTH {
                    // Bit GLYPH_WIDTH-1 is the leftmost pixel.
                    if row & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                        continue;
                    }
                    if self.vertices.len() + VERTICES_PER_QUAD > self.vertices.capacity() {
                        return;
                    }
                    let x = origin_x + column as f32 * cell;
                    let y = origin_y + row_index as f32 * cell;
                    self.push_quad(x, y, cell, width, height);
                }
            }
        }
    }

    /// Appends an opaque backdrop large enough for `lines` rows of `columns` characters.
    ///
    /// Without it the text sits directly on the scene and is unreadable wherever the scene
    /// is bright — which, in a demo whose whole purpose is to be looked at, is most of the
    /// screen. Drawn behind the text, which is what [`GRAPH_BACKGROUND_DEPTH`] arranges.
    ///
    /// Appends, so it must be called after the lines it covers; the depth test, not the
    /// draw order, decides which one wins.
    pub fn push_backdrop(&mut self, lines: usize, columns: usize, width: u32, height: u32) {
        if width == 0 || height == 0 || lines == 0 || columns == 0 {
            return;
        }
        let cell = self.cell_pixels;
        let text_width = columns as f32 * GLYPH_ADVANCE as f32 * cell;
        let text_height = lines as f32 * LINE_ADVANCE as f32 * cell;
        self.push_colored_quad(
            (
                0.0,
                0.0,
                // The margin again on the far side, so the text is inset rather than
                // touching the edge of its own backdrop.
                MARGIN_PIXELS * 2.0 + text_width,
                MARGIN_PIXELS * 2.0 + text_height,
            ),
            GRAPH_BACKGROUND_COLOR,
            GRAPH_BACKGROUND_DEPTH,
            (width, height),
        );
    }

    /// Appends a frame-time graph: one bar per sample, oldest on the left.
    ///
    /// `samples` are milliseconds and `budget_ms` is the height the panel is scaled to; a
    /// sample above it is clamped to full height and drawn in the overrun colour, so an
    /// occasional spike is visible as a red bar rather than as a rescaled graph in which
    /// every other bar suddenly shrinks.
    ///
    /// The panel is placed by its bottom-left corner in pixels. Appends, so callers push
    /// their text lines first and the graph afterwards.
    pub fn push_frame_graph(
        &mut self,
        samples: &[f32],
        budget_ms: f32,
        origin: (f32, f32),
        size: (f32, f32),
        width: u32,
        height: u32,
    ) {
        if width == 0 || height == 0 || samples.is_empty() || size.0 <= 0.0 || size.1 <= 0.0 {
            return;
        }
        let budget = if budget_ms > 0.0 { budget_ms } else { 1.0 };
        let (left, bottom) = origin;
        let (panel_width, panel_height) = size;

        self.push_colored_quad(
            (left, bottom - panel_height, panel_width, panel_height),
            GRAPH_BACKGROUND_COLOR,
            GRAPH_BACKGROUND_DEPTH,
            (width, height),
        );

        // Bars are sized to fill the panel exactly, so the graph's width says how many
        // frames it covers rather than being a function of how wide a bar happens to be.
        let bar_width = panel_width / samples.len() as f32;
        for (index, sample) in samples.iter().enumerate() {
            let over_budget = *sample > budget;
            let fraction = (*sample / budget).clamp(0.0, 1.0);
            let bar_height = (fraction * panel_height).max(1.0);
            let color = if over_budget {
                GRAPH_BAD_COLOR
            } else {
                GRAPH_GOOD_COLOR
            };
            self.push_colored_quad(
                (
                    left + index as f32 * bar_width,
                    bottom - bar_height,
                    // A pixel of gap, so individual frames stay distinguishable.
                    (bar_width - 1.0).max(1.0),
                    bar_height,
                ),
                color,
                FOREGROUND_DEPTH,
                (width, height),
            );
        }
    }

    /// Emits the two counter-clockwise triangles of one cell.
    ///
    /// Winding matches `rasterizer_desc`: counter-clockwise in clip space, or back-face
    /// culling would discard the overlay exactly as it once discarded the triangle.
    fn push_quad(&mut self, x: f32, y: f32, size: f32, width: u32, height: u32) {
        self.push_colored_quad(
            (x, y, size, size),
            TEXT_COLOR,
            FOREGROUND_DEPTH,
            (width, height),
        );
    }

    /// Emits one axis-aligned rectangle of an arbitrary size, colour and clip-space depth.
    ///
    /// `rect` is `(x, y, width, height)` in pixels from the top-left of a `target`-sized
    /// render target. Overflow is dropped rather than allocated for, which is what keeps
    /// the frame path allocation-free.
    fn push_colored_quad(
        &mut self,
        rect: (f32, f32, f32, f32),
        color: [f32; 4],
        depth: f32,
        target: (u32, u32),
    ) {
        if self.vertices.len() + VERTICES_PER_QUAD > self.vertices.capacity() {
            return;
        }
        let (x, y, quad_width, quad_height) = rect;
        let (width, height) = target;
        let to_clip_x = |px: f32| px / width as f32 * 2.0 - 1.0;
        let to_clip_y = |py: f32| 1.0 - py / height as f32 * 2.0;

        let left = to_clip_x(x);
        let right = to_clip_x(x + quad_width);
        let top = to_clip_y(y);
        let bottom = to_clip_y(y + quad_height);

        let corner = |cx: f32, cy: f32| Vertex {
            position: [cx, cy, depth],
            color,
        };

        self.vertices.extend_from_slice(&[
            corner(left, top),
            corner(left, bottom),
            corner(right, bottom),
            corner(left, top),
            corner(right, bottom),
            corner(right, top),
        ]);
    }

    /// The generated vertices.
    #[must_use]
    pub fn vertices(&self) -> &[Vertex] {
        &self.vertices
    }

    /// Whether anything was generated this frame.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// Sets the size of one font cell in pixels.
    pub fn set_cell_pixels(&mut self, pixels: f32) {
        self.cell_pixels = pixels.max(1.0);
    }
}

impl Default for OverlayGeometry {
    fn default() -> Self {
        Self::new()
    }
}

/// One upload-heap vertex buffer per frame slot, mapped for the whole lifetime.
///
/// A frame slot's buffer may still be read by the GPU while the next frame is recorded,
/// which is why each slot owns one rather than sharing a single buffer.
pub struct OverlayBuffers {
    buffers: Vec<MappedVertexBuffer>,
}

impl OverlayBuffers {
    /// Creates `slot_count` buffers, each holding [`MAX_QUADS`] quads.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a buffer cannot be created or mapped.
    pub fn new(device: &ID3D12Device, slot_count: usize) -> Result<Self, RendererError> {
        let mut buffers = Vec::with_capacity(slot_count);
        for _ in 0..slot_count {
            buffers.push(MappedVertexBuffer::new(
                device,
                MAX_QUADS * VERTICES_PER_QUAD,
            )?);
        }
        Ok(Self { buffers })
    }

    /// Copies `vertices` into `slot`'s buffer and returns the view to bind.
    ///
    /// Returns `None` if there is nothing to draw.
    pub fn upload(&mut self, slot: usize, vertices: &[Vertex]) -> Option<D3D12_VERTEX_BUFFER_VIEW> {
        self.buffers.get_mut(slot)?.write(vertices)
    }
}

/// An upload-heap vertex buffer that stays mapped.
///
/// Upload heaps are CPU-write-combined and GPU-readable, and D3D12 permits a mapping to
/// persist. Keeping it mapped avoids a `Map`/`Unmap` pair per frame; the memory is only
/// ever written, never read back, so no cache invalidation is required.
struct MappedVertexBuffer {
    resource: ID3D12Resource,
    mapped: *mut u8,
    capacity_vertices: usize,
    gpu_address: u64,
}

impl MappedVertexBuffer {
    fn new(device: &ID3D12Device, capacity_vertices: usize) -> Result<Self, RendererError> {
        let byte_size = capacity_vertices * size_of::<Vertex>();

        let heap_properties = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: byte_size as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };

        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: FFI call with initialized descriptors that outlive it. GENERIC_READ is
        // the required initial state for an upload-heap resource.
        unsafe {
            device.CreateCommittedResource(
                &heap_properties,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
                &mut resource,
            )
        }
        .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;

        let Some(resource) = resource else {
            return Err(RendererError::InvalidConfig {
                reason: "overlay vertex buffer creation produced no resource".to_owned(),
            });
        };

        let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
        // A read range of 0..0 states that the CPU will never read this buffer.
        let read_range = D3D12_RANGE { Begin: 0, End: 0 };
        // SAFETY: FFI call mapping subresource 0 of an upload-heap buffer. The mapping is
        // kept for the lifetime of the resource, which D3D12 permits.
        unsafe { resource.Map(0, Some(&read_range), Some(&mut mapped)) }
            .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

        // SAFETY: FFI call returning the resource's GPU address, valid while it lives.
        let gpu_address = unsafe { resource.GetGPUVirtualAddress() };

        Ok(Self {
            resource,
            mapped: mapped.cast::<u8>(),
            capacity_vertices,
            gpu_address,
        })
    }

    /// Copies `vertices` into the buffer, returning the view that describes them.
    fn write(&mut self, vertices: &[Vertex]) -> Option<D3D12_VERTEX_BUFFER_VIEW> {
        if vertices.is_empty() {
            return None;
        }
        let count = vertices.len().min(self.capacity_vertices);
        let bytes = count * size_of::<Vertex>();

        // SAFETY: the mapping covers `capacity_vertices` vertices and `count` is clamped
        // to it, so the write stays inside the allocation. Source and destination are
        // distinct allocations, and the GPU is not reading this slot's buffer — the
        // renderer waited on that slot's fence before recording this frame.
        unsafe {
            std::ptr::copy_nonoverlapping(vertices.as_ptr().cast::<u8>(), self.mapped, bytes);
        }

        Some(D3D12_VERTEX_BUFFER_VIEW {
            BufferLocation: self.gpu_address,
            SizeInBytes: bytes as u32,
            StrideInBytes: size_of::<Vertex>() as u32,
        })
    }
}

impl Drop for MappedVertexBuffer {
    fn drop(&mut self) {
        // SAFETY: FFI call unmapping the subresource mapped in `new`. A written range of
        // 0..0 would claim nothing changed, so `None` is passed to let the driver assume
        // the whole buffer may have been written.
        unsafe { self.resource.Unmap(0, None) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry_for(text: &str) -> OverlayGeometry {
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_line(0, text, 1920, 1080);
        geometry
    }

    #[test]
    fn empty_text_produces_no_geometry() {
        let geometry = geometry_for("");
        assert!(geometry.is_empty());
    }

    #[test]
    fn spaces_produce_no_geometry() {
        assert!(geometry_for("   ").is_empty());
    }

    #[test]
    fn each_lit_cell_becomes_one_quad() {
        let geometry = geometry_for("T");
        let expected_quads = crate::font::lit_cells("T");
        assert_eq!(
            geometry.vertices().len(),
            expected_quads * VERTICES_PER_QUAD
        );
    }

    #[test]
    fn quads_are_counter_clockwise_like_the_rest_of_the_pipeline() {
        let geometry = geometry_for("L");
        for triangle in geometry.vertices().chunks_exact(3) {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            let area = (b.position[0] - a.position[0]) * (c.position[1] - a.position[1])
                - (c.position[0] - a.position[0]) * (b.position[1] - a.position[1]);
            assert!(
                area > 0.0,
                "overlay quads must be counter-clockwise or they are culled, area {area}"
            );
        }
    }

    #[test]
    fn geometry_stays_inside_the_clip_volume() {
        let geometry = geometry_for("PERFORMANCE 1234567890");
        for vertex in geometry.vertices() {
            assert!(
                (-1.0..=1.0).contains(&vertex.position[0]),
                "x out of range: {vertex:?}"
            );
            assert!(
                (-1.0..=1.0).contains(&vertex.position[1]),
                "y out of range: {vertex:?}"
            );
        }
    }

    #[test]
    fn text_starts_in_the_top_left_corner() {
        let geometry = geometry_for("L");
        let first = geometry.vertices()[0];
        assert!(first.position[0] < -0.9, "should start near the left edge");
        assert!(first.position[1] > 0.9, "should start near the top edge");
    }

    #[test]
    fn later_lines_are_lower_on_screen() {
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_line(0, "L", 1920, 1080);
        let first_line_top = geometry.vertices()[0].position[1];

        geometry.begin();
        geometry.push_line(3, "L", 1920, 1080);
        let fourth_line_top = geometry.vertices()[0].position[1];

        assert!(
            fourth_line_top < first_line_top,
            "line 3 ({fourth_line_top}) must sit below line 0 ({first_line_top})"
        );
    }

    #[test]
    fn a_frame_graph_emits_a_panel_and_one_bar_per_sample() {
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        let samples = [1.0_f32, 2.0, 3.0, 4.0];
        geometry.push_frame_graph(&samples, 4.0, (10.0, 500.0), (200.0, 80.0), 1920, 1080);
        assert_eq!(
            geometry.vertices().len(),
            (samples.len() + 1) * VERTICES_PER_QUAD,
            "one background panel plus one bar per sample"
        );
    }

    #[test]
    fn graph_bars_sit_in_front_of_their_own_background_panel() {
        // The overlay shares the scene's depth-testing pipeline. A panel drawn at the same
        // depth as the bars writes that depth first, the bars then fail LESS, and the graph
        // renders as an empty rectangle — which is exactly what happened before this test.
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_frame_graph(
            &[1.0, 2.0, 3.0],
            4.0,
            (10.0, 500.0),
            (200.0, 80.0),
            1920,
            1080,
        );

        let vertices = geometry.vertices();
        let panel_depth = vertices[0].position[2];
        for bar in vertices[VERTICES_PER_QUAD..].iter() {
            assert!(
                bar.position[2] < panel_depth,
                "a bar at depth {} cannot pass LESS against a panel at {panel_depth}",
                bar.position[2]
            );
        }
    }

    #[test]
    fn text_is_drawn_at_the_near_plane_so_the_scene_cannot_hide_it() {
        let geometry = geometry_for("FPS");
        assert!(
            geometry
                .vertices()
                .iter()
                .all(|vertex| vertex.position[2] == FOREGROUND_DEPTH)
        );
    }

    #[test]
    fn a_backdrop_covers_the_text_it_is_sized_for_and_sits_behind_it() {
        let mut geometry = geometry_for("ABCDE");
        let text_bottom = geometry
            .vertices()
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::INFINITY, f32::min);
        let text_right = geometry
            .vertices()
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);

        geometry.push_backdrop(1, 5, 1920, 1080);
        let backdrop = &geometry.vertices()[geometry.vertices().len() - VERTICES_PER_QUAD..];
        let backdrop_bottom = backdrop
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::INFINITY, f32::min);
        let backdrop_right = backdrop
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::NEG_INFINITY, f32::max);

        assert!(backdrop_bottom < text_bottom, "the text must fit inside");
        assert!(backdrop_right > text_right, "the text must fit inside");
        assert!(
            backdrop.iter().all(|vertex| vertex.position[2] > 0.0),
            "the backdrop must be behind the text it backs"
        );
    }

    #[test]
    fn an_empty_backdrop_produces_nothing() {
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_backdrop(0, 30, 1920, 1080);
        geometry.push_backdrop(20, 0, 1920, 1080);
        geometry.push_backdrop(20, 30, 0, 0);
        assert!(geometry.is_empty());
    }

    #[test]
    fn a_frame_graph_appends_rather_than_replacing_the_text() {
        let mut geometry = geometry_for("FPS");
        let text_only = geometry.vertices().len();
        geometry.push_frame_graph(&[1.0, 2.0], 4.0, (10.0, 500.0), (200.0, 80.0), 1920, 1080);
        assert!(geometry.vertices().len() > text_only);
    }

    #[test]
    fn a_spike_is_clamped_to_the_panel_rather_than_rescaling_it() {
        // A 60 ms hitch must not shrink every other bar to invisibility; it is clamped and
        // recoloured instead.
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_frame_graph(&[60.0], 4.0, (0.0, 1080.0), (100.0, 80.0), 1920, 1080);
        for vertex in geometry.vertices() {
            assert!(
                (-1.0..=1.0).contains(&vertex.position[1]),
                "a spike must stay inside the panel: {vertex:?}"
            );
        }
        let bar = geometry.vertices()[VERTICES_PER_QUAD];
        assert_eq!(bar.color, GRAPH_BAD_COLOR, "an overrun is coloured");
    }

    #[test]
    fn an_empty_frame_graph_produces_nothing() {
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_frame_graph(&[], 4.0, (10.0, 500.0), (200.0, 80.0), 1920, 1080);
        geometry.push_frame_graph(&[1.0], 4.0, (10.0, 500.0), (0.0, 0.0), 1920, 1080);
        assert!(geometry.is_empty());
    }

    #[test]
    fn begin_clears_previous_geometry() {
        let mut geometry = geometry_for("HELLO");
        assert!(!geometry.is_empty());
        geometry.begin();
        assert!(geometry.is_empty());
    }

    #[test]
    fn a_zero_sized_target_produces_nothing_instead_of_dividing_by_zero() {
        let mut geometry = OverlayGeometry::new();
        geometry.begin();
        geometry.push_line(0, "TEST", 0, 0);
        assert!(geometry.is_empty());
    }

    #[test]
    fn generation_never_grows_the_buffer() {
        let mut geometry = OverlayGeometry::new();
        let capacity = geometry.vertices.capacity();
        geometry.begin();
        // Far more text than the overlay will ever show.
        for line in 0..64 {
            geometry.push_line(
                line,
                &"WWWWWWWWWWWWWWWWWWWWWWWWWWWWWW".repeat(4),
                1920,
                1080,
            );
        }
        assert_eq!(
            geometry.vertices.capacity(),
            capacity,
            "overflow must be dropped, not allocated for"
        );
    }
}
