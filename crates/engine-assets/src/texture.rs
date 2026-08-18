//! Textures, as the runtime wants them: uncompressed RGBA texels and a colour space.
//!
//! # Why the runtime format is uncompressed
//!
//! PNG is a *container*. The GPU cannot sample it, so something has to decode it, and the
//! only question is when. Decoding at load costs milliseconds once; decoding per frame is
//! not a thing anyone does. So the importer decodes and the runtime asset holds texels.
//!
//! BC7 would be the next step — a quarter of the memory and the same visual result — and it
//! is deliberately not here yet. A block compressor is a build-time tool, and this phase has
//! no measurement saying texture memory is the constraint. [`TextureFormat`] exists so that
//! adding `Bc7Srgb` later is a variant rather than a redesign.
//!
//! # Colour space is not a detail
//!
//! A base colour texture is authored in sRGB and must be sampled through an sRGB view, so
//! the hardware linearizes it before the shader does any arithmetic. A normal map or a
//! roughness map is *data*, not colour, and must not be linearized. Getting this backwards
//! produces a scene that looks washed out or muddy with no obvious cause, so the format is
//! carried on the asset rather than assumed at upload.

use crate::error::AssetError;

/// How the texels are laid out and how they should be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// Four 8-bit channels holding colour. Sampled through an sRGB view.
    Rgba8Srgb,
    /// Four 8-bit channels holding data — normals, roughness, masks. Sampled linearly.
    Rgba8Unorm,
}

impl TextureFormat {
    /// Bytes per texel.
    #[must_use]
    pub fn bytes_per_texel(self) -> usize {
        match self {
            Self::Rgba8Srgb | Self::Rgba8Unorm => 4,
        }
    }

    /// Whether the hardware should linearize on read.
    #[must_use]
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::Rgba8Srgb)
    }
}

/// An image ready to be uploaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureAsset {
    /// What it is, for diagnostics: `"fire_mage_base_color"`.
    pub name: String,
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// How to interpret [`TextureAsset::texels`].
    pub format: TextureFormat,
    /// Row-major texels, `width * height * format.bytes_per_texel()` bytes.
    pub texels: Vec<u8>,
}

