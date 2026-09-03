//! The batched UI pass: one pipeline, one texture, one draw call for the whole HUD.
//!
//! ```text
//! UiDrawList  ->  UiBatcher  ->  [UiVertex]  ->  upload ring  ->  one DrawInstanced
//!  (commands)     (this file)     one buffer
//! ```
//!
//! Every UI element — a panel, a health bar, an outline, a glyph, an ability icon — is a
//! textured, tinted quad. Glyphs sample their cell in the UI atlas; icons sample theirs;
//! solid shapes sample the one opaque texel in the same atlas. Because they share a texture
//! and a pipeline state, they share a **batch**, and the entire user interface is submitted
//! as a single draw.
//!
//! ## What this replaces
//!
//! Phase 3's overlay emitted one quad per lit font cell — roughly fifteen quads per
//! character — and drew each overlay layer separately. Milestone 2 measured twenty
//! nameplates at 0.15-0.23 ms of CPU and a CPU p95 of 0.97 ms against 0.26 ms without them.
//! Here a character is one quad, so the same text is about a fifteenth of the geometry.
//!
//! ## Ordering
//!
//! Depth testing and depth writing are **off**. The list is painted in order, which is the
//! only ordering strategy that works once anything is translucent: a blended fragment that
//! fails a depth test vanishes, and one that passes writes depth that occludes whatever
//! should have blended over it. See `engine_render_data::ui` for the full reasoning.

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_BLEND_DESC, D3D12_BLEND_INV_SRC_ALPHA, D3D12_BLEND_ONE,
    D3D12_BLEND_OP_ADD, D3D12_BLEND_SRC_ALPHA, D3D12_COLOR_WRITE_ENABLE_ALL,
    D3D12_COMPARISON_FUNC_ALWAYS, D3D12_CULL_MODE_NONE, D3D12_DEPTH_STENCIL_DESC,
    D3D12_DEPTH_WRITE_MASK_ZERO, D3D12_DESCRIPTOR_RANGE, D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
    D3D12_FILL_MODE_SOLID, D3D12_FILTER_MIN_MAG_MIP_LINEAR, D3D12_FILTER_MIN_MAG_MIP_POINT,
    D3D12_GRAPHICS_PIPELINE_STATE_DESC, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
    D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC, D3D12_LOGIC_OP_NOOP,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_RASTERIZER_DESC, D3D12_RENDER_TARGET_BLEND_DESC,
    D3D12_ROOT_DESCRIPTOR, D3D12_ROOT_DESCRIPTOR_TABLE, D3D12_ROOT_PARAMETER,
    D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_CBV,
    D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SHADER_VISIBILITY_ALL,
    D3D12_SHADER_VISIBILITY_PIXEL, D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
    D3D12_STATIC_SAMPLER_DESC, D3D12_TEXTURE_ADDRESS_MODE_CLAMP, D3D12SerializeRootSignature,
    ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32A32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use engine_core::jobs::JobSystem;
use engine_render_data::ui::{TextAlign, UiCommand, UiDrawList, UiRect};

use crate::error::RendererError;
use crate::font::{GLYPH_ADVANCE, GLYPH_HEIGHT, GLYPH_WIDTH};
use crate::shader::{UI_PS, UI_VS, bytecode};
use crate::swapchain::BACK_BUFFER_FORMAT;
use crate::ui_atlas::{AtlasUv, UiAtlas};

/// Root parameter holding the pixels-to-clip constant buffer.
pub const ROOT_UI_FRAME_CBV: u32 = 0;

/// Root parameter holding the UI-atlas descriptor table.
pub const ROOT_UI_ATLAS_TABLE: u32 = 1;

/// Register of the point sampler the font band is read through.
pub const SAMPLER_GLYPH: u32 = 0;

/// Register of the linear sampler the icon band is read through.
pub const SAMPLER_ICON: u32 = 1;

/// Angles a single radial sweep can turn at: its two ends, plus the four corners.
const MAX_SWEEP_ANGLES: usize = 6;

/// The four corners of a rectangle, as angles clockwise from twelve o'clock, in `0..TAU`.
///
/// Where a radial sweep has to put a vertex, because they are the only places the border
/// changes direction. Returned in no particular order — the caller sorts whichever of them
/// land inside the sweep.
fn corner_angles(half_width: f32, half_height: f32) -> [f32; 4] {
    // `atan2(dx, -dy)` puts zero straight up and grows clockwise, matching the sweep.
    let angle = |dx: f32, dy: f32| {
        let raw = f32::atan2(dx, -dy);
        if raw < 0.0 {
            raw + std::f32::consts::TAU
        } else {
            raw
        }
    };
    [
        angle(half_width, -half_height),
        angle(half_width, half_height),
        angle(-half_width, half_height),
        angle(-half_width, -half_height),
    ]
}

/// Vertices per quad: two triangles, no index buffer.
///
/// An index buffer would save a third of the vertex bytes and cost a second buffer to
/// allocate, upload and bind. At the sizes this pass reaches — tens of thousands of
/// vertices — the copy is not what costs, so the simpler shape wins.
pub const VERTICES_PER_QUAD: usize = 6;

/// One UI vertex: a pixel position, a texture coordinate and a colour.
///
/// `repr(C)` is required — the input layout describes this exact memory layout to the GPU,
/// so Rust must not be free to reorder the fields. 32 bytes, two per cache line.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiVertex {
    /// Position in pixels, origin at the client area's top-left.
    pub position: [f32; 2],
    /// Where to sample the UI atlas.
    pub uv: [f32; 2],
    /// Linear RGBA tint. Alpha is honoured.
    pub color: [f32; 4],
}

