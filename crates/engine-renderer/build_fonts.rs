// The interface fonts' signed-distance-field atlas, generated at build time.
//
// Included by build.rs. Every glyph of every face is rasterized four times over, turned into a
// distance field by an exact Euclidean distance transform, sampled down, and packed into one
// single-channel atlas. The renderer draws a glyph at any size from it: the pixel shader turns
// the distance into coverage with a one-pixel ramp, so text is crisp from 11 px to 4K.
//
// The fonts and the character list are `engine-render-data`'s; that crate's build script reads
// the same files for the advance widths text is measured with.

use ab_glyph::{Font, FontRef, PxScale, point};

/// Atlas texels per em. Forty-eight: enough resolution that a 60 px heading keeps its serifs.
const SDF_EM: f32 = 48.0;
/// Distance, in atlas texels, the field reaches either side of an outline.
const SDF_RANGE: f32 = 6.0;
/// Supersampling of the coverage raster the field is computed from.
const SDF_SS: usize = 4;
/// Atlas width, in texels.
const FONT_ATLAS_WIDTH: usize = 1024;

/// The faces, in `UiFont` order.
const FONT_FACES: [&str; 4] = [
    "Inter-Regular.ttf",
    "Inter-Medium.ttf",
    "Inter-SemiBold.ttf",
    "Cinzel-SemiBold.ttf",
];

/// Must equal `engine_render_data::font::CHARSET`; `src/text.rs` asserts it.
const FONT_CHARSET: &str = concat!(
    " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`",
    "abcdefghijklmnopqrstuvwxyz{|}~",
    "\u{00B7}\u{2013}\u{2014}\u{2019}\u{00D7}\u{2022}\u{2026}"
);

/// One glyph's cell, before packing.
struct Cell {
    width: usize,
    height: usize,
    texels: Vec<u8>,
    /// Plane bounds in ems relative to the pen on the baseline, y down: left, top, right, bottom.
    plane: [f32; 4],
}

