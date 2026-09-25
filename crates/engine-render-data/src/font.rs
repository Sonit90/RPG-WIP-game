//! The interface's typefaces, as data: which faces exist and how wide every glyph is.
//!
//! ```text
//! fonts/*.ttf ──build.rs──► FACES (advances, ascent, descent)   this crate: measuring
//!             └─engine-renderer/build.rs──► SDF atlas + quads    the renderer: drawing
//! ```
//!
//! Two families, bundled with the game (SIL Open Font License, `fonts/OFL-*.txt`): **Inter**
//! for everything that is read — descriptions, numbers, labels — in three weights, and
//! **Cinzel** for the few headings that carry the game's identity. Both are drawn from signed
//! distance fields, so one atlas serves every size and every interface scale crisply.
//!
//! Measurement lives here so a layout written in `engine-presentation` can align, wrap and fit
//! text without linking the renderer. The renderer lays glyphs out with these same advances.
//! No kerning: the faces are spaced well enough without it at interface sizes, and a pair table
//! would be a second source of widths to keep in step.

include!(concat!(env!("OUT_DIR"), "/font_metrics.rs"));

/// One face's line and glyph metrics, in ems (multiply by the pixel size).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceMetrics {
    /// Height above the baseline.
    pub ascent: f32,
    /// Depth below the baseline, positive.
    pub descent: f32,
    /// Extra space between lines the face asks for.
    pub line_gap: f32,
    /// How far the pen moves after each glyph of [`CHARSET`], in order.
    pub advances: [f32; GLYPH_COUNT],
}

/// Which face a run of text is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiFont {
    /// Inter Regular: descriptions and body text.
    #[default]
    Regular,
    /// Inter Medium: labels and metadata that must hold their own at small sizes.
    Medium,
    /// Inter SemiBold: names, section headings, anything that is scanned for.
    SemiBold,
    /// Cinzel SemiBold: the dungeon's title and major encounter titles. Sparingly.
    Display,
}

impl UiFont {
    /// Every face, in atlas order.
    pub const ALL: [Self; 4] = [Self::Regular, Self::Medium, Self::SemiBold, Self::Display];

    /// Its index into [`FACES`] and the renderer's atlas.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Its metrics.
    #[must_use]
    pub fn metrics(self) -> &'static FaceMetrics {
        &FACES[self.index()]
    }

    /// Height above the baseline at `size` pixels per em.
    #[must_use]
    pub fn ascent(self, size: f32) -> f32 {
        self.metrics().ascent * size
    }

    /// Baseline-to-baseline distance at `size`, with the face's own gap.
    #[must_use]
    pub fn line_height(self, size: f32) -> f32 {
        let metrics = self.metrics();
        (metrics.ascent + metrics.descent + metrics.line_gap) * size
    }

    /// How far one character moves the pen at `size`. An unknown character is measured, and
    /// drawn, as a question mark.
    #[must_use]
    pub fn advance(self, character: char, size: f32) -> f32 {
        self.metrics().advances[glyph_index(character)] * size
    }

    /// How wide `text` is at `size`, in pixels.
    #[must_use]
    pub fn width(self, text: &str, size: f32) -> f32 {
        let advances = &self.metrics().advances;
        text.chars()
            .map(|character| advances[glyph_index(character)])
            .sum::<f32>()
            * size
    }

    /// `text` broken into lines no wider than `max_width` at `size`, at spaces; a word wider
    /// than a whole line is broken where it has to be.
    #[must_use]
    pub fn wrap(self, text: &str, size: f32, max_width: f32) -> Vec<String> {
        let space = self.advance(' ', size);
        let mut lines = Vec::new();
        let mut line = String::new();
        let mut width = 0.0;
        for word in text.split_whitespace() {
            let word_width = self.width(word, size);
            if !line.is_empty() && width + space + word_width > max_width {
                lines.push(std::mem::take(&mut line));
                width = 0.0;
            }
            if word_width > max_width {
                // Break the word itself, a character at a time.
                for character in word.chars() {
                    let advance = self.advance(character, size);
                    if !line.is_empty() && width + advance > max_width {
                        lines.push(std::mem::take(&mut line));
                        width = 0.0;
                    }
                    line.push(character);
                    width += advance;
                }
                continue;
            }
            if !line.is_empty() {
                line.push(' ');
                width += space;
            }
            line.push_str(word);
            width += word_width;
        }
        if !line.is_empty() {
            lines.push(line);
        }
        lines
    }

    /// `text` cut to fit `max_width` at `size`, with an ellipsis when it had to be cut.
    #[must_use]
    pub fn fit(self, text: &str, size: f32, max_width: f32) -> String {
        if self.width(text, size) <= max_width {
            return text.to_owned();
        }
        let ellipsis = self.advance('\u{2026}', size);
        let mut out = String::new();
        let mut width = 0.0;
        for character in text.chars() {
            let advance = self.advance(character, size);
            if width + advance + ellipsis > max_width {
                break;
            }
            out.push(character);
            width += advance;
        }
        let trimmed = out.trim_end().len();
        out.truncate(trimmed);
        out.push('\u{2026}');
        out
    }
}

/// The glyph a character is drawn with: its index in [`CHARSET`], or the question mark's.
#[must_use]
pub fn glyph_index(character: char) -> usize {
    let code = character as u32;
    if (32..=126).contains(&code) {
        return (code - 32) as usize;
    }
    CHARSET
        .chars()
        .skip(95)
        .position(|known| known == character)
        .map_or(('?' as u32 - 32) as usize, |index| index + 95)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_maps_straight_onto_the_charset_and_extras_are_found() {
        assert_eq!(glyph_index(' '), 0);
        assert_eq!(glyph_index('A'), 33);
        assert_eq!(
            CHARSET.chars().nth(glyph_index('\u{00B7}')),
            Some('\u{00B7}')
        );
        assert_eq!(
            glyph_index('\u{4E00}'),
            glyph_index('?'),
            "unknown is a question mark"
        );
    }

    #[test]
    fn a_proportional_face_is_proportional_and_scales_linearly() {
        let font = UiFont::Regular;
        assert!(font.advance('W', 10.0) > font.advance('i', 10.0));
        let small = font.width("Grave Drain", 12.0);
        let large = font.width("Grave Drain", 24.0);
        assert!((large - small * 2.0).abs() < 1e-3);
        assert!(font.line_height(16.0) > font.ascent(16.0));
    }

    #[test]
    fn wrapping_keeps_every_line_inside_the_width_and_every_word() {
        let text = "Move away from the targeted player before the effect detonates.";
        let lines = UiFont::Regular.wrap(text, 14.0, 160.0);
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(UiFont::Regular.width(line, 14.0) <= 160.0 + 1e-3, "{line}");
        }
        assert_eq!(lines.join(" "), text);
    }

    #[test]
    fn fitting_adds_an_ellipsis_only_when_it_cuts() {
        assert_eq!(UiFont::Medium.fit("Short", 14.0, 200.0), "Short");
        let cut = UiFont::Medium.fit("A name far too long for its little box", 14.0, 90.0);
        assert!(cut.ends_with('\u{2026}'));
        assert!(UiFont::Medium.width(&cut, 14.0) <= 90.0 + 1e-3);
    }
}