/// The constant buffer the UI shaders read.
///
/// The multiply that takes pixels to normalized device coordinates, and where the atlas's
/// icon band begins. Computing the first once per frame on the CPU means the shader does no
/// divide, and a resize changes two floats rather than every vertex.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UiFrameConstants {
    /// `2 / width` and `-2 / height`.
    pub pixel_to_clip: [f32; 2],
    /// The `v` at or below which the atlas is the font band. See [`UiAtlas::icon_band_v`].
    ///
    /// The pixel shader picks its sampler by comparing against this rather than by reading a
    /// per-vertex flag, which is what keeps [`UiVertex`] at 32 bytes. It travels in what was
    /// already padding, so it costs nothing at all.
    pub icon_band_v: f32,
    /// Padding to the 16-byte constant-buffer row.
    pub padding: f32,
}

impl UiFrameConstants {
    /// The constants for a viewport of this size.
    ///
    /// A zero dimension produces a zero scale rather than an infinity: a minimized window
    /// reports one, and a NaN in a vertex position takes out the whole draw.
    #[must_use]
    pub fn for_viewport(width: u32, height: u32) -> Self {
        let x = if width == 0 { 0.0 } else { 2.0 / width as f32 };
        let y = if height == 0 {
            0.0
        } else {
            -2.0 / height as f32
        };
        Self {
            pixel_to_clip: [x, y],
            icon_band_v: UiAtlas::icon_band_v(),
            padding: 0.0,
        }
    }
}

/// What the last batch produced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UiBatchStats {
    /// Commands consumed.
    pub commands: usize,
    /// Quads emitted.
    pub quads: usize,
    /// Vertices emitted.
    pub vertices: usize,
    /// Glyphs emitted, of which spaces produce none.
    pub glyphs: usize,
    /// Icons emitted.
    pub icons: usize,
    /// Quads dropped because the vertex budget was reached.
    pub dropped: usize,
}

/// Turns a [`UiDrawList`] into one vertex buffer.
///
/// Owns its scratch, which is cleared and never freed, so batching a frame allocates
/// nothing once the peak has been seen.
#[derive(Debug)]
pub struct UiBatcher {
    vertices: Vec<UiVertex>,
    max_quads: usize,
    stats: UiBatchStats,
    /// One output slot per worker, kept across frames so a parallel build allocates
    /// nothing once the peak has been seen — the same reason `vertices` is kept.
    chunks: Vec<UiChunk>,
}

/// One worker's share of a build: the vertices it produced and what it produced them from.
///
/// Separate slots rather than one shared buffer because that is what makes the pass
/// parallel at all; they are concatenated in command order afterwards, so the merged buffer
/// is the one the serial path would have written.
#[derive(Debug, Default)]
struct UiChunk {
    vertices: Vec<UiVertex>,
    stats: UiBatchStats,
}

impl UiBatcher {
    /// Creates a batcher that will emit at most `max_quads` quads per frame.
    #[must_use]
    pub fn new(max_quads: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(max_quads * VERTICES_PER_QUAD),
            max_quads,
            stats: UiBatchStats::default(),
            chunks: Vec::new(),
        }
    }

    /// The vertices the last [`UiBatcher::build`] produced.
    #[must_use]
    pub fn vertices(&self) -> &[UiVertex] {
        &self.vertices
    }

    /// What the last build produced.
    #[must_use]
    pub fn stats(&self) -> UiBatchStats {
        self.stats
    }

    /// The most quads a frame may emit.
    #[must_use]
    pub fn max_quads(&self) -> usize {
        self.max_quads
    }

    /// Expands a draw list into vertices, in paint order.
    ///
    /// Returns the vertices produced. Quads past the budget are counted in
    /// [`UiBatchStats::dropped`] rather than growing the buffer, so a runaway UI degrades
    /// visibly instead of allocating without bound in the frame path.
    pub fn build(&mut self, list: &UiDrawList) -> &[UiVertex] {
        self.build_with(list, None)
    }

    /// [`UiBatcher::build`], optionally expanded across `jobs`' workers.
    ///
    /// `None` expands on the calling thread. `Some` splits the command list into contiguous
    /// runs, expands each into its own buffer and concatenates them **in command order**, so
    /// the vertex buffer is byte-identical to the serial one — paint order is what a user
    /// interface is, and a parallel build that reordered it would be a rendering change
    /// dressed as an optimization.
    ///
    /// Stays serial below [`MIN_TOTAL_UNITS`] of work, or when the split would give a worker
    /// less than [`MIN_CHUNK_UNITS`].
    pub fn build_with(&mut self, list: &UiDrawList, jobs: Option<&JobSystem>) -> &[UiVertex] {
        let chunk_count = jobs.map_or(1, |jobs| {
            chunk_count_for(work_units(list), jobs.worker_count())
        });
        // Bound together: a chunk count above one is only ever produced from a job system,
        // and taking both out of the same `if let` is what says so without an `expect`.
        if let Some(jobs) = jobs.filter(|_| chunk_count > 1) {
            let commands = list.commands();
            let chunk_size = commands.len().div_ceil(chunk_count);
            if self.chunks.len() < chunk_count {
                self.chunks.resize_with(chunk_count, UiChunk::default);
            }
            let max_quads = self.max_quads;
            let active = &mut self.chunks[..chunk_count];
            let result = jobs.map_chunks(active, |index, out| {
                let start = index * chunk_size;
                let end = (start + chunk_size).min(commands.len());
                build_range(list, &commands[start..end], max_quads, out);
            });
            match result {
                Ok(()) => {
                    self.merge(chunk_count);
                    return &self.vertices;
                }
                // A half-expanded interface would draw half a HUD, so the frame is expanded
                // again serially rather than submitted as it stands.
                Err(error) => {
                    log::error!("parallel UI build failed, falling back to serial: {error}");
                }
            }
        }

        {
            // One chunk is the serial path, and it is the *same* path: the whole list goes
            // through `build_range` and then through the same merge, so "serial" and
            // "parallel with one worker" cannot produce different pixels.
            if self.chunks.is_empty() {
                self.chunks.push(UiChunk::default());
            }
            let mut chunk = std::mem::take(&mut self.chunks[0]);
            build_range(list, list.commands(), self.max_quads, &mut chunk);
            self.chunks[0] = chunk;
            self.merge(1);
        }
        &self.vertices
    }

    /// Concatenates the first `chunk_count` chunks in command order, enforcing the budget.
    ///
    /// The budget is applied here rather than per chunk because "the frame's first
    /// `max_quads` quads survive" is a statement about paint order, and only the merge knows
    /// the order. A chunk that overflowed on its own already counted its own drops.
    fn merge(&mut self, chunk_count: usize) {
        // One chunk is the whole list, and its own budget is the frame's budget, so the
        // merge is the identity: take its buffer rather than copying it. Copying 900 quads
        // out of a scratch buffer and into another one costs several microseconds of a pass
        // that measures tens, which would make the serial path pay for the parallel one.
        if chunk_count == 1 {
            std::mem::swap(&mut self.vertices, &mut self.chunks[0].vertices);
            self.stats = self.chunks[0].stats;
            self.stats.vertices = self.vertices.len();
            return;
        }

        self.vertices.clear();
        let mut stats = UiBatchStats::default();
        for chunk in &self.chunks[..chunk_count] {
            let room = self.max_quads - stats.quads;
            let taken = chunk.stats.quads.min(room);
            self.vertices
                .extend_from_slice(&chunk.vertices[..taken * VERTICES_PER_QUAD]);
            stats.commands += chunk.stats.commands;
            stats.quads += taken;
            // What this chunk itself refused, plus what the merge had no room for.
            stats.dropped += chunk.stats.dropped + (chunk.stats.quads - taken);
            stats.glyphs += chunk.stats.glyphs;
            stats.icons += chunk.stats.icons;
        }
        stats.vertices = self.vertices.len();
        self.stats = stats;
    }
}

