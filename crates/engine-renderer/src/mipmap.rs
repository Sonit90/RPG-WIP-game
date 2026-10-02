//! Mip-chain generation for uploaded textures.
//!
//! # Why a texture needs one
//!
//! A texture sampled at less than one texel per pixel aliases: the hardware picks one texel
//! out of the several that fell inside the pixel, and which one it picks changes as the
//! camera moves. On a base colour map that is shimmer on a distant wall. On a *normal* map
//! it is worse, because the aliased value is a direction, and a direction that flickers makes
//! the lighting flicker with it — which is exactly what Milestone 3 made visible by starting
//! to consume normal maps at all.
//!
//! A mip chain is the fix: the same image at half size, quarter size and so on, each level
//! pre-filtered, so the hardware can pick a level whose texel density matches the pixel
//! density and blend between the two.
//!
//! # Why the filter is not one filter
//!
//! A box filter — average the four texels of each 2x2 block — is correct for *data*. It is
//! wrong twice over for anything else:
//!
//! * **Colour.** An sRGB texture's numbers are not proportional to light. Averaging them
//!   directly and then reading the result through an sRGB view darkens every mip; the
//!   familiar symptom is a texture that gets muddier with distance. The average has to happen
//!   in linear space, which means decoding, averaging and re-encoding.
//! * **Normals.** A normal map's texels are a *direction* packed into [0,1]. The average of
//!   two unit vectors is not a unit vector, and the shorter the average the flatter the
//!   surface — averaging in the packed space and leaving it there produces mips that are
//!   both denormalized and biased towards (0.5, 0.5, 1). Unpacking, averaging and
//!   renormalizing keeps every level a real direction.
//!
//! So the filter is chosen by what the texture *means*, which is why [`TextureKind`] exists
//! and why the renderer works out a texture's role from the material slots that name it
//! before it uploads anything.
//!
//! # Non-power-of-two images
//!
//! Every level halves and rounds down, stopping at 1x1, so a 100x60 image produces
//! 50x30, 25x15, 12x7, 6x3, 3x1, 1x1 — D3D12's own rule, and the one its `MipLevels` count
//! is defined against. Where a dimension is odd the 2x2 block runs off the edge and the
//! sample is clamped back onto the last row or column, which shifts that level by half a
//! texel. That is the standard trade: the alternative is a windowed filter that changes
//! width per level, and no one has ever seen the difference on a game texture.
//!
//! # What it costs, and why it is tables and bands
//!
//! A colour texel of a mip level is four sRGB decodes per channel and one encode, and both
//! are a `powf`: fifteen of them per output texel. On the imported 2048x2048 hero textures
//! that was 390 ms of a one-second launch (docs/PERFORMANCE_AUDIT.md). Both conversions are
//! now tables computed from the very same formulas, so every output byte is unchanged:
//!
//! * **decode** takes a `u8`, so it has exactly 256 answers - [`SRGB_TO_LINEAR`] is the
//!   formula evaluated 256 times;
//! * **encode** is monotonic, so the byte a linear value encodes to is the number of
//!   [`ENCODE_THRESHOLDS`] at or below it, each threshold being the smallest float the formula
//!   encodes to that byte. A test walks every float in [0, 1] through both and compares.
//!
//! Each level's rows are then filtered in bands across threads ([`crate::parallel`]), each
//! row from the level above it exactly as before.

use std::ops::Range;
use std::sync::LazyLock;

/// What a texture means, which decides how it is filtered and how it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureKind {
    /// Colour, authored in sRGB: base colour and emissive. Read through an sRGB view, and
    /// filtered in linear space.
    Color,
    /// Data, linear: ORM, masks, the UI atlas. Read and filtered as plain numbers.
    Data,
    /// A tangent-space normal map. Read as data, filtered as directions.
    Normal,
}

impl TextureKind {
    /// Whether the hardware should linearize this texture on read.
    #[must_use]
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::Color)
    }
}

/// Which block-compressed format an image should be uploaded as, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    /// Uploaded as it is, four bytes a texel.
    None,
    /// BC1: opaque colour at half a byte a texel.
    Bc1,
    /// BC5: the X and Y of a tangent-space normal at one byte a texel, with Z reconstructed
    /// in the shader.
    Bc5,
}