/// One-dimensional squared Euclidean distance transform (Felzenszwalb & Huttenlocher).
fn edt_1d(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let mut s = parabola_crossing(f, q, v[k]);
        // z[0] is minus infinity, so this never walks past the first parabola.
        while s <= z[k] {
            k -= 1;
            s = parabola_crossing(f, q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let delta = q as f32 - v[k] as f32;
        *out = delta * delta + f[v[k]];
    }
}

/// Where the parabolas rooted at `q` and `p` cross.
fn parabola_crossing(f: &[f32], q: usize, p: usize) -> f32 {
    let (qf, pf) = (q as f32, p as f32);
    ((f[q] + qf * qf) - (f[p] + pf * pf)) / (2.0 * qf - 2.0 * pf)
}

/// Squared distance from every pixel to the nearest pixel where `seed` is true.
fn edt_2d(seed: &[bool], width: usize, height: usize) -> Vec<f32> {
    const FAR: f32 = 1e20;
    let mut grid: Vec<f32> = seed.iter().map(|&s| if s { 0.0 } else { FAR }).collect();
    let n = width.max(height);
    let (mut f, mut d, mut v, mut z) = (vec![0.0; n], vec![0.0; n], vec![0usize; n], vec![0.0; n + 1]);
    for x in 0..width {
        for y in 0..height {
            f[y] = grid[y * width + x];
        }
        edt_1d(&f[..height], &mut d[..height], &mut v, &mut z);
        for y in 0..height {
            grid[y * width + x] = d[y];
        }
    }
    for y in 0..height {
        f[..width].copy_from_slice(&grid[y * width..(y + 1) * width]);
        edt_1d(&f[..width], &mut d[..width], &mut v, &mut z);
        grid[y * width..(y + 1) * width].copy_from_slice(&d[..width]);
    }
    grid
}

/// The distance-field cell of one glyph, or `None` for one with no outline (a space).
fn glyph_cell(font: &FontRef<'_>, character: char) -> Option<Cell> {
    let em_units = font.units_per_em().unwrap_or(1000.0);
    let height_units = font.ascent_unscaled() - font.descent_unscaled();
    let hi_em = SDF_EM * SDF_SS as f32;
    let scale = PxScale::from(hi_em * height_units / em_units);
    let glyph = font.glyph_id(character).with_scale_and_position(scale, point(0.0, 0.0));
    let outlined = font.outline_glyph(glyph)?;
    let bounds = outlined.px_bounds();
    let pad = (SDF_RANGE as usize + 1) * SDF_SS;
    let raster_w = bounds.width().ceil() as usize;
    let raster_h = bounds.height().ceil() as usize;
    // Whole output texels, so the cell's size in ems is exact.
    let width = (raster_w + 2 * pad).div_ceil(SDF_SS);
    let height = (raster_h + 2 * pad).div_ceil(SDF_SS);
    let (hw, hh) = (width * SDF_SS, height * SDF_SS);
    let mut coverage = vec![0.0f32; hw * hh];
    outlined.draw(|x, y, c| {
        let (x, y) = (x as usize + pad, y as usize + pad);
        if x < hw && y < hh {
            coverage[y * hw + x] = c;
        }
    });
    let inside: Vec<bool> = coverage.iter().map(|&c| c > 0.5).collect();
    let outside: Vec<bool> = inside.iter().map(|&i| !i).collect();
    let to_inside = edt_2d(&inside, hw, hh);
    let to_outside = edt_2d(&outside, hw, hh);
    let mut texels = vec![0u8; width * height];
    for ty in 0..height {
        for tx in 0..width {
            // The high-resolution pixel under the texel's centre.
            let (hx, hy) = (tx * SDF_SS + SDF_SS / 2, ty * SDF_SS + SDF_SS / 2);
            let index = hy * hw + hx;
            let signed = (to_outside[index].sqrt() - to_inside[index].sqrt()) / SDF_SS as f32;
            let value = (0.5 + signed / (2.0 * SDF_RANGE)).clamp(0.0, 1.0);
            texels[ty * width + tx] = (value * 255.0).round() as u8;
        }
    }
    let left = (bounds.min.x - pad as f32) / hi_em;
    let top = (bounds.min.y - pad as f32) / hi_em;
    Some(Cell {
        width,
        height,
        texels,
        plane: [left, top, left + width as f32 / SDF_EM, top + height as f32 / SDF_EM],
    })
}

/// Rasterizes every face into the atlas and generates `fonts.rs`.
fn build_font_atlas(font_dir: &std::path::Path, out_dir: &std::path::Path) {
    let mut cells: Vec<Vec<Option<Cell>>> = Vec::new();
    for face in FONT_FACES {
        let path = font_dir.join(face);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let font = FontRef::try_from_slice(&bytes).unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        cells.push(FONT_CHARSET.chars().map(|character| glyph_cell(&font, character)).collect());
    }

    // Shelf packing, a texel of gap between cells.
    let mut placements: Vec<Vec<Option<(usize, usize)>>> = Vec::new();
    let (mut x, mut y, mut shelf) = (0usize, 0usize, 0usize);
    for face in &cells {
        let mut row = Vec::new();
        for cell in face {
            let Some(cell) = cell else {
                row.push(None);
                continue;
            };
            if x + cell.width + 1 > FONT_ATLAS_WIDTH {
                x = 0;
                y += shelf + 1;
                shelf = 0;
            }
            row.push(Some((x, y)));
            x += cell.width + 1;
            shelf = shelf.max(cell.height);
        }
        placements.push(row);
    }
    let atlas_height = (y + shelf + 1).next_power_of_two().max(64);
    let mut atlas = vec![0u8; FONT_ATLAS_WIDTH * atlas_height];
    let mut table = String::new();
    for (face, row) in cells.iter().zip(&placements) {
        table.push_str("    [\n");
        for (cell, place) in face.iter().zip(row) {
            match (cell, place) {
                (Some(cell), Some((px, py))) => {
                    for line in 0..cell.height {
                        let to = (py + line) * FONT_ATLAS_WIDTH + px;
                        atlas[to..to + cell.width]
                            .copy_from_slice(&cell.texels[line * cell.width..(line + 1) * cell.width]);
                    }
                    let (w, h) = (FONT_ATLAS_WIDTH as f32, atlas_height as f32);
                    table.push_str(&format!(
                        "        GlyphQuad {{ plane: [{:?}, {:?}, {:?}, {:?}], uv: [{:?}, {:?}, {:?}, {:?}] }},\n",
                        cell.plane[0], cell.plane[1], cell.plane[2], cell.plane[3],
                        *px as f32 / w, *py as f32 / h,
                        (px + cell.width) as f32 / w, (py + cell.height) as f32 / h,
                    ));
                }
                _ => table.push_str("        GlyphQuad::EMPTY,\n"),
            }
        }
        table.push_str("    ],\n");
    }
    let atlas_path = out_dir.join("font_atlas.r8");
    std::fs::write(&atlas_path, &atlas).unwrap_or_else(|error| panic!("failed to write the font atlas: {error}"));
    let generated = format!(
        "// Generated by build.rs from engine-render-data/fonts. Do not edit.\n\
         /// The characters, in glyph order. Must equal `engine_render_data::font::CHARSET`.\n\
         pub const CHARSET: &str = {FONT_CHARSET:?};\n\
         /// Atlas width, texels.\n\
         pub const ATLAS_WIDTH: usize = {FONT_ATLAS_WIDTH};\n\
         /// Atlas height, texels.\n\
         pub const ATLAS_HEIGHT: usize = {atlas_height};\n\
         /// Distance the field reaches either side of an outline, in atlas texels.\n\
         pub const SDF_RANGE: f32 = {SDF_RANGE:?};\n\
         /// One byte per texel: 128 is the outline, more is inside.\n\
         pub static ATLAS: &[u8] = include_bytes!(r\"{}\");\n\
         /// Each glyph's quad, per face.\n\
         pub static QUADS: [[GlyphQuad; {}]; {}] = [\n{table}];\n",
        atlas_path.display(),
        FONT_CHARSET.chars().count(),
        FONT_FACES.len(),
    );
    std::fs::write(out_dir.join("fonts.rs"), generated).unwrap_or_else(|error| panic!("failed to write fonts.rs: {error}"));
}
