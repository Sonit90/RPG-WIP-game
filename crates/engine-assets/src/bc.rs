//! Block compression: BC1 for opaque colour, BC5 for tangent-space normals.
//!
//! # Why compress at all
//!
//! An RGBA8 texture costs four bytes a texel, resident for the whole run. BC1 costs half a
//! byte and BC5 costs one, so a 1024x1024 base colour map goes from 4 MB to 512 KB and a
//! normal map of the same size from 4 MB to 1 MB — and the saving is not only memory: a
//! compressed texture moves a quarter of the bytes across the bus at upload and, more
//! importantly, a quarter of the bytes through the texture cache on every sample. The
//! sampler decompresses in hardware, so the shader does not know the difference.
//!
//! # Why these two formats and not others
//!
//! * **BC1** for opaque colour. Two RGB565 endpoints and a two-bit index per texel: 8 bytes
//!   for a 4x4 block. It has visible error on smooth gradients and none worth seeing on the
//!   stylized, high-frequency art this engine draws. Its one-bit alpha mode is deliberately
//!   not used — a cutout material's alpha decides whether a fragment exists at all, and a
//!   format that quantizes it to one bit *per block* would fray every leaf edge. Colour with
//!   meaningful alpha stays uncompressed until there is a BC7 encoder to give it to.
//! * **BC5** for normals. Two independent single-channel blocks, each two endpoints and
//!   three-bit indices: 16 bytes for 4x4, one byte a texel. It stores X and Y only, at eight
//!   bits of endpoint precision each, and the shader reconstructs Z — which is *better* than
//!   BC1 on a normal map, not merely smaller, because BC1's shared RGB endpoints tie the
//!   three channels of a direction together and produce blocky lighting.
//!
//! ORM maps are deliberately left uncompressed. Occlusion, roughness and metallic are three
//! independent data channels, and BC1 couples them through one pair of endpoints: a block
//! where roughness varies and metallic does not comes back with both varying. BC7 handles it
//! properly and is a much larger encoder; until that exists, four bytes a texel on the one
//! map where the artefacts would be material behaviour rather than a shade of colour is the
//! right trade, and it is stated here so the omission reads as a decision.
//!
//! # Why the encoder is here rather than in the renderer
//!
//! Compression is an asset-pipeline step: it turns one representation of an image into
//! another and needs no device, no queue and no descriptor. Keeping it in this crate means
//! it is unit-tested without a GPU, and it is the crate a future offline asset baker would
//! call. The renderer decides *when* to compress, because only the renderer knows a texture's
//! role and has already built its mip chain; this module decides *how*.

/// Texels along each edge of a block, for both formats.
pub const BLOCK_SIZE: u32 = 4;

/// Bytes one BC1 block occupies.
pub const BC1_BLOCK_BYTES: usize = 8;

/// Bytes one BC5 block occupies: two BC4 blocks, one per channel.
pub const BC5_BLOCK_BYTES: usize = 16;

/// How many blocks cover `width` x `height` texels.
///
/// Rounded up, which is what makes a 1x1 image legal: it occupies one block whose other
/// fifteen texels are padding. D3D12 sizes a compressed subresource the same way, so a mip
/// chain that ends at 2x2 and 1x1 is not a special case for either side.
#[must_use]
pub fn block_counts(width: u32, height: u32) -> (u32, u32) {
    (width.div_ceil(BLOCK_SIZE), height.div_ceil(BLOCK_SIZE))
}

/// Bytes a BC1 image of this size occupies.
#[must_use]
pub fn bc1_size(width: u32, height: u32) -> usize {
    let (blocks_x, blocks_y) = block_counts(width, height);
    blocks_x as usize * blocks_y as usize * BC1_BLOCK_BYTES
}

/// Bytes a BC5 image of this size occupies.
#[must_use]
pub fn bc5_size(width: u32, height: u32) -> usize {
    let (blocks_x, blocks_y) = block_counts(width, height);
    blocks_x as usize * blocks_y as usize * BC5_BLOCK_BYTES
}