/// The smallest image worth compressing, in texels on each edge.
///
/// Below this the saving is a few kilobytes and the cost is real: block compression works on
/// 4x4 tiles, so on a 16x16 image one bad block is a sixteenth of the picture, and on the
/// 1x1 fallbacks it would quantize a texel that the whole material ABI depends on being
/// exact. The engine's procedural textures are mostly small and its imported ones are not,
/// which is exactly the split this threshold makes.
pub const MIN_COMPRESSED_SIZE: u32 = 64;

/// Decides how an image should be compressed.
///
/// `has_alpha` says whether any texel's alpha is not fully opaque. It matters only for
/// colour: BC1's alpha is one bit *per block*, which would fray the edge of every cutout
/// leaf, so a colour map with meaningful alpha stays uncompressed until there is a BC7
/// encoder to hand it to.
///
/// ORM maps are never compressed — see `engine_assets::bc` for why three independent data
/// channels and one pair of shared endpoints do not mix.
#[must_use]
pub fn compression_for(kind: TextureKind, width: u32, height: u32, has_alpha: bool) -> Compression {
    if width < MIN_COMPRESSED_SIZE || height < MIN_COMPRESSED_SIZE {
        return Compression::None;
    }
    match kind {
        TextureKind::Color if !has_alpha => Compression::Bc1,
        TextureKind::Normal => Compression::Bc5,
        TextureKind::Color | TextureKind::Data => Compression::None,
    }
}

/// Whether any texel of an RGBA8 buffer is not fully opaque.
#[must_use]
pub fn has_transparency(texels: &[u8]) -> bool {
    texels.chunks_exact(4).any(|texel| texel[3] != 255)
}

/// How many mip levels a `width` x `height` texture has, counting the full-size level.
///
/// D3D12's definition: halve, rounding down, until both dimensions are 1.
#[must_use]
pub fn level_count(width: u32, height: u32) -> u32 {
    let mut levels = 1;
    let mut size = width.max(height);
    while size > 1 {
        size /= 2;
        levels += 1;
    }
    levels
}

/// The size of mip `level` of a `width` x `height` texture.
#[must_use]
pub fn level_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}

/// Builds every mip level below level 0, filtered for `kind`.
///
/// Level 0 is *not* included: the caller already has it, and copying it would double the
/// largest allocation in the chain for no reason. The returned vector is level 1 onwards, in
/// order, each `level_size(...)` texels of RGBA8.
///
/// Returns an empty vector for a 1x1 texture, which has no levels below its first — the
/// shared fallback texels are all 1x1, and they take this path every time a renderer starts.
#[must_use]
pub fn generate(texels: &[u8], width: u32, height: u32, kind: TextureKind) -> Vec<Vec<u8>> {
    let levels = level_count(width, height);
    if levels <= 1 || texels.len() < (width as usize * height as usize * 4) {
        return Vec::new();
    }

    let mut chain: Vec<Vec<u8>> = Vec::with_capacity(levels as usize - 1);
    let (mut source_width, mut source_height) = (width, height);
    for level in 1..levels {
        let (target_width, target_height) = level_size(width, height, level);
        // Each level is reduced from the one above it, read in place: level 0 is the
        // caller's, every other one is the last level pushed.
        let source = chain.last().map_or(texels, Vec::as_slice);
        let reduced = halve(
            source,
            source_width,
            source_height,
            target_width,
            target_height,
            kind,
        );
        source_width = target_width;
        source_height = target_height;
        chain.push(reduced);
    }
    chain
}

/// One reduction step: `source` at its size, filtered down to `target_width` x
/// `target_height`, in bands of rows across threads when the level is large enough to pay
/// for them.
fn halve(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    kind: TextureKind,
) -> Vec<u8> {
    let row_bytes = target_width as usize * 4;
    let mut out = vec![0_u8; row_bytes * target_height as usize];
    crate::parallel::for_each_band(
        target_height,
        row_bytes,
        target_width as usize,
        &mut out,
        |rows, band| {
            halve_rows(
                source,
                source_width,
                source_height,
                target_width,
                kind,
                rows,
                band,
            );
        },
    );
    out
}

