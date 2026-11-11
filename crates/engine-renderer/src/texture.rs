//! Textures as the renderer takes them: a handle, an image to upload, and the CPU work of
//! preparing one.
//!
//! Nothing here touches a device, so it is the same on every backend.

use crate::error::RendererError;
use crate::mipmap::TextureKind;
use crate::texture_id::TextureKey;

/// A texture uploaded through `Renderer::create_texture`.
///
/// A plain index. Textures are created while a scene is assembled and never released, so
/// there is no freed slot for a stale handle to address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureHandle(pub(crate) u32);

impl TextureHandle {
    /// A handle naming slot `index`.
    ///
    /// There is no check that the slot exists — the renderer performs that when the handle
    /// is used, and naming a slot that does not exist is exactly what a test of that check
    /// needs to be able to do.
    #[must_use]
    pub fn from_index(index: u32) -> Self {
        Self(index)
    }

    /// The slot this handle names.
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// An image to upload: dimensions, colour space, and straight RGBA8 texels.
///
/// Borrowed rather than owned, because the caller already has the texels in an
/// `engine_assets::TextureAsset` and copying them to hand them over would double the peak
/// memory of a scene load for no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureImage<'a> {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// What the image means: colour, data, or a tangent-space normal map.
    ///
    /// Two things follow from it, and both are wrong in a way that has no other symptom if
    /// it is wrong here. The colour space — colour is read through an sRGB view, data is
    /// not, and getting that backwards is the classic washed-out or muddy import. And the
    /// mip filter: colour is averaged in linear space, data where it lies, and a normal map
    /// is averaged as a direction and renormalized. See [`crate::mipmap`].
    pub kind: TextureKind,
    /// Row-major RGBA8 texels.
    pub texels: &'a [u8],
}

impl TextureImage<'_> {
    /// Whether the hardware linearizes this image on read.
    #[must_use]
    pub fn is_srgb(&self) -> bool {
        self.kind.is_srgb()
    }
}

/// What each of a model's textures is *for*, worked out from the materials that name it.
///
/// The asset carries a colour space per texture and nothing else, which was enough while the
/// colour space was all the upload needed. A mip chain needs more: a normal map's levels have
/// to be averaged as directions and renormalized, and a texture the uploader thought was
/// ordinary data would produce mips that flatten with distance. The material slots are the
/// only place that knowledge exists, so the roles are collected here rather than guessed from
/// the format.
///
/// A texture named by two slots of different kinds — a normal map also used as an ORM map,
/// which no exporter produces — resolves to the first role that claims it, and the log line
/// says so rather than picking silently.
pub(crate) fn texture_kinds(model: &engine_assets::ModelAsset) -> Vec<TextureKind> {
    let mut kinds: Vec<TextureKind> = model
        .textures
        .iter()
        .map(|texture| {
            if texture.format.is_srgb() {
                TextureKind::Color
            } else {
                TextureKind::Data
            }
        })
        .collect();
    for material in &model.materials {
        if let Some(index) = material.normal_texture
            && let Some(kind) = kinds.get_mut(index)
        {
            if *kind == TextureKind::Color {
                log::warn!(
                    "{}: texture {index} is used as a normal map and marked as colour; \
                     filtering it as a normal map",
                    model.name
                );
            }
            *kind = TextureKind::Normal;
        }
    }
    kinds
}

/// What makes a texture this texture: its content key and how it will be compressed.
///
/// The cheap half of a texture's CPU work — one hash and one scan of the texels — and the
/// half that decides whether the expensive half is needed at all.
pub(crate) struct TextureIdentity {
    pub(crate) key: TextureKey,
    pub(crate) compression: crate::mipmap::Compression,
    pub(crate) mip_levels: u32,
}

