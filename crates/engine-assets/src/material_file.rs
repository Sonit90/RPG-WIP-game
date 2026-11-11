//! A material as a file: what the generator writes into `assets/generated/materials/`.
//!
//! ```text
//!  materials/rock.json ──► load ──► MaterialFile ──► MaterialAsset + TextureAsset
//!  materials/rock.png  ──┘                             the same two types a .glb produces
//! ```
//!
//! # Why a material is a file at all
//!
//! A `.glb` carries its own materials, so nothing *has* to read this to draw a rock. What a
//! file buys is the other three things a material has to be: something the asset browser can
//! list, something a designer can change without a rebuild, and something an artist can
//! replace by dropping a `.png` over the generated one. A material that exists only inside
//! twenty-four models is a material that has to be changed twenty-four times.
//!
//! # Why the numbers are here and the image is beside it
//!
//! The JSON holds what a person edits — colour, roughness, metallic, opacity — and names the
//! image beside it. An image inside JSON is a base64 blob nobody can open, and a `.png` with
//! roughness written into a corner of it is a convention that has to be documented rather
//! than read.
//!
//! # What a broken file does
//!
//! Fails, naming the file and what is wrong, exactly as a scene file does. A material that
//! half-loads is a surface that is subtly the wrong colour in one level and right in the
//! next.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::AssetError;
use crate::material::MaterialAsset;
use crate::texture::{TextureAsset, TextureFormat};

/// The version this build writes, and the only one it reads.
pub const FORMAT_VERSION: u32 = 1;

/// A material, as the file says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialFile {
    /// Which version of this format the file is.
    pub version: u32,
    /// What the material is called: `"rock"`.
    pub name: String,
    /// Base colour and opacity, linear, `[r, g, b, a]`.
    ///
    /// White with a texture on it means "the texture is the colour", which is what every
    /// generated material says. A material with no texture is this colour and nothing else.
    pub base_color: [f32; 4],
    /// How rough the surface is: 0 is a mirror, 1 is chalk.
    pub roughness: f32,
    /// How metallic it is. Everything generated is 0.
    pub metallic: f32,
    /// The base colour image beside this file: `"rock.png"`.
    ///
    /// Relative to the file's own directory, so a materials folder can be moved whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_color_texture: Option<String>,
}

impl MaterialFile {
    /// The file a [`MaterialAsset`] and its image would be written as.
    #[must_use]
    pub fn from_material(material: &MaterialAsset, texture: Option<&str>) -> Self {
        Self {
            version: FORMAT_VERSION,
            name: material.name.clone(),
            base_color: [
                material.base_color.x,
                material.base_color.y,
                material.base_color.z,
                material.base_color.w,
            ],
            roughness: material.roughness,
            metallic: material.metallic,
            base_color_texture: texture.map(str::to_owned),
        }
    }

    /// The runtime material this file describes.
    ///
    /// The texture index is `Some(0)` when the file names an image, because a material loaded
    /// on its own is loaded with its own image and nothing else — the model that carries it
    /// decides the real slot.
    #[must_use]
    pub fn to_material(&self) -> MaterialAsset {
        let material = MaterialAsset::colored(
            self.name.clone(),
            self.base_color[0],
            self.base_color[1],
            self.base_color[2],
        )
        .with_roughness(self.roughness.clamp(0.0, 1.0))
        .with_metallic(self.metallic.clamp(0.0, 1.0))
        .with_opacity(self.base_color[3].clamp(0.0, 1.0));
        if self.base_color_texture.is_some() {
            material.with_base_color_texture(0)
        } else {
            material
        }
    }

    /// Where this material's image is, given where the file is.
    #[must_use]
    pub fn texture_path(&self, file: &Path) -> Option<PathBuf> {
        let image = self.base_color_texture.as_ref()?;
        let directory = file.parent().unwrap_or_else(|| Path::new("."));
        Some(directory.join(image))
    }