/// Rows `rows` of one reduction step, written into `out`, which holds exactly those rows.
fn halve_rows(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    kind: TextureKind,
    rows: Range<u32>,
    out: &mut [u8],
) {
    // Dereferenced once per band rather than once per channel: a `LazyLock` read is an
    // atomic load, and the inner loop below runs millions of times.
    let tables = (&*SRGB_TO_LINEAR, &*ENCODE_THRESHOLDS);
    for (y, row) in rows.zip(out.chunks_exact_mut(target_width as usize * 4)) {
        for (x, destination) in (0..target_width).zip(row.chunks_exact_mut(4)) {
            // The 2x2 block this destination texel covers, clamped so an odd source
            // dimension reads its last row or column twice rather than past the end.
            let x0 = (x * 2).min(source_width - 1);
            let y0 = (y * 2).min(source_height - 1);
            let x1 = (x * 2 + 1).min(source_width - 1);
            let y1 = (y * 2 + 1).min(source_height - 1);
            let block = [
                texel(source, source_width, x0, y0),
                texel(source, source_width, x1, y0),
                texel(source, source_width, x0, y1),
                texel(source, source_width, x1, y1),
            ];
            let filtered = match kind {
                TextureKind::Data => average_data(&block),
                TextureKind::Color => average_color(&block, tables),
                TextureKind::Normal => average_normal(&block),
            };
            destination.copy_from_slice(&filtered);
        }
    }
}

/// Block-compresses one level of a chain, in bands of block rows across threads when the
/// level is large enough to pay for them.
///
/// Byte for byte what `engine_assets::bc::compress_bc1` / `compress_bc5` return: each block
/// is encoded from its own texels, and a band is a run of whole block rows. Empty for
/// [`Compression::None`], which has nothing to compress.
#[must_use]
pub fn compress_level(compression: Compression, texels: &[u8], width: u32, height: u32) -> Vec<u8> {
    use engine_assets::bc;

    let (blocks_x, blocks_y) = bc::block_counts(width, height);
    let block_bytes = match compression {
        Compression::Bc1 => bc::BC1_BLOCK_BYTES,
        Compression::Bc5 => bc::BC5_BLOCK_BYTES,
        Compression::None => return Vec::new(),
    };
    let row_bytes = blocks_x as usize * block_bytes;
    let mut out = vec![0_u8; row_bytes * blocks_y as usize];
    let texels_per_row = blocks_x as usize * (bc::BLOCK_SIZE * bc::BLOCK_SIZE) as usize;
    crate::parallel::for_each_band(
        blocks_y,
        row_bytes,
        texels_per_row,
        &mut out,
        |rows, band| match compression {
            Compression::Bc5 => bc::compress_bc5_rows(texels, width, height, rows, band),
            _ => bc::compress_bc1_rows(texels, width, height, rows, band),
        },
    );
    out
}

/// One RGBA texel out of a row-major buffer.
fn texel(source: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * width as usize + x as usize) * 4;
    [source[at], source[at + 1], source[at + 2], source[at + 3]]
}

/// The plain average of four texels, channel by channel.
fn average_data(block: &[[u8; 4]; 4]) -> [u8; 4] {
    let mut out = [0_u8; 4];
    for (channel, value) in out.iter_mut().enumerate() {
        let sum: u32 = block.iter().map(|texel| u32::from(texel[channel])).sum();
        // Rounded, not truncated: truncation biases every mip level darker, and the bias
        // compounds down a chain of ten.
        *value = ((sum + 2) / 4) as u8;
    }
    out
}

/// The average of four sRGB texels, computed in linear space.
///
/// Alpha is linear already — it is coverage, not colour — so it is averaged directly. Doing
/// otherwise is the second half of the classic mip bug: an alpha-tested leaf whose alpha was
/// gamma-averaged loses coverage with every level, and the foliage thins out with distance.
///
/// `tables` is [`SRGB_TO_LINEAR`] and [`ENCODE_THRESHOLDS`], passed in so the caller reads
/// each `LazyLock` once rather than once per channel.
fn average_color(block: &[[u8; 4]; 4], tables: (&[f32; 256], &[f32; 255])) -> [u8; 4] {
    let (decode, thresholds) = tables;
    let mut out = [0_u8; 4];
    for channel in 0..3 {
        // The same four terms summed in the same order as the formula path, so the sum is
        // the same float.
        let sum: f32 = block
            .iter()
            .map(|texel| decode[usize::from(texel[channel])])
            .sum();
        out[channel] = encode_with(thresholds, sum / 4.0);
    }
    let alpha: u32 = block.iter().map(|texel| u32::from(texel[3])).sum();
    out[3] = ((alpha + 2) / 4) as u8;
    out
}

