//! A glyph atlas: the 5x7 bitmap font rasterized once into a single texture.
//!
//! ```text
//! font::glyph (bit patterns)  -->  FontAtlas (one R8 texture)  -->  one quad per glyph
//! ```
//!
//! ## Why this exists
//!
//! Phase 3 drew text as **one quad per lit font cell**. A character averages fifteen lit
//! cells, so a twenty-two character nameplate title is roughly 330 quads, and twenty plates
//! are about 1.1 MB of vertices generated and uploaded every frame. Measured in Milestone 2:
//! twenty plates cost 0.15-0.23 ms of CPU and pushed CPU p95 from 0.26 ms to 0.97 ms.
//!
//! With an atlas a glyph is **one quad with a texture coordinate**, so the same title is 22
//! quads rather than 330 — a 15x reduction in generated geometry, and the difference between
//! a UI that fits a 2.083 ms budget and one that does not.
//!
//! ## Layout
//!
//! Printable ASCII, 32 (`' '`) to 126 (`'~'`), plus one cell for everything else:
//!
//! ```text
//! cell index = character.to_ascii_uppercase() as usize - 32,  or UNKNOWN_CELL
//! column     = index % COLUMNS
//! row        = index / COLUMNS
//! ```
//!
//! 96 cells in a 32x3 grid. Each cell is 7x9 texels: the 5x7 glyph plus a one-texel border
//! on every side, so bilinear sampling at a cell edge cannot pull in the neighbouring
//! glyph — the classic atlas bleed, and the reason cells are padded rather than packed.
//!
//! The atlas is **256x32, R8_UNORM, 8 KiB**. 256 is not the tightest width — 32 columns of 7
//! texels need only 224 — but a D3D12 texture upload requires a 256-byte-aligned row pitch,
//! and choosing the width to match means the upload is a straight copy with no per-row
//! padding path to get wrong.
//!
//! Lookup is a **pure function of the character**: no state, no cache, no insertion. Two
//! atlases built in the same process are byte-identical, and asking for the same glyph twice
//! cannot produce two answers.

use crate::font::{GLYPH_HEIGHT, GLYPH_WIDTH, glyph};

/// Cells across the atlas.
pub const COLUMNS: usize = 32;

/// Cells down the atlas.
pub const ROWS: usize = 3;

/// Texels of padding around each glyph inside its cell.
///
/// One is enough: a bilinear sample at the edge of a glyph reaches half a texel past it, and
/// a one-texel transparent border keeps that half-texel inside the same cell.
pub const CELL_PADDING: usize = 1;

/// Width of one cell, in texels.
pub const CELL_WIDTH: usize = GLYPH_WIDTH + CELL_PADDING * 2;

/// Height of one cell, in texels.
pub const CELL_HEIGHT: usize = GLYPH_HEIGHT + CELL_PADDING * 2;

/// Atlas width in texels.
///
/// 256 rather than the 224 the columns need, so the row pitch already satisfies D3D12's
/// 256-byte upload alignment for an 8-bit format.
pub const ATLAS_WIDTH: usize = 256;

/// Atlas height in texels.
pub const ATLAS_HEIGHT: usize = 32;

/// The first character with a cell.
const FIRST_CHARACTER: u32 = 32;

/// The last character with a cell.
const LAST_CHARACTER: u32 = 126;

/// The cell every unsupported character resolves to.
///
/// A hollow box, so a missing glyph is *visible*. A blank would make a font bug look like a
/// content bug.
pub const UNKNOWN_CELL: usize = (LAST_CHARACTER - FIRST_CHARACTER + 1) as usize;

/// Cells the atlas holds, including the unknown box.
pub const CELL_COUNT: usize = UNKNOWN_CELL + 1;