/// Compresses RGBA8 texels to BC1, discarding alpha.
///
/// The caller has already decided alpha does not matter — see the module documentation for
/// why that decision is not made here.
#[must_use]
pub fn compress_bc1(texels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (_, blocks_y) = block_counts(width, height);
    let mut out = vec![0_u8; bc1_size(width, height)];
    compress_bc1_rows(texels, width, height, 0..blocks_y, &mut out);
    out
}

/// [`compress_bc1`] for the block rows `block_rows` only, written into `out`.
///
/// Every block is encoded from its own sixteen texels and nothing else, so an image can be
/// cut into bands of block rows, the bands compressed on different threads, and the result
/// is byte for byte what [`compress_bc1`] produces — which is what the renderer does with a
/// large texture at load. `out` is exactly the bands' bytes: `block_rows.len()` rows of
/// `block_counts(width, height).0` blocks of [`BC1_BLOCK_BYTES`].
///
/// # Panics
///
/// Panics if `out` is not that length.
pub fn compress_bc1_rows(
    texels: &[u8],
    width: u32,
    height: u32,
    block_rows: std::ops::Range<u32>,
    out: &mut [u8],
) {
    let (blocks_x, _) = block_counts(width, height);
    let row_bytes = blocks_x as usize * BC1_BLOCK_BYTES;
    assert_eq!(
        out.len(),
        block_rows.len() * row_bytes,
        "the output is the bands' bytes"
    );
    for (row, block_y) in out.chunks_exact_mut(row_bytes).zip(block_rows) {
        for (slot, block_x) in row.chunks_exact_mut(BC1_BLOCK_BYTES).zip(0..blocks_x) {
            let block = gather(texels, width, height, block_x, block_y);
            slot.copy_from_slice(&encode_bc1(&block));
        }
    }
}

/// Compresses the red and green channels of RGBA8 texels to BC5.
///
/// For a tangent-space normal map: X and Y are stored and Z is reconstructed in the shader
/// as `sqrt(1 - x^2 - y^2)`, which is exact for a unit normal and is what makes throwing the
/// blue channel away lossless in the only sense that matters.
#[must_use]
pub fn compress_bc5(texels: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (_, blocks_y) = block_counts(width, height);
    let mut out = vec![0_u8; bc5_size(width, height)];
    compress_bc5_rows(texels, width, height, 0..blocks_y, &mut out);
    out
}

/// [`compress_bc5`] for the block rows `block_rows` only, written into `out`. See
/// [`compress_bc1_rows`]: the same contract, with [`BC5_BLOCK_BYTES`] per block.
///
/// # Panics
///
/// Panics if `out` is not `block_rows.len()` rows of blocks.
pub fn compress_bc5_rows(
    texels: &[u8],
    width: u32,
    height: u32,
    block_rows: std::ops::Range<u32>,
    out: &mut [u8],
) {
    let (blocks_x, _) = block_counts(width, height);
    let row_bytes = blocks_x as usize * BC5_BLOCK_BYTES;
    assert_eq!(
        out.len(),
        block_rows.len() * row_bytes,
        "the output is the bands' bytes"
    );
    for (row, block_y) in out.chunks_exact_mut(row_bytes).zip(block_rows) {
        for (slot, block_x) in row.chunks_exact_mut(BC5_BLOCK_BYTES).zip(0..blocks_x) {
            let block = gather(texels, width, height, block_x, block_y);
            let red: [u8; 16] = std::array::from_fn(|index| block[index][0]);
            let green: [u8; 16] = std::array::from_fn(|index| block[index][1]);
            slot[..8].copy_from_slice(&encode_bc4(&red));
            slot[8..].copy_from_slice(&encode_bc4(&green));
        }
    }
}