/// The average of four packed normals, renormalized.
///
/// The blue channel is not treated specially: unpack all three, average, normalize, repack.
/// A block of identical texels therefore comes back identical, which is what keeps a flat
/// region of a normal map flat all the way down its chain.
fn average_normal(block: &[[u8; 4]; 4]) -> [u8; 4] {
    let mut sum = [0.0_f32; 3];
    for texel in block {
        for (channel, total) in sum.iter_mut().enumerate() {
            // The same decode the shader uses — about 128, not about 127.5 — so a flat
            // texel of 128 unpacks to exactly zero here as well and cannot drift down the
            // chain.
            *total += f32::from(texel[channel]) * (1.0 / 128.0) - 1.0;
        }
    }
    let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
    let unit = if length > 1e-6 {
        [sum[0] / length, sum[1] / length, sum[2] / length]
    } else {
        // Four normals that cancel exactly. Geometrically this is a texel with no direction
        // at all, and +Z — the flat normal — is the only answer that keeps the level usable.
        [0.0, 0.0, 1.0]
    };
    let mut out = [0_u8; 4];
    for (channel, value) in unit.iter().enumerate() {
        // Repacked with the same convention: 0 maps to 128 exactly.
        out[channel] = ((value + 1.0) * 128.0).round().clamp(0.0, 255.0) as u8;
    }
    let alpha: u32 = block.iter().map(|texel| u32::from(texel[3])).sum();
    out[3] = ((alpha + 2) / 4) as u8;
    out
}

/// Every sRGB byte's linear value: [`srgb_to_linear`] evaluated once for each of the 256.
static SRGB_TO_LINEAR: LazyLock<[f32; 256]> =
    LazyLock::new(|| std::array::from_fn(|value| srgb_to_linear(value as u8)));

/// For each byte from 1 to 255, the smallest float in [0, 1] that [`linear_to_srgb`] encodes
/// to that byte or above. Ascending, because the encoding is monotonic.
static ENCODE_THRESHOLDS: LazyLock<[f32; 255]> =
    LazyLock::new(|| std::array::from_fn(|index| smallest_encoding_to((index + 1) as u8)));