/// Texel coordinates of a single fully-opaque texel, in the atlas's bottom-right corner.
///
/// This is what makes a solid rectangle and a line of text the **same draw call**: a rect
/// samples this texel, a glyph samples its own, and neither needs a different texture, a
/// different pipeline state or a different batch. Without it, every panel behind every
/// health bar would break the batch in two.
const SOLID_TEXEL: (usize, usize) = (ATLAS_WIDTH - 2, ATLAS_HEIGHT - 2);

/// Where one glyph sits in the atlas, in texture coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphUv {
    /// Left edge.
    pub u0: f32,
    /// Top edge.
    pub v0: f32,
    /// Right edge.
    pub u1: f32,
    /// Bottom edge.
    pub v1: f32,
}

impl GlyphUv {
    /// Width in texture coordinates.
    #[must_use]
    pub fn width(self) -> f32 {
        self.u1 - self.u0
    }

    /// Height in texture coordinates.
    #[must_use]
    pub fn height(self) -> f32 {
        self.v1 - self.v0
    }
}

/// The rasterized font, as texels plus a lookup.
///
/// Cheap to build (8 KiB written once) and cheap to hold. Built at renderer creation and
/// uploaded once; nothing about it changes per frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontAtlas {
    /// `ATLAS_WIDTH * ATLAS_HEIGHT` coverage values, 0 or 255.
    ///
    /// Not anti-aliased: the source is a bit pattern, and inventing intermediate coverage
    /// would blur a font designed to be crisp at one size.
    texels: Vec<u8>,
}

impl Default for FontAtlas {
    fn default() -> Self {
        Self::new()
    }
}

impl FontAtlas {
    /// Rasterizes every glyph into a new atlas.
    #[must_use]
    pub fn new() -> Self {
        let mut texels = vec![0u8; ATLAS_WIDTH * ATLAS_HEIGHT];

        for cell in 0..CELL_COUNT {
            let pattern = match cell_character(cell) {
                Some(character) => glyph(character),
                // The unknown cell has no character; `glyph` produces its box for any
                // unsupported one, and `\u{7f}` (DEL) is exactly that.
                None => glyph('\u{7f}'),
            };
            let Some(pattern) = pattern else {
                // The space character: a cell that exists and draws nothing.
                continue;
            };

            let (origin_x, origin_y) = cell_origin(cell);
            for (row, bits) in pattern.iter().enumerate() {
                for column in 0..GLYPH_WIDTH {
                    // Most significant bit leftmost, matching the font's own convention.
                    let lit = bits & (1 << (GLYPH_WIDTH - 1 - column)) != 0;
                    if !lit {
                        continue;
                    }
                    let x = origin_x + CELL_PADDING + column;
                    let y = origin_y + CELL_PADDING + row;
                    texels[y * ATLAS_WIDTH + x] = 255;
                }
            }
        }

        // One opaque texel so an untextured rectangle shares the text batch. It sits in the
        // padding row past the last cell, which no glyph can reach.
        texels[SOLID_TEXEL.1 * ATLAS_WIDTH + SOLID_TEXEL.0] = 255;

        Self { texels }
    }

    /// Texture coordinates of a fully-opaque texel.
    ///
    /// A rectangle samples this and comes out solid, which is what lets panels, bars and
    /// text all be recorded as one batch against one texture.
    #[must_use]
    pub fn solid_uv() -> GlyphUv {
        // The centre of the texel, not its corner: sampling exactly on a boundary is what
        // makes a filter pick up the neighbour.
        let u = (SOLID_TEXEL.0 as f32 + 0.5) / ATLAS_WIDTH as f32;
        let v = (SOLID_TEXEL.1 as f32 + 0.5) / ATLAS_HEIGHT as f32;
        GlyphUv {
            u0: u,
            v0: v,
            u1: u,
            v1: v,
        }
    }

    /// The atlas texels, row-major, one byte each.
    #[must_use]
    pub fn texels(&self) -> &[u8] {
        &self.texels
    }

