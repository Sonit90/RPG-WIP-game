//! The interface fonts on the GPU side: the distance-field atlas `build.rs` generated and each
//! glyph's quad in it.
//!
//! Widths come from `engine_render_data::font`, which reads the same TTFs; this module only
//! knows where each glyph's ink sits relative to the pen and where its cell is in the atlas.

/// One glyph's quad: where it sits relative to the pen, and where it is in the atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    /// Left, top, right, bottom, in ems, relative to the pen on the baseline, y down.
    pub plane: [f32; 4],
    /// Left, top, right, bottom, in atlas texture coordinates.
    pub uv: [f32; 4],
}

impl GlyphQuad {
    /// A glyph with no ink: a space.
    pub const EMPTY: Self = Self {
        plane: [0.0; 4],
        uv: [0.0; 4],
    };

    /// Whether it draws anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plane[2] <= self.plane[0]
    }
}

mod generated {
    #![allow(missing_docs, reason = "every item is documented by the generator")]
    use super::GlyphQuad;
    include!(concat!(env!("OUT_DIR"), "/fonts.rs"));
}

pub use generated::{ATLAS, ATLAS_HEIGHT, ATLAS_WIDTH, CHARSET as ATLAS_CHARSET, QUADS, SDF_RANGE};

/// The atlas as straight RGBA: white, with the distance in alpha, which is what the UI shader
/// reads. Four times the bytes of the single channel, for a texture uploaded once.
#[must_use]
pub fn atlas_rgba() -> Vec<u8> {
    ATLAS
        .iter()
        .flat_map(|&distance| [255, 255, 255, distance])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_render_data::font::{CHARSET, UiFont, glyph_index};

    #[test]
    fn the_atlas_and_the_metrics_describe_the_same_characters() {
        assert_eq!(generated::CHARSET, CHARSET);
        assert_eq!(ATLAS.len(), ATLAS_WIDTH * ATLAS_HEIGHT);
        assert_eq!(QUADS.len(), UiFont::ALL.len());
    }

    #[test]
    fn a_space_has_no_quad_and_a_letter_has_one_inside_the_atlas() {
        for font in UiFont::ALL {
            assert!(QUADS[font.index()][glyph_index(' ')].is_empty());
            let a = QUADS[font.index()][glyph_index('A')];
            assert!(!a.is_empty());
            assert!(a.uv.iter().all(|value| (0.0..=1.0).contains(value)));
            // A capital stands on the baseline and rises above it.
            assert!(a.plane[1] < 0.0 && a.plane[3] > 0.0);
        }
    }
}