impl TextureAsset {
    /// Builds a texture, checking that the texel count matches the dimensions.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] for a zero dimension or a texel buffer that is not
    /// exactly `width * height * bytes_per_texel` long. A short buffer is the failure worth
    /// catching here: the upload reads a fixed number of bytes per row, so a buffer one row
    /// short reads past the end of a `Vec`.
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        format: TextureFormat,
        texels: Vec<u8>,
    ) -> Result<Self, AssetError> {
        let name = name.into();
        if width == 0 || height == 0 {
            return Err(AssetError::invalid(
                &name,
                "texture",
                format!("a texture cannot be {width}x{height}"),
            ));
        }
        let expected = width as usize * height as usize * format.bytes_per_texel();
        if texels.len() != expected {
            return Err(AssetError::invalid(
                &name,
                "texture",
                format!(
                    "{}x{} needs {expected} bytes, got {}",
                    width,
                    height,
                    texels.len()
                ),
            ));
        }
        Ok(Self {
            name,
            width,
            height,
            format,
            texels,
        })
    }

    /// Bytes this texture occupies on the GPU, ignoring alignment padding.
    #[must_use]
    pub fn byte_size(&self) -> usize {
        self.texels.len()
    }

    /// A texture filled by a function of the texel's normalized position.
    ///
    /// How every texture in this phase is authored: the procedural characters have no
    /// painted maps, and a function of `(u, v)` is a perfectly good source for the stylized
    /// bands, trims and grain they need. An imported `.glb` supplies real images through the
    /// same type, so the renderer cannot tell the two apart.
    #[must_use]
    pub fn generated(
        name: impl Into<String>,
        width: u32,
        height: u32,
        format: TextureFormat,
        mut shade: impl FnMut(f32, f32) -> [u8; 4],
    ) -> Self {
        let mut texels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            // Texel centres, not corners: sampling the corner of a 2-texel image gives 0.0
            // and 0.5 rather than 0.25 and 0.75, which biases every generated pattern half a
            // texel towards the origin.
            let v = (y as f32 + 0.5) / height as f32;
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                texels.extend_from_slice(&shade(u, v));
            }
        }
        Self {
            name: name.into(),
            width,
            height,
            format,
            texels,
        }
    }

    /// The same image halved in each dimension, averaging each 2x2 block.
    ///
    /// `None` once either dimension is 1, which is where halving stops meaning anything.
    #[must_use]
    pub fn halved(&self) -> Option<Self> {
        if self.width < 2 || self.height < 2 {
            return None;
        }
        let (width, height) = (self.width / 2, self.height / 2);
        let stride = self.width as usize * 4;
        let mut texels = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let top = y * 2 * stride + x * 2 * 4;
                let bottom = top + stride;
                for channel in 0..4 {
                    // A box filter in whatever space the texels are in. Correct for a data
                    // map; slightly dark for an sRGB one, which is the trade every engine
                    // makes until it has a reason to convert both ways per texel.
                    let sum = u32::from(self.texels[top + channel])
                        + u32::from(self.texels[top + 4 + channel])
                        + u32::from(self.texels[bottom + channel])
                        + u32::from(self.texels[bottom + 4 + channel]);
                    texels.push((sum / 4) as u8);
                }
            }
        }
        Some(Self {
            name: self.name.clone(),
            width,
            height,
            format: self.format,
            texels,
        })
    }

    /// The same image, halved until neither dimension exceeds `max`.
    ///
    /// What lets a 4096 map an artist exported anyway load at the size the runtime budgets
    /// for, instead of the whole model being refused over a texture nobody looks at closely.
    /// A texture already within the limit is returned unchanged and copies nothing.
    #[must_use]
    pub fn fitted_within(mut self, max: u32) -> Self {
        let max = max.max(1);
        while self.width > max || self.height > max {
            match self.halved() {
                Some(smaller) => self = smaller,
                None => break,
            }
        }
        self
    }

    /// A single-texel opaque white image.
    ///
    /// What a material with no base colour texture binds. A branch in the shader would be a
    /// second pipeline permutation; a 4-byte texture is cheaper than the permutation and
    /// keeps every material on one code path.
    #[must_use]
    pub fn white() -> Self {
        Self {
            name: "white".to_owned(),
            width: 1,
            height: 1,
            format: TextureFormat::Rgba8Srgb,
            texels: vec![255, 255, 255, 255],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_texel_buffer_that_does_not_match_the_dimensions_is_rejected() {
        // A short buffer reads past the end of the Vec during upload rather than drawing
        // something visibly wrong, so it has to fail here.
        let error = TextureAsset::new("short", 4, 4, TextureFormat::Rgba8Srgb, vec![0; 60]);
        assert!(error.is_err(), "a 4x4 RGBA image is 64 bytes, not 60");
    }

    #[test]
    fn a_zero_dimension_is_rejected() {
        assert!(TextureAsset::new("empty", 0, 4, TextureFormat::Rgba8Srgb, vec![]).is_err());
    }

    #[test]
    fn a_generated_texture_samples_texel_centres() {
        // Corners would make the first texel's u exactly 0.0 and bias every pattern half a
        // texel towards the origin.
        let texture = TextureAsset::generated("ramp", 2, 1, TextureFormat::Rgba8Unorm, |u, _| {
            [(u * 255.0) as u8, 0, 0, 255]
        });
        assert_eq!(
            texture.texels[0], 63,
            "u should be 0.25 at the first centre"
        );
        assert_eq!(texture.texels[4], 191, "and 0.75 at the second");
    }

    #[test]
    fn halving_averages_each_block_and_keeps_the_buffer_consistent() {
        let texture = TextureAsset::generated("ramp", 4, 4, TextureFormat::Rgba8Unorm, |u, _| {
            [(u * 200.0) as u8, 0, 0, 255]
        });
        let Some(half) = texture.halved() else {
            panic!("a 4x4 image halves");
        };
        assert_eq!((half.width, half.height), (2, 2));
        assert_eq!(half.texels.len(), 2 * 2 * 4);
        assert_eq!(
            u32::from(half.texels[0]),
            (u32::from(texture.texels[0]) + u32::from(texture.texels[4])) / 2,
            "a block of two equal rows averages to the row"
        );
    }

    #[test]
    fn a_texture_over_the_limit_is_halved_until_it_fits_and_one_under_it_is_untouched() {
        let big = TextureAsset::generated("big", 8, 4, TextureFormat::Rgba8Srgb, |_, _| {
            [1, 2, 3, 4]
        });
        let fitted = big.fitted_within(2);
        assert_eq!((fitted.width, fitted.height), (2, 1));

        let small = TextureAsset::white().fitted_within(2);
        assert_eq!((small.width, small.height), (1, 1));
    }

    #[test]
    fn colour_and_data_textures_are_distinguishable() {
        assert!(TextureFormat::Rgba8Srgb.is_srgb());
        assert!(!TextureFormat::Rgba8Unorm.is_srgb());
        assert_eq!(TextureAsset::white().byte_size(), 4);
    }
}