/// Appends an untextured rectangle, sampling the atlas's opaque texel.
fn push_solid(
    vertices: &mut Vec<UiVertex>,
    max_quads: usize,
    rect: UiRect,
    color: [f32; 4],
    stats: &mut UiBatchStats,
) {
    if !rect.is_visible() {
        return;
    }
    push_quad(vertices, max_quads, rect, UiAtlas::solid_uv(), color, stats);
}

/// Appends a radial sweep as a triangle fan from the centre, filling `rect`.
///
/// Swept clockwise from twelve o'clock, which is the direction every cooldown in the
/// genre runs. Each triangle is emitted as a degenerate quad so the whole pass stays one
/// buffer and one draw call.
///
/// ## The sweep reaches the edges, not a radius
///
/// This was a circle inscribed in `rect`, which on a square ability slot left the four
/// corners uncovered: a cooling ability kept a bright triangle of art in each corner, and
/// a full sweep never darkened the whole button. A radius large enough to reach the
/// corners would instead bulge a tenth of the slot past every edge and spill into the
/// neighbouring one, because this pass has no clipping.
///
/// So the sweep follows the **rectangle's own border**: every ray runs to wherever it
/// leaves `rect`. A full sweep covers `rect` exactly, with nothing outside it.
///
/// That also makes it cheaper. The border is four straight lines, so the only vertices
/// that matter are the two ends of the sweep and whichever corners fall between them —
/// at most six, against the sixteen segments the arc used to be approximated with. There
/// is no arc left to be smooth, so there is nothing to trade off.
fn push_radial(
    vertices: &mut Vec<UiVertex>,
    max_quads: usize,
    rect: UiRect,
    start: f32,
    fraction: f32,
    color: [f32; 4],
    stats: &mut UiBatchStats,
) {
    if !rect.is_visible() || fraction <= 0.0 {
        return;
    }
    let fraction = fraction.min(1.0);
    let centre = (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5);
    let (half_width, half_height) = (rect.width * 0.5, rect.height * 0.5);
    let sweep = fraction * std::f32::consts::TAU;
    let origin = start * std::f32::consts::TAU;

    // Where the ray at `angle` leaves the rectangle. Zero is straight up and the sweep
    // runs clockwise, so the direction is `(sin, -cos)` — screen y grows downward.
    let edge = |angle: f32| {
        let (dx, dy) = (angle.sin(), -angle.cos());
        // How far along the ray the left/right edges are, and how far the top/bottom
        // ones are; the nearer is the edge actually crossed. A zero component never
        // limits, and dividing by it would produce the infinity that says exactly that.
        let to_side = half_width / dx.abs();
        let to_cap = half_height / dy.abs();
        let distance = to_side.min(to_cap);
        (centre.0 + dx * distance, centre.1 + dy * distance)
    };

    // The sweep's two ends, plus every corner strictly between them. A fixed array
    // rather than a `Vec`: four corners and two ends is the most there can ever be, and
    // this runs once per cooling ability per frame.
    let mut angles = [origin; MAX_SWEEP_ANGLES];
    let mut count = 1;
    for corner in corner_angles(half_width, half_height) {
        // Lift the corner into the turn the sweep is in. A corner exactly at the start
        // is skipped: it is already the first vertex.
        let mut angle = corner + (origin / std::f32::consts::TAU).floor() * std::f32::consts::TAU;
        while angle <= origin {
            angle += std::f32::consts::TAU;
        }
        if angle < origin + sweep {
            angles[count] = angle;
            count += 1;
        }
    }
    angles[1..count].sort_by(f32::total_cmp);
    angles[count] = origin + sweep;
    count += 1;

    let uv = UiAtlas::solid_uv();
    let vertex = |position: (f32, f32)| UiVertex {
        position: [position.0, position.1],
        uv: [uv.u0, uv.v0],
        color,
    };
    let hub = vertex(centre);

    for pair in angles[..count].windows(2) {
        // Counted per segment rather than once for the whole abandoned sweep. A sweep is a
        // run of quads like any other, and a drop count that depends on *where* in the list
        // the budget ran out is a count two builds of the same list can disagree on — which
        // is exactly what the parallel merge, deciding the budget after the fact, would.
        if stats.quads >= max_quads {
            stats.dropped += 1;
            continue;
        }
        let b = vertex(edge(pair[0]));
        let c = vertex(edge(pair[1]));
        // Six vertices per triangle, like a quad, so the buffer stays uniform.
        vertices.extend_from_slice(&[hub, b, c, hub, b, c]);
        stats.quads += 1;
    }
}