/// The sixteen texels of one block, clamped at the edges of a partial block.
///
/// Clamping rather than zero-filling: a block on the right edge of a 5-texel-wide image has
/// three real columns and one padded one, and repeating the last real column keeps the
/// endpoints describing the texels that exist. Zeros would drag every endpoint towards black.
fn gather(texels: &[u8], width: u32, height: u32, block_x: u32, block_y: u32) -> [[u8; 4]; 16] {
    let mut block = [[0_u8; 4]; 16];
    for row in 0..BLOCK_SIZE {
        for column in 0..BLOCK_SIZE {
            let x = (block_x * BLOCK_SIZE + column).min(width - 1);
            let y = (block_y * BLOCK_SIZE + row).min(height - 1);
            let at = (y as usize * width as usize + x as usize) * 4;
            let index = (row * BLOCK_SIZE + column) as usize;
            block[index] = [texels[at], texels[at + 1], texels[at + 2], texels[at + 3]];
        }
    }
    block
}

/// Encodes one 4x4 block as BC1.
///
/// The endpoints are the two texels furthest apart in RGB, found by comparing all 120 pairs.
///
/// The obvious cheaper choice — the corners of the block's bounding box — is wrong whenever
/// two channels move in opposite directions: a block whose red rises as its blue falls has a
/// bounding box whose corners are (high red, high blue) and (low red, low blue), and neither
/// corner is anywhere near the data. The symptom is a badly wrong colour on exactly the
/// blocks that carry a hue transition, which is where the eye is looking. A covariance fit is
/// the textbook answer; the furthest pair costs 120 comparisons on a sixteen-element array at
/// load time and lands close enough that the difference is not visible.
fn encode_bc1(block: &[[u8; 4]; 16]) -> [u8; BC1_BLOCK_BYTES] {
    let mut high = [block[0][0], block[0][1], block[0][2]];
    let mut low = high;
    let mut furthest = 0_u32;
    for (index, first) in block.iter().enumerate() {
        for second in &block[index + 1..] {
            let a = [first[0], first[1], first[2]];
            let b = [second[0], second[1], second[2]];
            let distance = squared_distance(a, b);
            if distance > furthest {
                furthest = distance;
                high = a;
                low = b;
            }
        }
    }

    let mut color0 = pack565(high);
    let mut color1 = pack565(low);
    // BC1's four-colour mode is selected by `color0 > color1`; the other ordering means
    // three colours and a transparent slot, which this encoder never wants. Equal endpoints
    // — a block of one colour, which is most of a stylized texture — are left as they are,
    // and the palette's four entries all decode to that colour.
    if color0 < color1 {
        std::mem::swap(&mut color0, &mut color1);
    }

    let palette = [
        unpack565(color0),
        unpack565(color1),
        lerp565(unpack565(color0), unpack565(color1), 1, 3),
        lerp565(unpack565(color0), unpack565(color1), 2, 3),
    ];

    let mut indices = 0_u32;
    for (index, texel) in block.iter().enumerate() {
        let mut best = 0_u32;
        let mut best_error = u32::MAX;
        for (candidate, entry) in palette.iter().enumerate() {
            let error = squared_distance(*entry, [texel[0], texel[1], texel[2]]);
            if error < best_error {
                best_error = error;
                best = candidate as u32;
            }
        }
        indices |= best << (index * 2);
    }

    let mut out = [0_u8; BC1_BLOCK_BYTES];
    out[0..2].copy_from_slice(&color0.to_le_bytes());
    out[2..4].copy_from_slice(&color1.to_le_bytes());
    out[4..8].copy_from_slice(&indices.to_le_bytes());
    out
}