impl TextureIdentity {
    /// Validates `image` and identifies it.
    pub(crate) fn of(image: &TextureImage<'_>) -> Result<Self, RendererError> {
        let expected = image.width as usize * image.height as usize * 4;
        if image.width == 0 || image.height == 0 || image.texels.len() != expected {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a {}x{} RGBA texture needs {expected} bytes, got {}",
                    image.width,
                    image.height,
                    image.texels.len()
                ),
            });
        }
        let mip_levels = crate::mipmap::level_count(image.width, image.height);
        let inspect = crate::upload_timing::Probe::start();
        let compression = crate::mipmap::compression_for(
            image.kind,
            image.width,
            image.height,
            crate::mipmap::has_transparency(image.texels),
        );
        let key = TextureKey::new(
            image.texels,
            image.width,
            image.height,
            image.kind,
            mip_levels > 1,
        );
        inspect.inspect();
        Ok(Self {
            key,
            compression,
            mip_levels,
        })
    }

    /// The expensive half: the mip chain, filtered for the image's kind, and every level
    /// block-compressed if the image is to be.
    pub(crate) fn prepare(self, image: &TextureImage<'_>) -> PreparedTexture {
        use crate::mipmap::Compression;
        // Filtered on the CPU at upload rather than generated on the GPU with a compute
        // pass. Uploading content is not a per-frame cost, the filters differ per texture
        // kind — a normal map's mips have to be renormalized — and a compute path would need
        // a UAV on every sampled texture, which is a resource flag on all of them to save
        // milliseconds that happen once.
        let mips = crate::upload_timing::Probe::start();
        let chain = crate::mipmap::generate(image.texels, image.width, image.height, image.kind);
        mips.mips(u64::from(image.width) * u64::from(image.height));

        // Block compression happens *after* the chain is filtered, level by level. Filtering
        // compressed data would mean decompressing it first, and compressing level 0 and
        // reducing that would compound the format's error down the chain.
        let probe = crate::upload_timing::Probe::start();
        let compressed: Vec<Vec<u8>> = match self.compression {
            Compression::None => Vec::new(),
            Compression::Bc1 | Compression::Bc5 => std::iter::once(image.texels)
                .chain(chain.iter().map(Vec::as_slice))
                .enumerate()
                .map(|(level, texels)| {
                    let (width, height) =
                        crate::mipmap::level_size(image.width, image.height, level as u32);
                    crate::mipmap::compress_level(self.compression, texels, width, height)
                })
                .collect(),
        };
        probe.compression();
        PreparedTexture {
            key: self.key,
            compression: self.compression,
            mip_levels: self.mip_levels,
            width: image.width,
            height: image.height,
            kind: image.kind,
            chain,
            compressed,
        }
    }
}

/// A texture's CPU work, done: what `Renderer::create_texture` computes before it touches
/// the device. `Send`, so it can be done on another thread and handed over.
#[derive(Debug)]
pub struct PreparedTexture {
    pub(crate) key: TextureKey,
    pub(crate) compression: crate::mipmap::Compression,
    pub(crate) mip_levels: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) kind: TextureKind,
    /// Levels 1 onwards, uncompressed.
    pub(crate) chain: Vec<Vec<u8>>,
    /// Every level, level 0 included, compressed — or empty when the texture is not.
    pub(crate) compressed: Vec<Vec<u8>>,
}

impl PreparedTexture {
    /// Whether this was prepared from an image shaped like `image`. The content is not
    /// re-hashed — that would be the work preparing saved — so this guards against a list
    /// handed to the wrong model, not against one whose texels changed in between.
    pub(crate) fn describes(&self, image: &TextureImage<'_>) -> bool {
        self.width == image.width && self.height == image.height && self.kind == image.kind
    }
}

/// Every texture of one model, prepared. See [`prepare_model_textures`].
#[derive(Debug)]
pub struct PreparedTextures {
    pub(crate) textures: Vec<PreparedTexture>,
}

/// Does the CPU work of uploading `image` — the content hash, the mip chain and the block
/// compression — without a device, so it can run on any thread.
///
/// # Errors
///
/// Returns [`RendererError::InvalidConfig`] if the texel buffer does not match the
/// dimensions.
pub fn prepare_texture(image: &TextureImage<'_>) -> Result<PreparedTexture, RendererError> {
    Ok(TextureIdentity::of(image)?.prepare(image))
}

/// [`prepare_texture`] for every texture of `model`, with the roles its materials give them.
///
/// What `Renderer::create_model_prepared` takes. A model whose textures cannot be prepared
/// returns `None`, and its upload prepares them itself and reports the error the way it
/// always did.
#[must_use]
pub fn prepare_model_textures(model: &engine_assets::ModelAsset) -> Option<PreparedTextures> {
    let kinds = texture_kinds(model);
    let textures = model
        .textures
        .iter()
        .zip(kinds)
        .map(|(texture, kind)| {
            prepare_texture(&TextureImage {
                width: texture.width,
                height: texture.height,
                kind,
                texels: &texture.texels,
            })
            .ok()
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PreparedTextures { textures })
}
