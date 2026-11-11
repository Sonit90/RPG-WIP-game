//! The dungeon map's picture: a top-down render of the level, styled into a map, and its file.
//!
//! ```text
//!  top-down capture (RGBA, the real renderer)  ──┐
//!                                                ├──► style ──► parchment map ──► .map.png
//!  WalkableMask (floor / wall / outside)       ──┘
//! ```
//!
//! # Why the render is styled rather than shown
//!
//! A straight top-down render of a crypt is a photograph of wall tops on grass: correct and
//! unreadable at minimap size. The genre's maps are drawings — pale floor, inked walls, dark
//! ground beyond them — and the [`WalkableMask`] says which is which. The render still does
//! the work only it can do: its light and shade are carried into the floor, so a brazier's
//! glow, a pillar's footprint and the edge of a dais are on the map because they are in the
//! level, not because anybody drew them.
//!
//! # Pure CPU and deterministic
//!
//! No device, no clock, no random number generator: the "paper grain" is a hash of the texel
//! coordinate. The same capture and mask style to the same bytes, which is what lets a test
//! assert on them and a re-bake of an unchanged level produce an identical file.
//!
//! # Colour space
//!
//! The capture is read back from a UNORM back buffer, so its bytes are already display values,
//! and the UI draws the map through a *data* view for the same reason the UI atlas is one: the
//! bytes go to the screen as they are. Nothing here converts anything.

use std::path::Path;

use engine_environment::map::{MapCell, WalkableMask};

use crate::error::AssetError;
use crate::texture::{TextureAsset, TextureFormat};

/// Bytes per texel of every image in this module.
pub const BYTES_PER_TEXEL: usize = 4;

/// The map's colours and material scale, as display-space RGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapPalette {
    /// Room floor where the render is brightest: warm limestone.
    pub floor_light: [f32; 3],
    /// Room floor where the render is darkest.
    pub floor_dark: [f32; 3],
    /// Corridor floor: the same stone, cooler and a shade down, so passages read as passages.
    pub corridor: [f32; 3],
    /// The ink along a wall's face.
    pub wall: [f32; 3],
    /// The body of a wall: dressed stone, lit from the north-west.
    pub wall_edge: [f32; 3],
    /// The ground beyond the walls: the interface's own blue-black, so the level floats on it.
    pub outside: [f32; 3],
    /// How much of the render's own colour survives into the floor, `0..1`.
    pub render_share: f32,
    /// A flagstone's size, in texels. Two metres at the map's density reads as paving without
    /// competing with the markers.
    pub tile_texels: f32,
}

impl Default for MapPalette {
    /// Limestone and dressed stone on blue-black: a tactical dungeon map.
    fn default() -> Self {
        Self {
            floor_light: [226.0, 212.0, 182.0],
            floor_dark: [168.0, 148.0, 116.0],
            corridor: [196.0, 188.0, 170.0],
            wall: [26.0, 22.0, 20.0],
            wall_edge: [92.0, 88.0, 84.0],
            outside: [17.0, 19.0, 25.0],
            render_share: 0.16,
            tile_texels: 10.0,
        }
    }
}