/// Encodes one 4x4 block of a single channel as BC4, which is half of BC5.
fn encode_bc4(block: &[u8; 16]) -> [u8; 8] {
    let low = *block.iter().min().unwrap_or(&0);
    let high = *block.iter().max().unwrap_or(&0);

    // The eight-value mode: endpoints and six interpolated steps, selected by `red0 > red1`.
    // A flat block takes the equal-endpoint path, where every index decodes to the same
    // value and the choice of mode does not matter.
    let (red0, red1) = (high, low);
    let mut out = [0_u8; 8];
    out[0] = red0;
    out[1] = red1;

    let span = f32::from(red0) - f32::from(red1);
    let mut bits = 0_u64;
    for (index, value) in block.iter().enumerate() {
        // Index 0 is red0 and index 1 is red1; 2..=7 are the six steps between them, which
        // is why the arithmetic below is not simply "scale into 0..7".
        let selector = if span <= 0.0 {
            0_u64
        } else {
            let position = (f32::from(*value) - f32::from(red1)) / span;
            let step = (position * 7.0).round().clamp(0.0, 7.0) as u64;
            match step {
                7 => 0,
                0 => 1,
                other => 8 - other,
            }
        };
        bits |= selector << (index * 3);
    }
    out[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    out
}

/// Packs an RGB8 colour into RGB565.
fn pack565(color: [u8; 3]) -> u16 {
    let red = u16::from(color[0]) >> 3;
    let green = u16::from(color[1]) >> 2;
    let blue = u16::from(color[2]) >> 3;
    (red << 11) | (green << 5) | blue
}

/// Unpacks RGB565 back to RGB8, replicating the high bits into the low ones the way the
/// hardware decoder does.
fn unpack565(packed: u16) -> [u8; 3] {
    let red = ((packed >> 11) & 0x1F) as u8;
    let green = ((packed >> 5) & 0x3F) as u8;
    let blue = (packed & 0x1F) as u8;
    [
        (red << 3) | (red >> 2),
        (green << 2) | (green >> 4),
        (blue << 3) | (blue >> 2),
    ]
}

/// One of BC1's interpolated palette entries: `(a * (3 - step) + b * step) / 3`.
fn lerp565(a: [u8; 3], b: [u8; 3], step: u16, over: u16) -> [u8; 3] {
    let mut out = [0_u8; 3];
    for channel in 0..3 {
        let value = (u16::from(a[channel]) * (over - step) + u16::from(b[channel]) * step) / over;
        out[channel] = value as u8;
    }
    out
}

/// Squared distance between two colours, for choosing a palette entry.
fn squared_distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    let mut total = 0_u32;
    for channel in 0..3 {
        let difference = i32::from(a[channel]) - i32::from(b[channel]);
        total += (difference * difference) as u32;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a BC1 block the way the hardware does, so the tests can measure the error the
    /// GPU will actually see rather than the error the encoder believes it produced.
    fn decode_bc1(block: &[u8]) -> [[u8; 3]; 16] {
        let color0 = u16::from_le_bytes([block[0], block[1]]);
        let color1 = u16::from_le_bytes([block[2], block[3]]);
        let indices = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        let palette = [
            unpack565(color0),
            unpack565(color1),
            lerp565(unpack565(color0), unpack565(color1), 1, 3),
            lerp565(unpack565(color0), unpack565(color1), 2, 3),
        ];
        std::array::from_fn(|index| palette[((indices >> (index * 2)) & 0x3) as usize])
    }

    /// Decodes one BC4 half-block.
    fn decode_bc4(block: &[u8]) -> [u8; 16] {
        let red0 = f32::from(block[0]);
        let red1 = f32::from(block[1]);
        let mut bits = 0_u64;
        for (index, byte) in block[2..8].iter().enumerate() {
            bits |= u64::from(*byte) << (index * 8);
        }
        std::array::from_fn(|index| {
            let selector = (bits >> (index * 3)) & 0x7;
            let value = match selector {
                0 => red0,
                1 => red1,
                other => {
                    let step = 8.0 - other as f32;
                    (red1 * (7.0 - step) + red0 * step) / 7.0
                }
            };
            value.round().clamp(0.0, 255.0) as u8
        })
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        color
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect()
    }

    #[test]
    fn a_compressed_image_is_the_size_the_format_says() {
        assert_eq!(bc1_size(4, 4), 8);
        assert_eq!(bc5_size(4, 4), 16);
        // A quarter of RGBA8 for BC5 and an eighth for BC1, which is the whole point.
        assert_eq!(bc1_size(256, 256), 256 * 256 / 2);
        assert_eq!(bc5_size(256, 256), 256 * 256);
        // Partial blocks round up, so the smallest mips of a chain are legal sizes.
        assert_eq!(bc1_size(1, 1), 8);
        assert_eq!(bc1_size(5, 5), 4 * 8);
        assert_eq!(compress_bc1(&solid(1, 1, [9, 9, 9, 255]), 1, 1).len(), 8);
        assert_eq!(compress_bc5(&solid(2, 2, [9, 9, 9, 255]), 2, 2).len(), 16);
    }

    #[test]
    fn a_flat_block_comes_back_almost_exactly() {
        // The common case in this engine's stylized art, and the one where any error is a
        // visible band across a whole surface. BC1 quantizes to 5:6:5, so "almost" is one
        // step of that quantization and no more.
        let color = [200_u8, 120, 60, 255];
        let compressed = compress_bc1(&solid(4, 4, color), 4, 4);
        for decoded in decode_bc1(&compressed) {
            for channel in 0..3 {
                assert!(
                    decoded[channel].abs_diff(color[channel]) <= 8,
                    "flat block: {decoded:?} against {color:?}"
                );
            }
        }
    }

    #[test]
    fn bc1_follows_a_gradient_along_one_axis_closely() {
        // What BC1 is actually good at, and what most of a texture is: colours that lie near
        // a line. The palette is four points on the segment between the endpoints, so a ramp
        // comes back within a step of that segment.
        //
        // The case BC1 is *not* good at — sixteen colours spread across a plane rather than a
        // line — is the next test, and its error is the format's, not the encoder's.
        let mut texels = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                let value = ((y * 4 + x) * 16) as u8;
                texels.extend_from_slice(&[value, value / 2, 255 - value, 255]);
            }
        }
        let compressed = compress_bc1(&texels, 4, 4);
        let decoded = decode_bc1(&compressed);
        for (index, entry) in decoded.iter().enumerate() {
            let source = &texels[index * 4..index * 4 + 3];
            for channel in 0..3 {
                // Four palette entries across a sixteen-step ramp is a quantization step of
                // about 80, so half a step is the floor on the error here whatever the
                // encoder does. An encoder that chose the wrong entry lands well outside it.
                assert!(
                    entry[channel].abs_diff(source[channel]) <= 45,
                    "texel {index}: {entry:?} against {source:?}"
                );
            }
        }
    }

    #[test]
    fn bc1_degrades_predictably_where_a_block_is_not_a_line() {
        // Red along X and green along Y: sixteen colours filling a plane, which a four-entry
        // palette on one line cannot represent whatever the encoder does. The bound here is
        // loose deliberately — it is the *format's* limit, and the test exists so that a real
        // encoder bug, which lands far outside it, cannot hide behind "BC1 is lossy".
        //
        // This is also why the renderer compresses base colour and refuses to compress ORM:
        // a shade of colour off by this much is invisible in stylized art, and a roughness
        // off by this much is a different material.
        let mut texels = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                texels.extend_from_slice(&[(x * 60) as u8, (y * 60) as u8, 128, 255]);
            }
        }
        let compressed = compress_bc1(&texels, 4, 4);
        let decoded = decode_bc1(&compressed);
        let mut total = 0_u32;
        for (index, entry) in decoded.iter().enumerate() {
            let source = &texels[index * 4..index * 4 + 3];
            for channel in 0..3 {
                total += u32::from(entry[channel].abs_diff(source[channel]));
                // No texel may come back wildly wrong even here: a palette index chosen at
                // random rather than by distance would blow through this immediately.
                assert!(
                    entry[channel].abs_diff(source[channel]) <= 128,
                    "texel {index}: {entry:?} against {source:?}"
                );
            }
        }
        let mean = f64::from(total) / 48.0;
        assert!(
            mean < 45.0,
            "the average channel error across a plane-filling block should stay moderate,              got {mean:.1}"
        );
    }

    #[test]
    fn bc5_keeps_the_two_channels_it_stores() {
        // Eight bits of endpoint per channel and three bits of index, so a smooth ramp comes
        // back close. This is why normals get BC5 and not BC1: the same data through BC1's
        // shared endpoints is several times worse.
        let mut texels = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                texels.extend_from_slice(&[(x * 40 + 60) as u8, (y * 40 + 60) as u8, 255, 255]);
            }
        }
        let compressed = compress_bc5(&texels, 4, 4);
        let red = decode_bc4(&compressed[0..8]);
        let green = decode_bc4(&compressed[8..16]);
        for index in 0..16 {
            assert!(
                red[index].abs_diff(texels[index * 4]) <= 12,
                "red {index}: {} against {}",
                red[index],
                texels[index * 4]
            );
            assert!(
                green[index].abs_diff(texels[index * 4 + 1]) <= 12,
                "green {index}: {} against {}",
                green[index],
                texels[index * 4 + 1]
            );
        }
    }

    #[test]
    fn a_flat_normal_survives_bc5_exactly() {
        // The texel every unmapped surface samples and every flat region of a real normal map
        // is made of. It has to come back exactly 128, because the shader's decode puts zero
        // there and anything else tilts every flat surface in the scene.
        let compressed = compress_bc5(&solid(4, 4, [128, 128, 255, 255]), 4, 4);
        let red = decode_bc4(&compressed[0..8]);
        let green = decode_bc4(&compressed[8..16]);
        assert_eq!(red, [128_u8; 16]);
        assert_eq!(green, [128_u8; 16]);
    }

    #[test]
    fn bands_compressed_separately_are_the_whole_image_compressed_at_once() {
        // A gradient with a hue turn in it, odd-sized so the last block row and column are
        // partial: every kind of block the bands could get wrong.
        let (width, height) = (37_u32, 29_u32);
        let texels: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let (x, y) = (index % width, index / width);
                [(x * 7) as u8, (y * 9) as u8, ((x + y) * 3) as u8, 255]
            })
            .collect();
        let (blocks_x, blocks_y) = block_counts(width, height);
        for (whole, size, compress) in [
            (
                compress_bc1(&texels, width, height),
                BC1_BLOCK_BYTES,
                compress_bc1_rows as fn(&[u8], u32, u32, std::ops::Range<u32>, &mut [u8]),
            ),
            (
                compress_bc5(&texels, width, height),
                BC5_BLOCK_BYTES,
                compress_bc5_rows,
            ),
        ] {
            let row = blocks_x as usize * size;
            let mut banded = vec![0_u8; whole.len()];
            let mut start = 0;
            for band in [2_u32, 1, 3, blocks_y - 6] {
                let rows = start..start + band;
                let bytes = &mut banded[start as usize * row..(start + band) as usize * row];
                compress(&texels, width, height, rows, bytes);
                start += band;
            }
            assert_eq!(start, blocks_y);
            assert_eq!(banded, whole);
        }
    }

    #[test]
    fn compression_is_deterministic() {
        // The same image must produce the same bytes every time, or content-hash identity
        // downstream would see two textures where there is one.
        let texels = solid(8, 8, [30, 90, 200, 255]);
        assert_eq!(
            compress_bc1(&texels, 8, 8),
            compress_bc1(&texels.clone(), 8, 8)
        );
        assert_eq!(
            compress_bc5(&texels, 8, 8),
            compress_bc5(&texels.clone(), 8, 8)
        );
    }

    #[test]
    fn an_image_smaller_than_a_block_clamps_rather_than_padding_with_black() {
        // A 2x2 image is one block with twelve padded texels. Padding with zeros would drag
        // the endpoints towards black and darken the real four; clamping repeats them.
        let compressed = compress_bc1(&solid(2, 2, [220, 220, 220, 255]), 2, 2);
        for decoded in decode_bc1(&compressed) {
            assert!(decoded[0] > 200, "{decoded:?}");
        }
    }
}