/// Appends one quad as two triangles.
fn push_quad(
    vertices: &mut Vec<UiVertex>,
    max_quads: usize,
    rect: UiRect,
    uv: AtlasUv,
    color: [f32; 4],
    stats: &mut UiBatchStats,
) {
    if stats.quads >= max_quads {
        stats.dropped += 1;
        return;
    }

    let (left, top) = (rect.x, rect.y);
    let (right, bottom) = (rect.right(), rect.bottom());
    let corners = [
        UiVertex {
            position: [left, top],
            uv: [uv.u0, uv.v0],
            color,
        },
        UiVertex {
            position: [right, top],
            uv: [uv.u1, uv.v0],
            color,
        },
        UiVertex {
            position: [left, bottom],
            uv: [uv.u0, uv.v1],
            color,
        },
        UiVertex {
            position: [right, bottom],
            uv: [uv.u1, uv.v1],
            color,
        },
    ];
    // Two triangles wound so they survive the rasterizer's front-face rule. Culling is
    // off for this pass, so the winding is documentation rather than load-bearing.
    vertices.extend_from_slice(&[
        corners[0], corners[2], corners[1], corners[1], corners[2], corners[3],
    ]);
    stats.quads += 1;
}

/// Expands one contiguous run of commands into `out`.
///
/// The body every path shares: the serial build calls it once over the whole list and a
/// parallel build calls it once per chunk. Two expansions that could drift would be two
/// interfaces, so there is only ever this one.
///
/// `max_quads` bounds this chunk alone. That is the same bound the merged buffer has, so a
/// chunk large enough to hit it is a chunk that fills the frame's whole budget by itself —
/// exactly what the serial path would have done at the same point in paint order.
fn build_range(list: &UiDrawList, commands: &[UiCommand], max_quads: usize, out: &mut UiChunk) {
    out.vertices.clear();
    out.stats = UiBatchStats {
        commands: commands.len(),
        ..UiBatchStats::default()
    };
    for command in commands {
        match *command {
            UiCommand::Rect { rect, color } => {
                push_solid(&mut out.vertices, max_quads, rect, color, &mut out.stats);
            }
            UiCommand::RadialFill {
                rect,
                start,
                fraction,
                color,
            } => {
                push_radial(
                    &mut out.vertices,
                    max_quads,
                    rect,
                    start,
                    fraction,
                    color,
                    &mut out.stats,
                );
            }
            UiCommand::Outline {
                rect,
                thickness,
                color,
            } => {
                // Four bars rather than a stroked rectangle: the top and bottom span the
                // full width and the sides fill what is left, so the corners are covered
                // exactly once. Overlapping them would double the alpha at each corner,
                // which is visible the moment the outline is translucent.
                let inner_height = (rect.height - thickness * 2.0).max(0.0);
                push_solid(
                    &mut out.vertices,
                    max_quads,
                    UiRect::new(rect.x, rect.y, rect.width, thickness),
                    color,
                    &mut out.stats,
                );
                push_solid(
                    &mut out.vertices,
                    max_quads,
                    UiRect::new(rect.x, rect.bottom() - thickness, rect.width, thickness),
                    color,
                    &mut out.stats,
                );
                push_solid(
                    &mut out.vertices,
                    max_quads,
                    UiRect::new(rect.x, rect.y + thickness, thickness, inner_height),
                    color,
                    &mut out.stats,
                );
                push_solid(
                    &mut out.vertices,
                    max_quads,
                    UiRect::new(
                        rect.right() - thickness,
                        rect.y + thickness,
                        thickness,
                        inner_height,
                    ),
                    color,
                    &mut out.stats,
                );
            }
            UiCommand::Icon { rect, icon, tint } => {
                // One quad, like a glyph. What makes it an icon rather than a letter is
                // only where in the atlas it reads from, which is also what tells the
                // pixel shader to filter it smoothly.
                //
                // The rectangle's own width is the size the icon is about to occupy, so
                // the atlas can pick the size variant nearest it. Nothing upstream has
                // to know that variants exist — the layout asks for an icon in a box.
                push_quad(
                    &mut out.vertices,
                    max_quads,
                    rect,
                    UiAtlas::icon_uv(icon, rect.width),
                    tint,
                    &mut out.stats,
                );
                out.stats.icons += 1;
            }
            UiCommand::Text {
                origin,
                range,
                color,
                scale,
                align,
            } => {
                let text = list.text_of(range);
                let width = text_width(text, scale);
                let x = match align {
                    TextAlign::Left => origin.0,
                    TextAlign::Center => origin.0 - width * 0.5,
                    TextAlign::Right => origin.0 - width,
                };
                let mut pen = x;
                for character in text.chars() {
                    if !UiAtlas::is_blank(character) {
                        let rect = UiRect::new(
                            pen,
                            origin.1,
                            GLYPH_WIDTH as f32 * scale,
                            GLYPH_HEIGHT as f32 * scale,
                        );
                        push_quad(
                            &mut out.vertices,
                            max_quads,
                            rect,
                            UiAtlas::glyph_uv(character),
                            color,
                            &mut out.stats,
                        );
                        out.stats.glyphs += 1;
                    }
                    pen += GLYPH_ADVANCE as f32 * scale;
                }
            }
        }
    }

    out.stats.vertices = out.vertices.len();
}