/// Styles a top-down `capture` into a map, using `mask` to tell floor from wall from outside.
///
/// `capture` is `width × height` RGBA with row zero north, the same grid the mask was built
/// on. A capture of another size is resampled by nearest texel, so a capture made at the
/// window's resolution and a mask made at the map's still line up.
///
/// # Errors
///
/// [`AssetError::Invalid`] if either input is empty or the capture's length does not match its
/// size.
pub fn style_map(
    capture: &[u8],
    capture_size: (u32, u32),
    mask: &WalkableMask,
    palette: &MapPalette,
) -> Result<Vec<u8>, AssetError> {
    let (cw, ch) = capture_size;
    if cw == 0 || ch == 0 || capture.len() != cw as usize * ch as usize * BYTES_PER_TEXEL {
        return Err(AssetError::invalid(
            "map",
            "capture",
            format!(
                "{cw}x{ch} needs {} bytes, got {}",
                cw as usize * ch as usize * 4,
                capture.len()
            ),
        ));
    }
    if mask.width == 0 || mask.height == 0 {
        return Err(AssetError::invalid(
            "map",
            "mask",
            "the mask is empty".to_owned(),
        ));
    }
    let (w, h) = (mask.width as usize, mask.height as usize);

    // Luminance of the capture, resampled onto the mask's grid.
    let mut luminance = vec![0.0f32; w * h];
    let mut colour = vec![[0.0f32; 3]; w * h];
    for y in 0..h {
        let sy = (y * ch as usize / h).min(ch as usize - 1);
        for x in 0..w {
            let sx = (x * cw as usize / w).min(cw as usize - 1);
            let offset = (sy * cw as usize + sx) * BYTES_PER_TEXEL;
            let rgb = [
                f32::from(capture[offset]),
                f32::from(capture[offset + 1]),
                f32::from(capture[offset + 2]),
            ];
            colour[y * w + x] = rgb;
            luminance[y * w + x] = (0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]) / 255.0;
        }
    }

    // Normalize the floor's brightness so a dark crypt and a sunlit field both span the page.
    // Percentiles rather than the extremes: one brazier's glow must not squash the rest of
    // the floor into a single shade.
    let mut floor_luminance: Vec<f32> = mask
        .cells
        .iter()
        .enumerate()
        .filter(|(_, cell)| **cell == MapCell::Floor)
        .map(|(index, _)| luminance[index])
        .collect();
    floor_luminance.sort_by(f32::total_cmp);
    let percentile = |fraction: f32| {
        floor_luminance
            .get(((floor_luminance.len().saturating_sub(1)) as f32 * fraction) as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let (low, high) = (percentile(0.03), percentile(0.97));
    // A floor with no light and shade in it at all (an untextured test level) is drawn in the
    // light half of the page rather than stretched from nothing.
    let uniform = high - low < 0.02;
    let span = (high - low).max(0.02);

    // How far each floor texel is from the nearest wall, and how far the widest spot near it
    // is: the first shades the floor into its walls, the second tells a corridor (never far
    // from a wall) from a room.
    let distance = floor_distance(mask);
    let openness = window_max(
        &distance,
        w,
        h,
        (palette.tile_texels * 2.2).max(4.0) as usize,
    );
    let corridor_limit = palette.tile_texels * 2.4;

    let mut out = vec![0u8; w * h * BYTES_PER_TEXEL];
    for y in 0..h {
        for x in 0..w {
            let index = y * w + x;
            let grain = grain(x as u32, y as u32);
            let vignette = vignette(x, y, w, h);
            let rgb = match mask.cells[index] {
                MapCell::Floor => {
                    let lum = if uniform {
                        0.8
                    } else {
                        ((luminance[index] - low) / span).clamp(0.0, 1.0)
                    };
                    let shade = 0.5 + 0.5 * lum.powf(0.7);
                    let room = mix(palette.floor_dark, palette.floor_light, shade);
                    // Corridors blend in over a few texels rather than switching.
                    let corridor_amount = ((corridor_limit - openness[index])
                        / (palette.tile_texels * 0.6))
                        .clamp(0.0, 1.0);
                    let stone = mix(
                        room,
                        mix(palette.corridor, palette.floor_dark, 1.0 - shade),
                        corridor_amount * 0.85,
                    );
                    let stone = scale(stone, flagstone(x, y, palette.tile_texels));
                    let tinted = mix(stone, colour[index], palette.render_share);
                    // Props on the floor keep their outline.
                    let edge = sobel(&luminance, w, h, x, y).min(1.0);
                    let inked = mix(tinted, palette.wall, edge * 0.4);
                    // Ambient occlusion into the walls, and the walls' shadow cast south-east.
                    let occlusion =
                        (1.0 - distance[index] / (palette.tile_texels * 0.9)).clamp(0.0, 1.0);
                    let shadow = cast_shadow(mask, x, y, (palette.tile_texels * 0.45) as i32);
                    scale(inked, 1.0 - occlusion * occlusion * 0.38 - shadow * 0.22)
                }
                MapCell::Wall => {
                    if floor_within(mask, x, y, 1) {
                        // The inked face.
                        palette.wall
                    } else {
                        // Dressed stone, bevelled: lit edges face north-west, shaded ones
                        // south-east, so every wall reads as standing up off the floor.
                        let lit = is_open(mask, x as i32 - 2, y as i32 - 2);
                        let dark = is_open(mask, x as i32 + 2, y as i32 + 2);
                        let bevel = if lit {
                            1.28
                        } else if dark {
                            0.72
                        } else {
                            1.0
                        };
                        let texture = 0.94 + 0.06 * hash01(x as u32 / 3, y as u32 / 3);
                        scale(palette.wall_edge, bevel * texture)
                    }
                }
                MapCell::Outside => palette.outside,
            };
            // The surround is left flat — no grain, no vignette — so it meets the interface's
            // own ground colour exactly and the picture has no visible edge.
            let (grain, vignette) = if mask.cells[index] == MapCell::Outside {
                (0.0, 1.0)
            } else {
                (grain, vignette)
            };
            let offset = index * BYTES_PER_TEXEL;
            for channel in 0..3 {
                let value = (rgb[channel] + grain) * vignette;
                out[offset + channel] = value.round().clamp(0.0, 255.0) as u8;
            }
            out[offset + 3] = 255;
        }
    }
    Ok(out)
}

/// A colour times a brightness.
fn scale(colour: [f32; 3], factor: f32) -> [f32; 3] {
    [colour[0] * factor, colour[1] * factor, colour[2] * factor]
}

/// A deterministic hash in `0..1`.
fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Paving: stones `size` texels square in offset courses, each its own shade, with a grout
/// line between them. A brightness multiplier around one.
fn flagstone(x: usize, y: usize, size: f32) -> f32 {
    let size = size.max(4.0);
    let row = (y as f32 / size).floor();
    let shifted = x as f32 + if row as i32 % 2 == 0 { 0.0 } else { size * 0.5 };
    let column = (shifted / size).floor();
    let (fx, fy) = (shifted - column * size, y as f32 - row * size);
    let grout = fx < 1.0 || fy < 1.0;
    let stone = 0.95 + 0.07 * hash01(column as u32, row as u32);
    if grout { stone * 0.88 } else { stone }
}

/// Whether a texel is open floor, treating the edge as closed.
fn is_open(mask: &WalkableMask, x: i32, y: i32) -> bool {
    x >= 0 && y >= 0 && mask.cell(x as u32, y as u32) == MapCell::Floor
}

/// How much of a wall's shadow falls on a floor texel: walls up and to the left within
/// `reach` texels, nearer walls darker.
fn cast_shadow(mask: &WalkableMask, x: usize, y: usize, reach: i32) -> f32 {
    for step in 1..=reach.max(1) {
        let (sx, sy) = (x as i32 - step, y as i32 - step);
        if sx < 0 || sy < 0 {
            return 0.0;
        }
        if mask.cell(sx as u32, sy as u32) == MapCell::Wall {
            return 1.0 - (step - 1) as f32 / reach.max(1) as f32;
        }
    }
    0.0
}

/// Chamfer distance, in texels, from every floor texel to the nearest non-floor texel.
fn floor_distance(mask: &WalkableMask) -> Vec<f32> {
    let (w, h) = (mask.width as usize, mask.height as usize);
    let mut d: Vec<f32> = mask
        .cells
        .iter()
        .map(|cell| {
            if *cell == MapCell::Floor {
                f32::MAX
            } else {
                0.0
            }
        })
        .collect();
    let (orthogonal, diagonal) = (1.0f32, std::f32::consts::SQRT_2);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if d[i] == 0.0 {
                continue;
            }
            let mut best = d[i];
            if x > 0 {
                best = best.min(d[i - 1] + orthogonal);
            }
            if y > 0 {
                best = best.min(d[i - w] + orthogonal);
                if x > 0 {
                    best = best.min(d[i - w - 1] + diagonal);
                }
                if x + 1 < w {
                    best = best.min(d[i - w + 1] + diagonal);
                }
            }
            d[i] = best;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if d[i] == 0.0 {
                continue;
            }
            let mut best = d[i];
            if x + 1 < w {
                best = best.min(d[i + 1] + orthogonal);
            }
            if y + 1 < h {
                best = best.min(d[i + w] + orthogonal);
                if x + 1 < w {
                    best = best.min(d[i + w + 1] + diagonal);
                }
                if x > 0 {
                    best = best.min(d[i + w - 1] + diagonal);
                }
            }
            d[i] = best;
        }
    }
    // A level with no walls at all has nothing to measure from.
    for value in &mut d {
        if *value == f32::MAX {
            *value = 1e6;
        }
    }
    d
}