    /// Atlas width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        ATLAS_WIDTH as u32
    }

    /// Atlas height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        ATLAS_HEIGHT as u32
    }

    /// Bytes between the start of one texel row and the next.
    #[must_use]
    pub fn row_pitch(&self) -> usize {
        ATLAS_WIDTH
    }

    /// One texel, for tests and for proving the rasterization landed where it should.
    #[must_use]
    pub fn texel(&self, x: usize, y: usize) -> Option<u8> {
        if x >= ATLAS_WIDTH || y >= ATLAS_HEIGHT {
            return None;
        }
        self.texels.get(y * ATLAS_WIDTH + x).copied()
    }

    /// Where a character's glyph sits in the atlas.
    ///
    /// A method for convenience; the answer does not depend on `self`, and
    /// [`FontAtlas::glyph_uv`] is the same function without one.
    #[must_use]
    pub fn uv(&self, character: char) -> GlyphUv {
        Self::glyph_uv(character)
    }

    /// Where a character's glyph sits, as a pure function of the character.
    ///
    /// Unsupported characters resolve to the hollow box rather than failing, so text is
    /// always drawable and a missing glyph is visible on screen.
    #[must_use]
    pub fn glyph_uv(character: char) -> GlyphUv {
        let cell = cell_index(character);
        let (origin_x, origin_y) = cell_origin(cell);
        let x0 = (origin_x + CELL_PADDING) as f32;
        let y0 = (origin_y + CELL_PADDING) as f32;
        GlyphUv {
            u0: x0 / ATLAS_WIDTH as f32,
            v0: y0 / ATLAS_HEIGHT as f32,
            u1: (x0 + GLYPH_WIDTH as f32) / ATLAS_WIDTH as f32,
            v1: (y0 + GLYPH_HEIGHT as f32) / ATLAS_HEIGHT as f32,
        }
    }

    /// Whether a character has anything to draw.
    ///
    /// Only the space does not. A text builder skips those rather than emitting an empty
    /// quad, which is the cheapest possible handling of the most common character.
    #[must_use]
    pub fn is_blank(character: char) -> bool {
        character == ' '
    }
}

/// Which cell a character occupies.
///
/// Letters are upper-cased first, because the font defines only uppercase forms — `'a'` and
/// `'A'` are the same glyph and must be the same cell, or the atlas would hold two copies of
/// one bitmap and disagree about which is authoritative.
#[must_use]
pub fn cell_index(character: char) -> usize {
    let upper = character.to_ascii_uppercase();
    let code = upper as u32;
    if (FIRST_CHARACTER..=LAST_CHARACTER).contains(&code) && upper.is_ascii() {
        (code - FIRST_CHARACTER) as usize
    } else {
        UNKNOWN_CELL
    }
}

/// The character a cell holds, or `None` for the unknown cell.
fn cell_character(cell: usize) -> Option<char> {
    if cell >= UNKNOWN_CELL {
        return None;
    }
    char::from_u32(FIRST_CHARACTER + cell as u32)
}