/// Smallest amount of work worth handing to a worker, in the units [`work_units`] counts.
///
/// `docs/threading.md` measured the job break-even near 12 us, and `ui_bench` measures the
/// combat HUD at ~46 ns per unit — 905 quads in 42 us. Five hundred units is ~24 us: over
/// the line with margin, because a unit is an estimate and a chunk that lands under it
/// costs more to submit than to run.
const MIN_CHUNK_UNITS: usize = 512;

/// Smallest whole build worth splitting at all.
///
/// Two thousand units is ~95 us. Below it the submission overhead is a visible share of the
/// pass however it is divided, so the whole build stays on the calling thread. The combat
/// HUD measures 13-42 us, which is why this pass is serial in the game today and parallel
/// only when the interface grows several times over.
const MIN_TOTAL_UNITS: usize = 2048;

/// How much work a draw list is, in units of roughly one quad.
///
/// One per command plus one per byte of text, because text is the term that dominates: a
/// glyph is a quad and a nameplate is a few hundred of them, while a rectangle is one.
/// Bytes rather than characters — the strings are ASCII and `len()` does not walk them.
#[must_use]
fn work_units(list: &UiDrawList) -> usize {
    list.len() + list.text().len()
}

/// How many chunks `units` of work should be split into across `workers`.
///
/// Never more than one chunk per worker, never smaller than [`MIN_CHUNK_UNITS`], and one —
/// the caller's signal to stay serial — below [`MIN_TOTAL_UNITS`].
#[must_use]
fn chunk_count_for(units: usize, workers: usize) -> usize {
    if workers <= 1 || units < MIN_TOTAL_UNITS {
        return 1;
    }
    (units / MIN_CHUNK_UNITS).min(workers).max(1)
}

/// How wide a run of text is, in pixels.
///
/// The last glyph contributes its width rather than its advance, so a centred string is
/// centred on the ink rather than on the gap after it.
#[must_use]
pub fn text_width(text: &str, scale: f32) -> f32 {
    let count = text.chars().count();
    if count == 0 {
        return 0.0;
    }
    ((count - 1) * GLYPH_ADVANCE + GLYPH_WIDTH) as f32 * scale
}

/// How tall one line of text is, in pixels.
#[must_use]
pub fn text_height(scale: f32) -> f32 {
    GLYPH_HEIGHT as f32 * scale
}

/// Root signature and pipeline state for the UI pass.
#[derive(Debug)]
pub struct UiPipeline {
    root_signature: ID3D12RootSignature,
    state: ID3D12PipelineState,
}

impl UiPipeline {
    /// Creates the UI pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if the root signature or pipeline state is rejected.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;
        let elements = input_elements();
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            // SAFETY: reinterprets the COM smart pointer as the raw interface pointer the
            // descriptor field expects, without changing the reference count. The signature
            // is kept alive by the local binding for the whole of the creation call below,
            // and by `Self` afterwards.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(UI_VS),
            PS: bytecode(UI_PS),
            BlendState: alpha_blend_desc(),
            SampleMask: u32::MAX,
            RasterizerState: ui_rasterizer_desc(),
            DepthStencilState: ui_depth_stencil_desc(),
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: elements.as_ptr(),
                NumElements: elements.len() as u32,
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let mut desc = desc;
        desc.RTVFormats[0] = BACK_BUFFER_FORMAT;
        // No depth-stencil format: this pass neither tests nor writes depth, and declaring
        // one it never uses would tie the UI to the scene's depth buffer for nothing.

        // SAFETY: FFI call with a fully initialized descriptor whose borrowed root
        // signature, bytecode and input elements all outlive it.
        let state: ID3D12PipelineState = unsafe {
            {
                let probe = crate::upload_timing::Probe::start();
                let state = device.CreateGraphicsPipelineState(&desc);
                probe.pipeline();
                state
            }
        }
        .map_err(|error| RendererError::api("CreateGraphicsPipelineState (ui)", error))?;

        Ok(Self {
            root_signature,
            state,
        })
    }

    /// The root signature.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state.
    #[must_use]
    pub fn state(&self) -> &ID3D12PipelineState {
        &self.state
    }
}