/// The largest value within `radius` texels of each texel, by two one-dimensional passes.
fn window_max(values: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; values.len()];
    for y in 0..h {
        for x in 0..w {
            let (lo, hi) = (x.saturating_sub(radius), (x + radius).min(w - 1));
            rows[y * w + x] = values[y * w + lo..=y * w + hi]
                .iter()
                .copied()
                .fold(0.0, f32::max);
        }
    }
    let mut out = vec![0.0f32; values.len()];
    for x in 0..w {
        for y in 0..h {
            let (lo, hi) = (y.saturating_sub(radius), (y + radius).min(h - 1));
            out[y * w + x] = (lo..=hi).map(|row| rows[row * w + x]).fold(0.0, f32::max);
        }
    }
    out
}

/// Linear interpolation between two colours.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Paper grain: a few levels of deterministic noise per texel.
fn grain(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    ((value & 0xFF) as f32 / 255.0 - 0.5) * 5.0
}

/// Darkens toward the picture's edge, so the map reads as a sheet rather than a crop.
fn vignette(x: usize, y: usize, w: usize, h: usize) -> f32 {
    let u = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
    let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
    let distance = (u * u + v * v).sqrt() / std::f32::consts::SQRT_2;
    1.0 - 0.28 * distance.powf(2.5)
}