/// The top-left texel of a cell.
fn cell_origin(cell: usize) -> (usize, usize) {
    let column = cell % COLUMNS;
    let row = cell / COLUMNS;
    (column * CELL_WIDTH, row * CELL_HEIGHT)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn the_atlas_fits_its_texture_and_its_row_pitch_is_upload_aligned() {
        let atlas = FontAtlas::new();
        assert_eq!(atlas.texels().len(), ATLAS_WIDTH * ATLAS_HEIGHT);
        assert_eq!(atlas.texels().len(), 8 * 1024, "8 KiB, uploaded once");
        // D3D12 requires a 256-byte-aligned row pitch for a texture upload. Choosing the
        // width to satisfy it is what makes the upload a straight copy.
        assert_eq!(atlas.row_pitch() % 256, 0);

        // Every cell must land inside the texture.
        for cell in 0..CELL_COUNT {
            let (x, y) = cell_origin(cell);
            assert!(x + CELL_WIDTH <= ATLAS_WIDTH, "cell {cell} overflows right");
            assert!(
                y + CELL_HEIGHT <= ATLAS_HEIGHT,
                "cell {cell} overflows bottom"
            );
        }
        assert_eq!(CELL_COUNT, COLUMNS * ROWS, "the grid is exactly filled");
    }

    #[test]
    fn every_printable_character_gets_its_own_cell() {
        let mut seen = std::collections::HashMap::new();
        for code in FIRST_CHARACTER..=LAST_CHARACTER {
            let character = char::from_u32(code).expect("ASCII");
            // Lowercase deliberately shares its uppercase cell; skip it in the uniqueness
            // check and assert the sharing separately.
            if character.is_ascii_lowercase() {
                continue;
            }
            let cell = cell_index(character);
            assert!(cell < CELL_COUNT);
            assert!(
                seen.insert(cell, character).is_none(),
                "{character:?} collides with {:?}",
                seen.get(&cell)
            );
        }
    }

    #[test]
    fn lowercase_shares_the_uppercase_cell() {
        // The font defines uppercase forms only. Two cells holding one bitmap would be two
        // answers to the same question.
        for (lower, upper) in ('a'..='z').zip('A'..='Z') {
            assert_eq!(cell_index(lower), cell_index(upper));
            assert_eq!(FontAtlas::glyph_uv(lower), FontAtlas::glyph_uv(upper));
        }
    }

    #[test]
    fn an_unsupported_character_resolves_to_a_visible_box_rather_than_a_blank() {
        // A blank would make a missing glyph look like missing content.
        let atlas = FontAtlas::new();
        for exotic in ['\u{20ac}', '\u{4e2d}', '\u{1f600}', '\u{7f}', '\u{1}'] {
            assert_eq!(cell_index(exotic), UNKNOWN_CELL, "{exotic:?}");
        }

        let uv = FontAtlas::glyph_uv('\u{20ac}');
        let (x, y) = cell_origin(UNKNOWN_CELL);
        let lit = (0..GLYPH_HEIGHT)
            .flat_map(|row| (0..GLYPH_WIDTH).map(move |column| (row, column)))
            .filter(|(row, column)| {
                atlas
                    .texel(x + CELL_PADDING + column, y + CELL_PADDING + row)
                    .is_some_and(|value| value > 0)
            })
            .count();
        assert!(lit > 0, "the unknown cell must draw something");
        assert!(uv.width() > 0.0 && uv.height() > 0.0);
    }

    #[test]
    fn the_space_cell_exists_and_is_empty() {
        // Space has a cell so lookup never fails, and draws nothing so a text builder can
        // skip it. Both properties matter: the first for safety, the second for cost.
        let atlas = FontAtlas::new();
        assert!(FontAtlas::is_blank(' '));
        assert_eq!(cell_index(' '), 0);

        let (x, y) = cell_origin(cell_index(' '));
        for row in 0..CELL_HEIGHT {
            for column in 0..CELL_WIDTH {
                assert_eq!(atlas.texel(x + column, y + row), Some(0));
            }
        }
    }

    #[test]
    fn a_glyph_is_rasterized_where_its_bit_pattern_says() {
        // The one test that proves the atlas is the font rather than a picture of one.
        let atlas = FontAtlas::new();
        let pattern = glyph('A').expect("A has a glyph");
        let (x, y) = cell_origin(cell_index('A'));

        for (row, bits) in pattern.iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                let expected = if bits & (1 << (GLYPH_WIDTH - 1 - column)) != 0 {
                    255
                } else {
                    0
                };
                assert_eq!(
                    atlas.texel(x + CELL_PADDING + column, y + CELL_PADDING + row),
                    Some(expected),
                    "A at row {row} column {column}"
                );
            }
        }
    }

    #[test]
    fn every_cell_keeps_a_transparent_border_so_glyphs_cannot_bleed() {
        // Bilinear sampling at a glyph edge reaches half a texel past it. Without the
        // border it would reach into the neighbouring glyph, which is the classic atlas
        // artefact.
        let atlas = FontAtlas::new();
        for cell in 0..CELL_COUNT {
            let (x, y) = cell_origin(cell);
            for column in 0..CELL_WIDTH {
                assert_eq!(atlas.texel(x + column, y), Some(0), "cell {cell} top");
                assert_eq!(
                    atlas.texel(x + column, y + CELL_HEIGHT - 1),
                    Some(0),
                    "cell {cell} bottom"
                );
            }
            for row in 0..CELL_HEIGHT {
                assert_eq!(atlas.texel(x, y + row), Some(0), "cell {cell} left");
                assert_eq!(
                    atlas.texel(x + CELL_WIDTH - 1, y + row),
                    Some(0),
                    "cell {cell} right"
                );
            }
        }
    }

    #[test]
    fn texture_coordinates_stay_inside_the_atlas_and_are_never_degenerate() {
        for code in FIRST_CHARACTER..=LAST_CHARACTER {
            let character = char::from_u32(code).expect("ASCII");
            let uv = FontAtlas::glyph_uv(character);
            assert!((0.0..=1.0).contains(&uv.u0), "{character:?}");
            assert!((0.0..=1.0).contains(&uv.v0), "{character:?}");
            assert!((0.0..=1.0).contains(&uv.u1), "{character:?}");
            assert!((0.0..=1.0).contains(&uv.v1), "{character:?}");
            assert!(
                uv.u1 > uv.u0 && uv.v1 > uv.v0,
                "{character:?} is degenerate"
            );
            assert!(
                (uv.width() - GLYPH_WIDTH as f32 / ATLAS_WIDTH as f32).abs() < 1e-6,
                "{character:?} is not one glyph wide"
            );
        }
    }

    #[test]
    fn lookup_is_stable_across_atlases_and_repeated_calls() {
        // "Predictable glyph lookup": no state, no insertion order, no cache. Repeated text
        // must not produce a different atlas or a different coordinate.
        let first = FontAtlas::new();
        let second = FontAtlas::new();
        assert_eq!(first, second, "two atlases must be byte-identical");

        let text = "DUMMY 100% BURNING 4.5 DUMMY 100%";
        let once: Vec<_> = text.chars().map(FontAtlas::glyph_uv).collect();
        let twice: Vec<_> = text.chars().map(FontAtlas::glyph_uv).collect();
        assert_eq!(once, twice);
        assert_eq!(
            FontAtlas::glyph_uv('D'),
            first.uv('D'),
            "the method and the function agree"
        );
    }

    #[test]
    fn a_solid_texel_lets_rectangles_share_the_text_batch() {
        // Without this, every panel behind every bar would break the batch in two.
        let atlas = FontAtlas::new();
        assert_eq!(atlas.texel(SOLID_TEXEL.0, SOLID_TEXEL.1), Some(255));

        let uv = FontAtlas::solid_uv();
        assert!((0.0..=1.0).contains(&uv.u0) && (0.0..=1.0).contains(&uv.v0));
        assert_eq!(uv.width(), 0.0, "a point sample, not a region");
        assert_eq!(uv.height(), 0.0);

        // It must not sit inside any glyph cell, or a rectangle would tint itself with a
        // letter.
        for cell in 0..CELL_COUNT {
            let (x, y) = cell_origin(cell);
            let inside = (x..x + CELL_WIDTH).contains(&SOLID_TEXEL.0)
                && (y..y + CELL_HEIGHT).contains(&SOLID_TEXEL.1);
            assert!(!inside, "the solid texel is inside cell {cell}");
        }
    }

    #[test]
    fn every_character_the_ui_emits_has_a_cell_that_is_not_the_unknown_box() {
        // The same guarantee the Phase 3 overlay tests made, restated against the atlas.
        const ALLOWED: &str = " 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        for character in ALLOWED.chars() {
            assert_ne!(
                cell_index(character),
                UNKNOWN_CELL,
                "{character:?} would render as a hollow box"
            );
        }
    }
}