    /// Reads a material file.
    ///
    /// # Errors
    ///
    /// [`AssetError::Malformed`] naming the file if it cannot be read or is not this format,
    /// and [`AssetError::Invalid`] if it is from another version.
    pub fn load(path: &Path) -> Result<Self, AssetError> {
        let text = std::fs::read_to_string(path).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })?;
        let file: Self = serde_json::from_str(&text).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })?;
        if file.version != FORMAT_VERSION {
            return Err(AssetError::invalid(
                path.display().to_string(),
                "version",
                format!(
                    "this build reads material format {FORMAT_VERSION}, the file is {}",
                    file.version
                ),
            ));
        }
        Ok(file)
    }

    /// Writes a material file, creating the directory if it is not there.
    ///
    /// # Errors
    ///
    /// [`AssetError::Malformed`] naming the file if it cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), AssetError> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).map_err(|error| AssetError::Malformed {
                model: path.display().to_string(),
                reason: error.to_string(),
            })?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })?;
        std::fs::write(path, text).map_err(|error| AssetError::Malformed {
            model: path.display().to_string(),
            reason: error.to_string(),
        })
    }

    /// Reads the material and its image together.
    ///
    /// The image is read as sRGB, which is what a base colour map is. A file that names an
    /// image that is not there is an error rather than a colourless material: a texture that
    /// silently did not load is a surface nobody can explain.
    ///
    /// # Errors
    ///
    /// Whatever [`MaterialFile::load`] and the PNG decoder return.
    pub fn load_with_texture(path: &Path) -> Result<(Self, Option<TextureAsset>), AssetError> {
        let file = Self::load(path)?;
        let Some(image) = file.texture_path(path) else {
            return Ok((file, None));
        };
        let bytes = std::fs::read(&image).map_err(|error| AssetError::Malformed {
            model: image.display().to_string(),
            reason: error.to_string(),
        })?;
        let texture =
            crate::gltf_import::decode_png_file(&file.name, TextureFormat::Rgba8Srgb, &bytes)?;
        Ok((file, Some(texture)))
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::gltf_export::encode_png_file;
    use crate::procedural::starter::STARTER_MATERIALS;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join("mygame_material_tests")
            .join(name)
    }

    #[test]
    fn a_material_survives_being_written_and_read_back() {
        let path = temp("round_trip.json");
        let starter = STARTER_MATERIALS[2];
        let original = MaterialFile::from_material(&starter.material(), Some("rock.png"));
        original.save(&path).expect("the material saves");
        let restored = MaterialFile::load(&path).expect("the material loads");
        assert_eq!(restored, original);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_numbers_come_back_as_the_material_they_describe() {
        let starter = STARTER_MATERIALS[5];
        assert_eq!(starter.name, "water");
        let file = MaterialFile::from_material(&starter.material(), Some("water.png"));
        let material = file.to_material();
        assert!((material.roughness - starter.roughness).abs() < 1e-6);
        assert!(
            material.is_transparent(),
            "water came back opaque, so its opacity did not survive the file"
        );
        assert_eq!(material.base_color_texture, Some(0));
    }

    #[test]
    fn a_material_and_its_image_load_together() {
        let directory = temp("with_image");
        std::fs::create_dir_all(&directory).expect("a temporary directory");
        let starter = STARTER_MATERIALS[0];
        let texture = starter.texture();
        std::fs::write(
            directory.join("grass.png"),
            encode_png_file(&texture).expect("the texture encodes"),
        )
        .expect("the image writes");
        let path = directory.join("grass.json");
        MaterialFile::from_material(&starter.material(), Some("grass.png"))
            .save(&path)
            .expect("the material saves");

        let (file, image) = MaterialFile::load_with_texture(&path).expect("both load");
        assert_eq!(file.name, "grass");
        let image = image.expect("the file names an image");
        assert_eq!((image.width, image.height), (texture.width, texture.height));
        assert_eq!(image.texels, texture.texels, "PNG is lossless");
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_file_from_another_version_says_so_rather_than_half_loading() {
        let path = temp("wrong_version.json");
        let mut file = MaterialFile::from_material(&STARTER_MATERIALS[1].material(), None);
        file.version = FORMAT_VERSION + 1;
        file.save(&path).expect("it writes");
        let error = MaterialFile::load(&path).expect_err("a future version is refused");
        assert!(
            format!("{error}").contains("material format"),
            "the error does not say what is wrong: {error}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_names_itself() {
        let error = MaterialFile::load(&temp("not_here.json")).expect_err("it is not there");
        assert!(format!("{error}").contains("not_here.json"), "{error}");
    }
}