/// Edge strength of the luminance at a texel: what inks the outline of a prop on the floor.
fn sobel(luminance: &[f32], w: usize, h: usize, x: usize, y: usize) -> f32 {
    let at = |dx: isize, dy: isize| {
        let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
        let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
        luminance[sy * w + sx]
    };
    let gx = at(1, -1) + 2.0 * at(1, 0) + at(1, 1) - at(-1, -1) - 2.0 * at(-1, 0) - at(-1, 1);
    let gy = at(-1, 1) + 2.0 * at(0, 1) + at(1, 1) - at(-1, -1) - 2.0 * at(0, -1) - at(1, -1);
    ((gx * gx + gy * gy).sqrt() * 1.5 - 0.15).max(0.0)
}

/// Whether floor lies within `reach` texels of a texel.
fn floor_within(mask: &WalkableMask, x: usize, y: usize, reach: i32) -> bool {
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx >= 0 && ny >= 0 && mask.cell(nx as u32, ny as u32) == MapCell::Floor {
                return true;
            }
        }
    }
    false
}

/// Writes RGBA texels as a PNG, making the directory if it has to.
///
/// # Errors
///
/// [`AssetError::Io`]-style [`AssetError::Invalid`] naming `path` if the directory cannot be
/// made, the file cannot be written, or the texels do not match the size.
pub fn save_png(path: &Path, width: u32, height: u32, texels: &[u8]) -> Result<(), AssetError> {
    let name = path.display().to_string();
    let fail = |reason: String| AssetError::invalid(&name, "png", reason);
    if texels.len() != width as usize * height as usize * BYTES_PER_TEXEL {
        return Err(fail(format!(
            "{width}x{height} needs {} bytes",
            width as usize * height as usize * 4
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| fail(error.to_string()))?;
    }
    let file = std::fs::File::create(path).map_err(|error| fail(error.to_string()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| fail(error.to_string()))?;
    writer
        .write_image_data(texels)
        .map_err(|error| fail(error.to_string()))?;
    writer.finish().map_err(|error| fail(error.to_string()))
}

/// Reads a map picture back.
///
/// # Errors
///
/// The I/O failure or the decoder's complaint, naming `path`.
pub fn load_png(path: &Path) -> Result<TextureAsset, AssetError> {
    let name = path.display().to_string();
    let bytes = std::fs::read(path)
        .map_err(|error| AssetError::invalid(&name, "png", error.to_string()))?;
    crate::gltf_import::decode_png_file(&name, TextureFormat::Rgba8Unorm, &bytes)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn mask_of(rows: &[&str]) -> WalkableMask {
        let height = rows.len() as u32;
        let width = rows[0].len() as u32;
        let cells = rows
            .iter()
            .flat_map(|row| {
                row.chars().map(|c| match c {
                    '.' => MapCell::Floor,
                    '#' => MapCell::Wall,
                    _ => MapCell::Outside,
                })
            })
            .collect();
        WalkableMask {
            width,
            height,
            cells,
        }
    }

    fn grey(width: u32, height: u32, value: u8) -> Vec<u8> {
        (0..width * height)
            .flat_map(|_| [value, value, value, 255])
            .collect()
    }

    fn luminance(texels: &[u8], x: usize, y: usize, w: usize) -> f32 {
        let o = (y * w + x) * 4;
        0.299 * f32::from(texels[o])
            + 0.587 * f32::from(texels[o + 1])
            + 0.114 * f32::from(texels[o + 2])
    }

    #[test]
    fn floor_is_light_walls_are_ink_and_the_outside_is_dark() {
        let mask = mask_of(&[
            "          ",
            " ######## ",
            " #......# ",
            " #......# ",
            " #......# ",
            " ######## ",
            "          ",
        ]);
        let styled =
            style_map(&grey(10, 7, 128), (10, 7), &mask, &MapPalette::default()).expect("styles");
        let floor = luminance(&styled, 4, 3, 10);
        let wall = luminance(&styled, 1, 3, 10);
        let outside = luminance(&styled, 0, 0, 10);
        assert!(floor > wall + 60.0, "{floor} {wall}");
        assert!(wall > outside, "{wall} {outside}");
        assert!(
            styled.chunks_exact(4).all(|texel| texel[3] == 255),
            "opaque"
        );
    }

    #[test]
    fn the_render_s_light_and_shade_survive_into_the_floor() {
        let mask = mask_of(&["........", "........", "........", "........"]);
        let mut capture = grey(8, 4, 40);
        for y in 0..4 {
            for x in 4..8 {
                let o = (y * 8 + x) * 4;
                capture[o..o + 3].copy_from_slice(&[220, 220, 220]);
            }
        }
        let styled = style_map(&capture, (8, 4), &mask, &MapPalette::default()).expect("styles");
        assert!(luminance(&styled, 6, 2, 8) > luminance(&styled, 1, 2, 8) + 30.0);
    }

    #[test]
    fn styling_is_deterministic_and_resamples_a_capture_of_another_size() {
        let mask = mask_of(&["..##", "..# ", "....", "#..."]);
        let a = style_map(&grey(8, 8, 90), (8, 8), &mask, &MapPalette::default()).expect("styles");
        let b = style_map(&grey(8, 8, 90), (8, 8), &mask, &MapPalette::default()).expect("styles");
        assert_eq!(a, b);
        assert_eq!(a.len(), 4 * 4 * 4, "the mask's size, not the capture's");
    }

    #[test]
    fn a_capture_that_does_not_match_its_size_is_refused() {
        let mask = mask_of(&["..", ".."]);
        assert!(style_map(&[0; 12], (2, 2), &mask, &MapPalette::default()).is_err());
    }

    #[test]
    fn a_map_round_trips_through_its_file() {
        let path =
            std::env::temp_dir().join(format!("map-image-{}/test.map.png", std::process::id()));
        let texels: Vec<u8> = (0..16 * 8)
            .flat_map(|i| [i as u8, 255 - i as u8, 7, 255])
            .collect();
        save_png(&path, 16, 8, &texels).expect("writes");
        let back = load_png(&path).expect("reads");
        assert_eq!((back.width, back.height), (16, 8));
        assert_eq!(back.texels, texels);
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}