/// The UI vertex layout, matching [`UiVertex`] exactly.
#[must_use]
pub fn input_elements() -> [D3D12_INPUT_ELEMENT_DESC; 3] {
    [
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("TEXCOORD"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 8,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D12_INPUT_ELEMENT_DESC {
            SemanticName: s!("COLOR"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 16,
            InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ]
}

/// Straight alpha blending: `src * a + dst * (1 - a)`.
///
/// Alpha is written as `a + dst * (1 - a)` rather than replaced, so a translucent element
/// drawn over another accumulates coverage instead of punching a hole in it. That only
/// matters if the UI is ever composited rather than drawn straight to the back buffer, and
/// getting it right costs nothing today.
#[must_use]
pub fn alpha_blend_desc() -> D3D12_BLEND_DESC {
    let target = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_SRC_ALPHA,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    D3D12_BLEND_DESC {
        RenderTarget: [target; 8],
        ..Default::default()
    }
}

/// Solid fill, no culling: a UI quad has no meaningful facing.
fn ui_rasterizer_desc() -> D3D12_RASTERIZER_DESC {
    D3D12_RASTERIZER_DESC {
        FillMode: D3D12_FILL_MODE_SOLID,
        CullMode: D3D12_CULL_MODE_NONE,
        FrontCounterClockwise: true.into(),
        DepthClipEnable: true.into(),
        ..Default::default()
    }
}

/// No depth test, no depth write: the UI is painted in list order.
fn ui_depth_stencil_desc() -> D3D12_DEPTH_STENCIL_DESC {
    D3D12_DEPTH_STENCIL_DESC {
        DepthEnable: false.into(),
        DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO,
        DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        StencilEnable: false.into(),
        ..Default::default()
    }
}

/// Two root parameters and two static samplers.
///
/// Both samplers are **static** rather than descriptors: neither ever changes, and a static
/// sampler costs no descriptor heap space and no bind.
///
/// There are two because the atlas holds two kinds of picture. `s0` is **point** filtering
/// for the font band: the glyphs are a crisp 1-bit bitmap magnified to integer scales, and
/// bilinear sampling of one produces grey fringes around letters designed to have none.
/// `s1` is **linear** for the icon band: an icon is rasterized at 88 texels and drawn at 44,
/// and point sampling a minification like that drops every other texel into aliasing.
///
/// The shader chooses between them from the texture coordinate, so nothing on the vertex or
/// in the pipeline has to say which is which.
fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, RendererError> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];

    let sampler = |filter, register| D3D12_STATIC_SAMPLER_DESC {
        Filter: filter,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        MinLOD: 0.0,
        MaxLOD: 0.0,
        ShaderRegister: register,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let samplers = [
        sampler(D3D12_FILTER_MIN_MAG_MIP_POINT, SAMPLER_GLYPH),
        sampler(D3D12_FILTER_MIN_MAG_MIP_LINEAR, SAMPLER_ICON),
    ];

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
    };

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: FFI call. `desc`, the parameter array, the range and the sampler all outlive
    // the call, and both out-parameters are valid.
    let result = unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        return Err(RendererError::api(
            "D3D12SerializeRootSignature (ui)",
            error,
        ));
    }
    let Some(blob) = blob else {
        return Err(RendererError::InvalidConfig {
            reason: "the UI root signature serialized to no blob".to_owned(),
        });
    };

    // SAFETY: FFI calls reading the serialized blob's buffer, which stays alive for the
    // duration of the call.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    // SAFETY: FFI call. `bytes` points into the live blob and node mask 0 targets the
    // single-adapter case. The return type is fixed by this function's signature, which is
    // what selects the COM interface the generic call produces.
    unsafe { device.CreateRootSignature(0, bytes) }
        .map_err(|error| RendererError::api("CreateRootSignature (ui)", error))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_render_data::ui::{UiDrawList, UiIcon};

    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

    fn batcher() -> UiBatcher {
        UiBatcher::new(4096)
    }

    #[test]
    fn the_vertex_layout_matches_the_input_elements() {
        // The GPU is told this exact memory layout; a reordered field renders garbage
        // rather than failing.
        assert_eq!(size_of::<UiVertex>(), 32, "2 + 2 + 4 floats, packed");
        assert_eq!(align_of::<UiVertex>(), 4);
        let elements = input_elements();
        assert_eq!(elements[0].AlignedByteOffset, 0);
        assert_eq!(elements[1].AlignedByteOffset, 8);
        assert_eq!(elements[2].AlignedByteOffset, 16);
    }

    #[test]
    fn a_rectangle_becomes_exactly_one_quad() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.rect(UiRect::new(10.0, 20.0, 100.0, 8.0), WHITE);

        let vertices = batcher.build(&list).to_vec();
        assert_eq!(batcher.stats().quads, 1);
        assert_eq!(vertices.len(), VERTICES_PER_QUAD);
        // The corners span the rectangle exactly.
        let xs: Vec<f32> = vertices.iter().map(|vertex| vertex.position[0]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 10.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 110.0);
    }

    #[test]
    fn a_character_is_one_quad_rather_than_one_per_lit_cell() {
        // The entire point of the phase, as an assertion. Phase 3 emitted roughly fifteen
        // quads for this; one is the target.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "A", WHITE, 1.0);

        batcher.build(&list);
        assert_eq!(batcher.stats().quads, 1);
        assert_eq!(batcher.stats().glyphs, 1);
    }

    #[test]
    fn a_line_of_text_costs_one_quad_per_visible_character() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "DUMMY 100%", WHITE, 1.0);

        batcher.build(&list);
        // Ten characters, one of which is a space and draws nothing.
        assert_eq!(batcher.stats().glyphs, 9);
        assert_eq!(batcher.stats().quads, 9);
        assert_eq!(batcher.stats().vertices, 9 * VERTICES_PER_QUAD);
    }

    #[test]
    fn a_space_advances_the_pen_without_emitting_geometry() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "A B", WHITE, 1.0);
        let vertices = batcher.build(&list).to_vec();

        assert_eq!(batcher.stats().quads, 2);
        let first = vertices[0].position[0];
        let second = vertices[VERTICES_PER_QUAD].position[0];
        assert_eq!(
            second - first,
            (GLYPH_ADVANCE * 2) as f32,
            "the space still moved the pen"
        );
    }

    #[test]
    fn an_outline_is_four_bars_that_meet_at_the_corners_without_overlapping() {
        // Overlapping corners double the alpha, which shows the moment an outline is
        // translucent.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.outline(UiRect::new(0.0, 0.0, 20.0, 10.0), 2.0, WHITE);
        batcher.build(&list);

        assert_eq!(batcher.stats().quads, 4);
        let vertices = batcher.vertices();
        // The vertical bars start below the top bar and end above the bottom one.
        let left_bar_top = vertices[2 * VERTICES_PER_QUAD].position[1];
        assert_eq!(left_bar_top, 2.0);
    }

    #[test]
    fn text_and_rectangles_share_one_batch() {
        // They sample the same atlas through the same pipeline, so the whole HUD is one
        // draw. If a rectangle ever needed a different texture this would be two.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.bar(UiRect::new(0.0, 0.0, 100.0, 10.0), 0.5, WHITE, WHITE);
        list.text_at((0.0, 0.0), "HP", WHITE, 1.0);

        let vertices = batcher.build(&list).to_vec();
        assert_eq!(batcher.stats().quads, 4, "two bar rects plus two glyphs");
        assert_eq!(vertices.len(), 4 * VERTICES_PER_QUAD);

        // The solid quads sample a point; the glyphs sample a region. One texture, one draw.
        let solid = UiAtlas::solid_uv();
        assert_eq!(vertices[0].uv, [solid.u0, solid.v0]);
        let glyph = UiAtlas::glyph_uv('H');
        assert_eq!(vertices[2 * VERTICES_PER_QUAD].uv, [glyph.u0, glyph.v0]);
    }

    #[test]
    fn a_full_radial_sweep_covers_its_whole_cell_including_the_corners() {
        // The bug this replaced: the sweep was a circle inscribed in the slot, so a cooling
        // ability kept a bright triangle of art in each corner and a full cooldown never
        // dimmed the whole button.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        let rect = UiRect::new(10.0, 20.0, 48.0, 48.0);
        list.radial(rect, 1.0, WHITE);
        let vertices = batcher.build(&list).to_vec();

        // Every corner of the cell is a vertex of the fan, so the geometry reaches them
        // rather than cutting across.
        for corner in [
            [rect.x, rect.y],
            [rect.right(), rect.y],
            [rect.x, rect.bottom()],
            [rect.right(), rect.bottom()],
        ] {
            assert!(
                vertices.iter().any(|vertex| {
                    (vertex.position[0] - corner[0]).abs() < 0.01
                        && (vertex.position[1] - corner[1]).abs() < 0.01
                }),
                "{corner:?} is not covered by the sweep"
            );
        }
    }

    #[test]
    fn a_radial_sweep_never_reaches_outside_its_cell() {
        // The other half of the same property. Reaching the corners with a circle would mean
        // a radius that bulges a tenth of the slot past every edge, and this pass has no
        // clipping — it would spill onto the neighbouring ability.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        // A non-square cell too: the sweep follows the border, not a radius.
        for rect in [
            UiRect::new(10.0, 20.0, 48.0, 48.0),
            UiRect::new(0.0, 0.0, 200.0, 40.0),
        ] {
            for start in [0.0, 0.1, 0.375, 0.6, 0.9] {
                list.clear();
                list.radial_from(rect, start, 1.0 - start, WHITE);
                for vertex in batcher.build(&list) {
                    let [x, y] = vertex.position;
                    assert!(
                        x >= rect.x - 0.01 && x <= rect.right() + 0.01,
                        "{x} escapes {rect:?} horizontally at start {start}"
                    );
                    assert!(
                        y >= rect.y - 0.01 && y <= rect.bottom() + 0.01,
                        "{y} escapes {rect:?} vertically at start {start}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_radial_sweep_costs_a_triangle_per_corner_it_crosses_and_no_more() {
        // The border is four straight lines, so there is no arc to approximate and nothing
        // to gain from more geometry. A full turn crosses all four corners: five triangles.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        let rect = UiRect::new(0.0, 0.0, 48.0, 48.0);

        list.radial(rect, 1.0, WHITE);
        batcher.build(&list);
        assert_eq!(batcher.stats().quads, 5, "a full sweep");

        // An eighth of a turn from twelve o'clock reaches the first corner exactly, so it is
        // a single triangle.
        list.clear();
        list.radial(rect, 0.125, WHITE);
        batcher.build(&list);
        assert_eq!(
            batcher.stats().quads,
            1,
            "twelve o'clock to the first corner"
        );

        // And a sweep that stops short of any corner is still one triangle.
        list.clear();
        list.radial(rect, 0.05, WHITE);
        batcher.build(&list);
        assert_eq!(batcher.stats().quads, 1, "a sliver");
    }

    #[test]
    fn an_icon_is_one_quad_that_shares_the_batch_with_text_and_rectangles() {
        // The point of putting icons in the font's atlas: art on the ability bar does not
        // cost a second texture, a second pipeline state or a second draw.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        let rect = UiRect::new(10.0, 20.0, 44.0, 44.0);
        list.rect(rect, WHITE);
        list.icon(rect, UiIcon::Meteor, WHITE);
        list.text_at((0.0, 0.0), "M", WHITE, 1.0);

        let vertices = batcher.build(&list).to_vec();
        assert_eq!(batcher.stats().quads, 3);
        assert_eq!(batcher.stats().icons, 1);
        assert_eq!(vertices.len(), 3 * VERTICES_PER_QUAD);

        // The icon quad spans the rectangle exactly and reads the cell for the size it is
        // actually drawn at — the rectangle's own width, which is what selects the variant.
        let icon = &vertices[VERTICES_PER_QUAD..VERTICES_PER_QUAD * 2];
        assert_eq!(icon[0].position, [10.0, 20.0]);
        let uv = UiAtlas::icon_uv(UiIcon::Meteor, rect.width);
        assert_eq!(icon[0].uv, [uv.u0, uv.v0]);
    }

    #[test]
    fn the_same_icon_drawn_larger_reads_a_different_cell() {
        // Variant selection is invisible above the batcher: the layout asks for an icon in
        // a box, and the size of the box is what decides which rasterization is sampled.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.icon(UiRect::new(0.0, 0.0, 40.0, 40.0), UiIcon::Firebolt, WHITE);
        let small = batcher.build(&list)[0].uv;

        list.clear();
        list.icon(UiRect::new(0.0, 0.0, 130.0, 130.0), UiIcon::Firebolt, WHITE);
        let large = batcher.build(&list)[0].uv;

        assert_ne!(small, large, "both sizes read the same cell");
    }

    #[test]
    fn an_icon_reads_below_the_band_boundary_and_a_glyph_reads_above_it() {
        // The pixel shader picks its sampler by comparing `v` against the boundary in the
        // frame constants. If a batched quad ever landed on the wrong side of it, an icon
        // would be point-sampled or a glyph would be blurred, with nothing failing.
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "AZ09", WHITE, 2.0);
        list.rect(UiRect::new(0.0, 0.0, 10.0, 10.0), WHITE);
        let glyphs_and_rects = batcher.build(&list).to_vec();

        let boundary = UiFrameConstants::for_viewport(1280, 720).icon_band_v;
        assert!(boundary > 0.0 && boundary < 1.0, "{boundary}");
        for vertex in &glyphs_and_rects {
            assert!(
                vertex.uv[1] < boundary,
                "a glyph or rectangle reads from the icon band: {:?}",
                vertex.uv
            );
        }

        list.clear();
        for icon in UiIcon::ALL {
            list.icon(UiRect::new(0.0, 0.0, 44.0, 44.0), icon, WHITE);
        }
        for vertex in batcher.build(&list) {
            assert!(
                vertex.uv[1] >= boundary,
                "an icon reads from the font band: {:?}",
                vertex.uv
            );
        }
    }

    #[test]
    fn centred_text_is_centred_on_its_ink() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_aligned((100.0, 0.0), "ABC", WHITE, 1.0, TextAlign::Center);
        batcher.build(&list);

        let width = text_width("ABC", 1.0);
        let left = batcher.vertices()[0].position[0];
        assert_eq!(left, 100.0 - width * 0.5);

        // Right-aligned text ends at the origin.
        list.clear();
        list.text_aligned((100.0, 0.0), "ABC", WHITE, 1.0, TextAlign::Right);
        batcher.build(&list);
        assert_eq!(batcher.vertices()[0].position[0], 100.0 - width);
    }

    #[test]
    fn scale_multiplies_both_the_glyph_and_the_advance() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        list.text_at((0.0, 0.0), "AB", WHITE, 2.0);
        batcher.build(&list);

        let vertices = batcher.vertices();
        // Winding is top-left, bottom-left, top-right, so index 2 is the far corner.
        let first_width = vertices[2].position[0] - vertices[0].position[0];
        assert_eq!(first_width, GLYPH_WIDTH as f32 * 2.0);
        let advance = vertices[VERTICES_PER_QUAD].position[0] - vertices[0].position[0];
        assert_eq!(advance, GLYPH_ADVANCE as f32 * 2.0);
    }

    #[test]
    fn the_quad_budget_drops_work_rather_than_growing_the_buffer_mid_frame() {
        // A runaway UI must degrade visibly, not allocate without bound in the frame path.
        let mut batcher = UiBatcher::new(4);
        let mut list = UiDrawList::new();
        for index in 0..10 {
            list.rect(UiRect::new(index as f32, 0.0, 2.0, 2.0), WHITE);
        }
        batcher.build(&list);

        assert_eq!(batcher.stats().quads, 4);
        assert_eq!(batcher.stats().dropped, 6);
        assert_eq!(batcher.vertices().len(), 4 * VERTICES_PER_QUAD);
    }

    #[test]
    fn rebuilding_reuses_the_vertex_allocation() {
        let mut batcher = batcher();
        let mut list = UiDrawList::new();
        for index in 0..200 {
            list.rect(UiRect::new(index as f32, 0.0, 2.0, 2.0), WHITE);
            list.text_at((0.0, index as f32), "DAMAGE 100", WHITE, 1.0);
        }
        batcher.build(&list);
        let capacity = batcher.vertices.capacity();

        for _ in 0..500 {
            batcher.build(&list);
        }
        assert_eq!(batcher.vertices.capacity(), capacity);
    }

    #[test]
    fn a_zero_sized_viewport_produces_a_zero_scale_rather_than_an_infinity() {
        // A minimized window reports one, and a NaN vertex position takes out the draw.
        let constants = UiFrameConstants::for_viewport(0, 0);
        assert_eq!(constants.pixel_to_clip, [0.0, 0.0]);

        let constants = UiFrameConstants::for_viewport(1920, 1080);
        assert!((constants.pixel_to_clip[0] - 2.0 / 1920.0).abs() < 1e-9);
        assert!((constants.pixel_to_clip[1] + 2.0 / 1080.0).abs() < 1e-9);
    }

    #[test]
    fn the_ui_pass_neither_tests_nor_writes_depth() {
        // Painter's order is the ordering strategy; a depth test would make a blended
        // fragment vanish and a depth write would occlude what should blend over it.
        let depth = ui_depth_stencil_desc();
        assert!(!depth.DepthEnable.as_bool());
        assert_eq!(depth.DepthWriteMask, D3D12_DEPTH_WRITE_MASK_ZERO);
    }

    #[test]
    fn the_ui_blend_state_is_straight_alpha() {
        let blend = alpha_blend_desc();
        let target = blend.RenderTarget[0];
        assert!(target.BlendEnable.as_bool());
        assert_eq!(target.SrcBlend, D3D12_BLEND_SRC_ALPHA);
        assert_eq!(target.DestBlend, D3D12_BLEND_INV_SRC_ALPHA);
    }

    #[test]
    fn an_empty_list_produces_no_vertices_and_no_draw() {
        let mut batcher = batcher();
        let list = UiDrawList::new();
        assert!(batcher.build(&list).is_empty());
        assert_eq!(batcher.stats().quads, 0);
    }
}
