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

    let mut chain = Vec::with_capacity(levels as usize - 1);
    let mut source = texels.to_vec();
    let (mut source_width, mut source_height) = (width, height);
    for level in 1..levels {
        let (target_width, target_height) = level_size(width, height, level);
        let reduced = halve(
            &source,
            source_width,
            source_height,
            target_width,
            target_height,
            kind,
        );
        source = reduced.clone();
        source_width = target_width;
        source_height = target_height;
        chain.push(reduced);
    }
    chain
}

/// One reduction step: `source` at its size, filtered down to `target_width` x
/// `target_height`.
fn halve(
    source: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    kind: TextureKind,
) -> Vec<u8> {
    let mut out = vec![0_u8; target_width as usize * target_height as usize * 4];
    for y in 0..target_height {
        for x in 0..target_width {
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
                TextureKind::Color => average_color(&block),
                TextureKind::Normal => average_normal(&block),
            };
            let at = (y as usize * target_width as usize + x as usize) * 4;
            out[at..at + 4].copy_from_slice(&filtered);
        }
    }
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
fn average_color(block: &[[u8; 4]; 4]) -> [u8; 4] {
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

/// One sRGB-encoded channel as a linear value in [0, 1].
fn srgb_to_linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear value in [0, 1] back to an sRGB-encoded channel.
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