/// The smallest float in [0, 1] that [`linear_to_srgb`] encodes to `target` or above.
///
/// A binary search over the *bits* of the float: non-negative floats order the same way their
/// bit patterns do, so this finds the exact boundary rather than an approximation of it. 30
/// steps each, 255 thresholds: a few thousand evaluations, once per process.
fn smallest_encoding_to(target: u8) -> f32 {
    let (mut low, mut high) = (0_u32, 1.0_f32.to_bits());
    while low < high {
        let middle = low + (high - low) / 2;
        if linear_to_srgb(f32::from_bits(middle)) >= target {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    f32::from_bits(low)
}

/// [`linear_to_srgb`], by counting the thresholds at or below `value`.
///
/// Equal to the formula for every float - see the test that checks all of them - and a
/// binary search over 255 floats instead of a `powf`.
fn encode_with(thresholds: &[f32; 255], value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    thresholds.partition_point(|threshold| *threshold <= value) as u8
}

/// One sRGB-encoded channel as a linear value in [0, 1]. The formula [`SRGB_TO_LINEAR`] is
/// built from; the filter reads the table.
fn srgb_to_linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear value in [0, 1] back to an sRGB-encoded channel. The formula
/// [`ENCODE_THRESHOLDS`] is built from; the filter counts thresholds instead.
fn linear_to_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decode_table_is_the_formula() {
        for value in 0..=255_u8 {
            assert_eq!(
                SRGB_TO_LINEAR[usize::from(value)].to_bits(),
                srgb_to_linear(value).to_bits()
            );
        }
    }

    /// Every float in [0, 1] whose bit pattern is a multiple of a prime stride, plus both
    /// sides of every threshold: cheap enough for a debug test run, and the thresholds are
    /// exactly where an off-by-one would be.
    #[test]
    fn the_threshold_encoding_is_the_formula_across_the_range() {
        let thresholds = &*ENCODE_THRESHOLDS;
        let mut bits = 0_u32;
        while bits <= 1.0_f32.to_bits() {
            let value = f32::from_bits(bits);
            assert_eq!(
                encode_with(thresholds, value),
                linear_to_srgb(value),
                "{value:e}"
            );
            bits += 4099;
        }
        for threshold in thresholds {
            for value in [
                f32::from_bits(threshold.to_bits().saturating_sub(1)),
                *threshold,
            ] {
                assert_eq!(
                    encode_with(thresholds, value),
                    linear_to_srgb(value),
                    "{value:e}"
                );
            }
        }
        // Outside the range, as a sum of decoded values can be by an ulp.
        for value in [-1.0_f32, -0.0, 1.000_000_1, 2.0, 4.0] {
            assert_eq!(
                encode_with(thresholds, value),
                linear_to_srgb(value),
                "{value:e}"
            );
        }
    }

    /// The whole claim, for every one of the 1,065,353,217 floats in [0, 1]. A few seconds in
    /// release across the machine's cores, minutes in debug, so it is ignored by default:
    ///
    /// ```text
    /// cargo test --release -p engine-renderer every_float -- --ignored
    /// ```
    #[test]
    #[ignore = "exhaustive: run in release with --ignored"]
    fn every_float_in_range_encodes_as_the_formula_does() {
        let thresholds = &*ENCODE_THRESHOLDS;
        let last = 1.0_f32.to_bits();
        let threads = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
        let per = last / threads as u32 + 1;
        std::thread::scope(|scope| {
            for thread in 0..threads as u32 {
                scope.spawn(move || {
                    let start = thread * per;
                    let end = (start + per).min(last + 1);
                    for bits in start..end {
                        let value = f32::from_bits(bits);
                        assert_eq!(
                            encode_with(thresholds, value),
                            linear_to_srgb(value),
                            "{bits:#x}"
                        );
                    }
                });
            }
        });
    }

    /// The filter as it was, with a `powf` per conversion: what the tables replaced.
    fn reference_average_color(block: &[[u8; 4]; 4]) -> [u8; 4] {
        let mut out = [0_u8; 4];
        for channel in 0..3 {
            let sum: f32 = block
                .iter()
                .map(|texel| srgb_to_linear(texel[channel]))
                .sum();
            out[channel] = linear_to_srgb(sum / 4.0);
        }
        let alpha: u32 = block.iter().map(|texel| u32::from(texel[3])).sum();
        out[3] = ((alpha + 2) / 4) as u8;
        out
    }

    #[test]
    fn the_table_filter_averages_exactly_as_the_formula_filter_did() {
        let tables = (&*SRGB_TO_LINEAR, &*ENCODE_THRESHOLDS);
        // A deterministic spread of blocks: every value appears in every position.
        let mut state = 0x9e37_79b9_u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for _ in 0..200_000 {
            let block: [[u8; 4]; 4] = std::array::from_fn(|_| next().to_le_bytes());
            assert_eq!(
                average_color(&block, tables),
                reference_average_color(&block)
            );
        }
    }

    #[test]
    fn a_large_level_filtered_in_bands_is_the_level_filtered_whole() {
        // Wide enough that the bands are real: 640x480 is 76,800 destination texels per
        // level-1 row block, several bands' worth.
        let (width, height) = (1281_u32, 961_u32);
        let texels: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let (x, y) = (index % width, index / width);
                [(x ^ y) as u8, (x * 3) as u8, (y * 5) as u8, (x + y) as u8]
            })
            .collect();
        for kind in [TextureKind::Color, TextureKind::Data, TextureKind::Normal] {
            let (target_width, target_height) = level_size(width, height, 1);
            let banded = halve(&texels, width, height, target_width, target_height, kind);
            let mut whole = vec![0_u8; banded.len()];
            halve_rows(
                &texels,
                width,
                height,
                target_width,
                kind,
                0..target_height,
                &mut whole,
            );
            assert_eq!(banded, whole, "{kind:?}");
        }
    }

    #[test]
    fn a_compressed_level_in_bands_is_the_level_compressed_whole() {
        let (width, height) = (1024_u32, 516_u32);
        let texels: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let (x, y) = (index % width, index / width);
                [(x * 7) as u8, (y * 3) as u8, (x ^ y) as u8, 255]
            })
            .collect();
        assert_eq!(
            compress_level(Compression::Bc1, &texels, width, height),
            engine_assets::bc::compress_bc1(&texels, width, height)
        );
        assert_eq!(
            compress_level(Compression::Bc5, &texels, width, height),
            engine_assets::bc::compress_bc5(&texels, width, height)
        );
    }

    #[test]
    fn a_chain_halves_to_one_by_one_and_counts_the_way_d3d12_counts() {
        assert_eq!(level_count(1, 1), 1);
        assert_eq!(level_count(2, 2), 2);
        assert_eq!(level_count(256, 256), 9);
        // The larger dimension decides, which is what makes a 256x1 strip nine levels and
        // not one.
        assert_eq!(level_count(256, 1), 9);
        // Not a power of two: 100 -> 50 -> 25 -> 12 -> 6 -> 3 -> 1.
        assert_eq!(level_count(100, 60), 7);
        assert_eq!(level_size(100, 60, 3), (12, 7));
        assert_eq!(level_size(100, 60, 6), (1, 1));
    }

    #[test]
    fn every_level_is_the_size_and_length_the_gpu_expects() {
        let texels = vec![128_u8; 100 * 60 * 4];
        let chain = generate(&texels, 100, 60, TextureKind::Data);
        assert_eq!(chain.len(), level_count(100, 60) as usize - 1);
        for (index, level) in chain.iter().enumerate() {
            let (width, height) = level_size(100, 60, index as u32 + 1);
            assert_eq!(
                level.len(),
                width as usize * height as usize * 4,
                "level {} is {width}x{height}",
                index + 1
            );
        }
    }

    #[test]
    fn a_one_by_one_texture_has_no_levels_below_it() {
        // Every shared fallback is 1x1, so this is the path a renderer takes four times
        // before it draws anything.
        assert!(generate(&[255, 255, 255, 255], 1, 1, TextureKind::Color).is_empty());
    }

    #[test]
    fn a_uniform_image_survives_the_whole_chain_unchanged() {
        // The invariant that catches a filter with a bias in it: averaging four identical
        // texels must produce that texel, in every space, at every level.
        for kind in [TextureKind::Color, TextureKind::Data, TextureKind::Normal] {
            let texel = match kind {
                TextureKind::Normal => [128, 128, 255, 255],
                _ => [200, 100, 50, 255],
            };
            let texels: Vec<u8> = texel.iter().copied().cycle().take(16 * 16 * 4).collect();
            for level in generate(&texels, 16, 16, kind) {
                for chunk in level.chunks_exact(4) {
                    match kind {
                        // The normal path renormalizes, so a flat texel comes back within a
                        // rounding step of itself rather than bit-identical.
                        TextureKind::Normal => {
                            assert!(chunk[0].abs_diff(128) <= 1, "{chunk:?} for {kind:?}");
                            assert!(chunk[2] >= 254, "{chunk:?} for {kind:?}");
                        }
                        _ => assert_eq!(chunk, texel, "{kind:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn colour_is_averaged_in_linear_space_and_data_is_not() {
        // Black and white in equal parts. In linear space that is 0.5, which encodes to sRGB
        // 188; averaging the encoded values instead gives 128. The difference between those
        // two numbers is the whole reason `TextureKind::Color` exists, and a texture that
        // mips to 128 is the one that looks muddy in the distance.
        let checker = |kind: TextureKind| {
            let mut texels = Vec::new();
            for y in 0..2 {
                for x in 0..2 {
                    let value = if (x + y) % 2 == 0 { 0 } else { 255 };
                    texels.extend_from_slice(&[value, value, value, 255]);
                }
            }
            generate(&texels, 2, 2, kind)[0].clone()
        };

        let color = checker(TextureKind::Color);
        assert!(
            color[0].abs_diff(188) <= 1,
            "linear-space average should encode to about 188, got {}",
            color[0]
        );
        let data = checker(TextureKind::Data);
        assert!(
            data[0].abs_diff(128) <= 1,
            "data is averaged where it lies, got {}",
            data[0]
        );
    }

    #[test]
    fn normals_stay_unit_length_down_the_chain() {
        // Four different directions in every block, so the average is genuinely shorter than
        // one before it is renormalized. A filter that skipped the renormalize would produce
        // levels that are progressively flatter, and the lighting would fade with distance.
        let mut texels = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let tilt = if (x + y) % 2 == 0 { 40 } else { 216 };
                texels.extend_from_slice(&[tilt, 255 - tilt, 200, 255]);
            }
        }
        for level in generate(&texels, 8, 8, TextureKind::Normal) {
            for chunk in level.chunks_exact(4) {
                let vector = [
                    f32::from(chunk[0]) / 128.0 - 1.0,
                    f32::from(chunk[1]) / 128.0 - 1.0,
                    f32::from(chunk[2]) / 128.0 - 1.0,
                ];
                let length =
                    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
                assert!(
                    (length - 1.0).abs() < 0.02,
                    "a filtered normal must stay unit: {length} from {chunk:?}"
                );
            }
        }
    }

    #[test]
    fn an_odd_dimension_clamps_rather_than_reading_past_the_end() {
        // 3x3 -> 1x1: the second column and row of the block fall outside the image. This
        // panics rather than fails if the clamp is missing.
        let texels = vec![64_u8; 3 * 3 * 4];
        let chain = generate(&texels, 3, 3, TextureKind::Data);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].len(), 4);
        assert_eq!(chain[0][0], 64);
    }
}
