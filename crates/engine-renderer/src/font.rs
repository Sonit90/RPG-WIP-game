//! A 5x7 bitmap font, embedded as bit patterns.
//!
//! Small enough to be a lookup table, which is the point: the debug overlay must not drag
//! in a font loader, a rasterizer or a UI framework. Each glyph is seven rows of five
//! bits, most significant bit leftmost, so `0b10001` is a pixel at each edge.
//!
//! Only uppercase letters, digits and the punctuation the overlay actually uses are
//! defined. Text is upper-cased before lookup, and anything unknown renders as a hollow
//! box so a missing glyph is visible rather than silently blank.

/// Glyph width in font cells.
pub const GLYPH_WIDTH: usize = 5;

/// Glyph height in font cells.
pub const GLYPH_HEIGHT: usize = 7;

/// Cells between the left edge of one glyph and the next.
pub const GLYPH_ADVANCE: usize = GLYPH_WIDTH + 1;

/// Cells between the top of one line and the next.
pub const LINE_ADVANCE: usize = GLYPH_HEIGHT + 2;

/// Rendered for characters with no glyph.
const UNKNOWN: [u8; GLYPH_HEIGHT] = [
    0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
];

/// Returns the bitmap for `character`, upper-casing letters first.
///
/// Returns the hollow box for anything unsupported, and `None` only for the space
/// character, which has nothing to draw.
#[must_use]
pub fn glyph(character: char) -> Option<[u8; GLYPH_HEIGHT]> {
    if character == ' ' {
        return None;
    }
    Some(match character.to_ascii_uppercase() {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b01010, 0b10001,
        ],
        'Y' => [
            0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000,
        ],
        ':' => [
            0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '%' => [
            0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011,
        ],
        '=' => [
            0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000,
        ],
        '_' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111,
        ],
        _ => UNKNOWN,
    })
}

/// Width in cells of `text` when rendered, excluding the trailing gap.
#[must_use]
pub fn text_width_cells(text: &str) -> usize {
    match text.chars().count() {
        0 => 0,
        count => count * GLYPH_ADVANCE - (GLYPH_ADVANCE - GLYPH_WIDTH),
    }
}

/// Number of lit cells in `text`, which is also the number of quads needed to draw it.
#[must_use]
pub fn lit_cells(text: &str) -> usize {
    text.chars()
        .filter_map(glyph)
        .flat_map(|rows| rows.into_iter())
        .map(|row| row.count_ones() as usize)
        .sum()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn space_has_no_glyph() {
        assert!(glyph(' ').is_none());
    }

    #[test]
    fn every_glyph_row_fits_in_five_bits() {
        let sample = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ.,:-+/()%=_";
        for character in sample.chars() {
            let rows = glyph(character).expect("sample characters all have glyphs");
            for (index, row) in rows.into_iter().enumerate() {
                assert!(
                    row < (1 << GLYPH_WIDTH),
                    "{character:?} row {index} uses more than {GLYPH_WIDTH} bits: {row:#07b}"
                );
            }
        }
    }

    #[test]
    fn letters_are_case_insensitive() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('z'), glyph('Z'));
    }

    #[test]
    fn unknown_characters_render_as_a_visible_box() {
        assert_eq!(glyph('\u{2764}'), Some(UNKNOWN));
        assert_eq!(glyph('#'), Some(UNKNOWN));
    }

    #[test]
    fn known_shapes_are_right_side_up() {
        // 'T' is a full bar on top and a stem below it.
        let t = glyph('T').expect("T has a glyph");
        assert_eq!(t[0], 0b11111, "top bar");
        assert!(t[1..].iter().all(|&row| row == 0b00100), "centred stem");

        // 'L' is a stem with a bar along the bottom.
        let l = glyph('L').expect("L has a glyph");
        assert_eq!(l[GLYPH_HEIGHT - 1], 0b11111, "bottom bar");
        assert!(l[..GLYPH_HEIGHT - 1].iter().all(|&row| row == 0b10000));

        // '.' only marks the bottom rows.
        let dot = glyph('.').expect(". has a glyph");
        assert!(dot[..5].iter().all(|&row| row == 0));
        assert_ne!(dot[GLYPH_HEIGHT - 1], 0);
    }

    #[test]
    fn text_width_accounts_for_inter_glyph_gaps() {
        assert_eq!(text_width_cells(""), 0);
        assert_eq!(text_width_cells("A"), GLYPH_WIDTH);
        assert_eq!(text_width_cells("AB"), GLYPH_WIDTH * 2 + 1);
        assert_eq!(text_width_cells("ABC"), GLYPH_WIDTH * 3 + 2);
    }

    #[test]
    fn lit_cell_count_matches_the_bitmaps() {
        assert_eq!(lit_cells(" "), 0, "space draws nothing");
        assert_eq!(lit_cells("T"), 5 + 6, "top bar plus six stem cells");
        assert_eq!(lit_cells("TT"), lit_cells("T") * 2);
    }
}
