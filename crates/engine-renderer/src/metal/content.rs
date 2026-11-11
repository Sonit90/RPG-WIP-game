//! Creating content: meshes, textures, models, materials, the map picture, screenshots and
//! particles. The `impl Renderer` half that uploads things and does not record frames.

use std::time::Instant;

use engine_render_data::{
    Camera, Lighting, MaterialHandle, MeshHandle, ModelBinding, ModelLodBinding, ModelPart,
    PartGeometry, RenderWorld, SkinnedMeshHandle,
};

use super::readback::Readback;
use super::renderer::{MetalMaterial, Renderer, UploadedTexture};
use super::resources::{
    DynamicMesh, GpuMesh, TextureFormat, buffer_with_data, texture_with_levels,
};
use super::ui::UiMapPicture;
use crate::error::RendererError;
use crate::material::{
    MATERIAL_TEXTURE_COUNT, MaterialConstants, MaterialDesc, MaterialTextureSet,
    MaterialTextureSlot, MaterialTextures,
};
use crate::mesh::{MeshData, MeshVertex, SkinnedMeshData, SkinnedVertex};
use crate::mipmap::Compression;
use crate::shader::ShaderFeatures;
use crate::texture::{
    PreparedTexture, PreparedTextures, TextureHandle, TextureIdentity, TextureImage, texture_kinds,
};
use crate::texture_id::TextureKey;
use crate::ui_pipeline::UiBatchStats;

/// The height of a model's LOD0 geometry, in metres.
///
/// Measured rather than declared, so a [`ModelBinding`] can always be scaled to a gameplay
/// height. A model with no geometry reports one metre rather than zero — the caller divides
/// by this.
fn measured_height(model: &engine_assets::ModelAsset) -> f32 {
    let Some(lod) = model.lod0() else {
        return 1.0;
    };
    let mut lowest = f32::MAX;
    let mut highest = f32::MIN;
    for position in lod
        .submeshes
        .iter()
        .flat_map(|submesh| submesh.geometry.positions())
    {
        lowest = lowest.min(position.y);
        highest = highest.max(position.y);
    }
    let height = highest - lowest;
    if height.is_finite() && height > 1e-3 {
        height
    } else {
        1.0
    }
}

/// What an RGBA8 image of this size costs with a full mip chain.
///
/// The baseline the texture-memory accounting compares against: four bytes a texel for every
/// level.
fn rgba_chain_bytes(width: u32, height: u32) -> usize {
    (0..crate::mipmap::level_count(width, height))
        .map(|level| {
            let (level_width, level_height) = crate::mipmap::level_size(width, height, level);
            level_width as usize * level_height as usize * 4
        })
        .sum()
}

impl Renderer {
    /// Opens an upload batch.
    ///
    /// On Metal this is bookkeeping only: buffers and textures are created in shared memory and
    /// written by the CPU, so a resource is readable by the GPU the moment its `create_*`
    /// returns and there is nothing to defer or wait for. The pair exists so the application's
    /// load code is the same on every backend. Nesting is not supported: a second call while a
    /// batch is open is a no-op.
    ///
    /// # Errors
    ///
    /// Never fails; the signature matches the D3D12 renderer's.
    pub fn begin_upload_batch(&mut self) -> Result<(), RendererError> {
        if self.batch.is_none() {
            self.batch = Some(0);
        }
        Ok(())
    }

    /// Closes the batch and returns how many resources were created while it was open.
    ///
    /// Harmless with no batch open.
    ///
    /// # Errors
    ///
    /// Never fails; the signature matches the D3D12 renderer's.
    pub fn end_upload_batch(&mut self) -> Result<usize, RendererError> {
        let queued = self.batch.take().unwrap_or(0);
        log::debug!("upload batch: {queued} resources, nothing to flush on Metal");
        Ok(queued)
    }

    /// Whether an upload batch is open.
    #[must_use]
    pub fn is_batching_uploads(&self) -> bool {
        self.batch.is_some()
    }

    /// Counts a resource created while a batch is open.
    pub(super) fn note_upload(&mut self) {
        if let Some(count) = self.batch.as_mut() {
            *count += 1;
        }
    }

    /// Uploads a mesh and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn create_mesh(&mut self, data: &MeshData) -> Result<MeshHandle, RendererError> {
        let mesh = GpuMesh::new(
            self.context.device(),
            data.vertices(),
            data.indices(),
            data.bounds(),
        )?;
        self.note_upload();
        let index = self.meshes.len() as u32;
        self.meshes.push(mesh);
        Ok(MeshHandle::new(index, 1))
    }

    /// Replaces the geometry behind `handle` with `data`, keeping the handle.
    ///
    /// For geometry that changes at run time — the editor's terrain under a brush. The old
    /// buffers are kept until the GPU has passed every frame already committed, since any of
    /// them may still be drawing the old geometry.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a handle that names no mesh, or
    /// [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn replace_mesh(
        &mut self,
        handle: MeshHandle,
        data: &MeshData,
    ) -> Result<(), RendererError> {
        let index = handle.index();
        if index >= self.meshes.len() {
            return Err(RendererError::InvalidConfig {
                reason: format!("mesh {index} was replaced, and {} exist", self.meshes.len()),
            });
        }
        let mesh = GpuMesh::new(
            self.context.device(),
            data.vertices(),
            data.indices(),
            data.bounds(),
        )?;
        self.note_upload();
        let old = std::mem::replace(&mut self.meshes[index], mesh);
        self.retired_meshes
            .push((self.frames.last_committed(), old));
        Ok(())
    }

    /// Creates a mesh whose geometry is rewritten at run time, up to once per frame.
    ///
    /// `max_vertices` and `max_indices` fix the capacity for the mesh's whole life. All the
    /// GPU memory is allocated here — `frames_in_flight + 1` copies — so
    /// [`Renderer::write_dynamic_mesh`] allocates nothing. The mesh draws nothing until its
    /// first write.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for a zero capacity, or
    /// [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn create_dynamic_mesh(
        &mut self,
        max_vertices: usize,
        max_indices: usize,
    ) -> Result<MeshHandle, RendererError> {
        let (dynamic, mesh) = DynamicMesh::new(
            self.context.device(),
            self.frames.frames_in_flight(),
            max_vertices,
            max_indices,
        )?;
        let index = self.meshes.len();
        self.meshes.push(mesh);
        self.dynamic_meshes.insert(index, dynamic);
        Ok(MeshHandle::new(index as u32, 1))
    }

    /// Replaces the geometry of a dynamic mesh.
    ///
    /// Takes slices rather than [`MeshData`] so a caller can reuse its buffers and allocate
    /// nothing per frame. An empty `indices` slice is allowed and means "draw nothing".
    /// `bounds` is the local-space sphere used for culling.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if `handle` is not a dynamic mesh or the
    /// geometry exceeds the capacity given to [`Renderer::create_dynamic_mesh`].
    pub fn write_dynamic_mesh(
        &mut self,
        handle: MeshHandle,
        vertices: &[MeshVertex],
        indices: &[crate::mesh::Index],
        bounds: engine_core::math::Sphere,
    ) -> Result<(), RendererError> {
        let index = handle.index();
        let frame = self.frames.frame_index();
        let (Some(dynamic), Some(mesh)) = (
            self.dynamic_meshes.get_mut(&index),
            self.meshes.get_mut(index),
        ) else {
            return Err(RendererError::InvalidConfig {
                reason: format!("mesh {index} is not a dynamic mesh"),
            });
        };
        dynamic.write(mesh, frame, vertices, indices, bounds)
    }

    /// Uploads a skinned mesh and returns a handle to it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn create_skinned_mesh(
        &mut self,
        data: &SkinnedMeshData,
    ) -> Result<SkinnedMeshHandle, RendererError> {
        let mesh = GpuMesh::new(
            self.context.device(),
            data.vertices(),
            data.indices(),
            data.bounds(),
        )?;
        self.note_upload();
        let index = self.skinned_meshes.len() as u32;
        self.skinned_meshes.push(mesh);
        Ok(SkinnedMeshHandle::new(index, 1))
    }

    /// Uploads a texture and returns a handle.
    ///
    /// The same image asked for twice is uploaded once: the request is answered from the
    /// content table before anything is filtered or allocated.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the texel buffer does not match the
    /// dimensions, or [`RendererError::Api`] if the texture cannot be created.
    pub fn create_texture(
        &mut self,
        image: &TextureImage<'_>,
    ) -> Result<TextureHandle, RendererError> {
        let identity = TextureIdentity::of(image)?;
        self.count_texture_request(image);
        if let Some(existing) = self.resident_texture(&identity.key, image) {
            return Ok(existing);
        }
        let prepared = identity.prepare(image);
        self.upload_prepared_texture(image, prepared)
    }

    /// [`Renderer::create_texture`] for a texture whose CPU work [`crate::prepare_texture`] has
    /// already done, on whatever thread did it.
    ///
    /// # Errors
    ///
    /// As [`Renderer::create_texture`].
    pub fn create_texture_prepared(
        &mut self,
        image: &TextureImage<'_>,
        prepared: PreparedTexture,
    ) -> Result<TextureHandle, RendererError> {
        if !prepared.describes(image) {
            // Prepared from a different image: do the work again rather than upload the
            // wrong picture.
            return self.create_texture(image);
        }
        self.count_texture_request(image);
        if let Some(existing) = self.resident_texture(&prepared.key, image) {
            return Ok(existing);
        }
        self.upload_prepared_texture(image, prepared)
    }

    /// The per-request counters, deduplicated hits included.
    fn count_texture_request(&mut self, image: &TextureImage<'_>) {
        self.texture_requests += 1;
        self.texture_bytes_naive += rgba_chain_bytes(image.width, image.height);
    }

    /// The handle of a texture with this content already resident, if there is one.
    fn resident_texture(
        &self,
        key: &TextureKey,
        image: &TextureImage<'_>,
    ) -> Option<TextureHandle> {
        let existing = self.textures_by_content.get(key)?;
        log::trace!(
            "texture: reusing {} for a {}x{} image already resident",
            existing.index(),
            image.width,
            image.height
        );
        Some(*existing)
    }

    /// Creates the texture for a prepared image and uploads its levels.
    ///
    /// Block-compressed levels are uploaded as BC1 or BC5 when the device supports them, which
    /// every Apple silicon Mac does. On a device that does not, the uncompressed chain
    /// `prepare` always produces is uploaded as RGBA8 instead.
    fn upload_prepared_texture(
        &mut self,
        image: &TextureImage<'_>,
        prepared: PreparedTexture,
    ) -> Result<TextureHandle, RendererError> {
        let PreparedTexture {
            key,
            compression,
            mip_levels,
            chain,
            compressed,
            ..
        } = prepared;

        let compressed_upload = !compressed.is_empty() && self.context.supports_block_compression();
        let (compression, levels): (Compression, Vec<&[u8]>) = if compressed_upload {
            log::debug!(
                "texture: {}x{} {:?} compressed to {:?}, {} KB against {} KB",
                image.width,
                image.height,
                image.kind,
                compression,
                compressed.iter().map(Vec::len).sum::<usize>() / 1024,
                (image.texels.len() * 4 / 3) / 1024
            );
            (compression, compressed.iter().map(Vec::as_slice).collect())
        } else {
            (
                Compression::None,
                std::iter::once(image.texels)
                    .chain(chain.iter().map(Vec::as_slice))
                    .collect(),
            )
        };

        // The pixel format carries the colour space: the texels are the same bytes either way
        // and it is the read that linearizes. BC1 has an sRGB variant; BC5 holds data.
        let format = TextureFormat::new(compression, image.is_srgb());
        crate::upload_timing::digest_texture(&levels);
        let texture = texture_with_levels(
            self.context.device(),
            image.width,
            image.height,
            format,
            &levels,
        )?;
        self.note_upload();

        let index = self.textures.len() as u32;
        self.textures.push(UploadedTexture {
            compression,
            texture,
            mip_levels,
        });
        let handle = TextureHandle(index);
        self.textures_by_content.insert(key, handle);
        self.texture_bytes += levels.iter().map(|level| level.len()).sum::<usize>();
        Ok(handle)
    }

    /// Bytes of GPU memory the resident textures occupy, and what the same images would have
    /// cost as uncompressed RGBA8 with one copy per request.
    #[must_use]
    pub fn texture_memory(&self) -> (usize, usize) {
        (self.texture_bytes, self.texture_bytes_naive)
    }

    /// How many times a texture has been asked for, deduplicated hits included.
    #[must_use]
    pub fn texture_requests(&self) -> usize {
        self.texture_requests
    }

    /// How an uploaded texture is stored, or `None` for a handle that names nothing.
    ///
    /// Reports what is on the GPU: [`Compression::None`] for a texture uploaded uncompressed
    /// because the device cannot sample BC.
    #[must_use]
    pub fn texture_compression(&self, texture: TextureHandle) -> Option<Compression> {
        self.textures
            .get(texture.index())
            .map(|entry| entry.compression)
    }

    /// How many mip levels an uploaded texture has, or `None` for a handle that names nothing.
    #[must_use]
    pub fn texture_mip_levels(&self, texture: TextureHandle) -> Option<u32> {
        self.textures
            .get(texture.index())
            .map(|entry| entry.mip_levels)
    }

    /// How many distinct textures are resident on the GPU.
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Uploads an [`engine_assets::ModelAsset`] and reports what it became.
    ///
    /// Every level of detail's every submesh becomes one mesh and one material, and the
    /// returned [`ModelBinding`] is the description extraction reads.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] carrying the validation failure, or
    /// [`RendererError::Api`] if a Metal call fails.
    pub fn create_model(
        &mut self,
        model: &engine_assets::ModelAsset,
    ) -> Result<ModelBinding, RendererError> {
        self.create_model_prepared(model, None)
    }

    /// [`Renderer::create_model`] with its textures' CPU work done in advance by
    /// [`crate::prepare_model_textures`]. `None`, or textures prepared from a different model, does
    /// that work here.
    ///
    /// # Errors
    ///
    /// As [`Renderer::create_model`].
    pub fn create_model_prepared(
        &mut self,
        model: &engine_assets::ModelAsset,
        prepared: Option<PreparedTextures>,
    ) -> Result<ModelBinding, RendererError> {
        use engine_assets::mesh::Geometry;

        engine_assets::validate_model(model, &engine_assets::ValidationLimits::default()).map_err(
            |error| RendererError::InvalidConfig {
                reason: error.to_string(),
            },
        )?;

        let kinds = texture_kinds(model);

        // Textures first, then materials that name them, then geometry that names materials.
        // Uploading is deduplicated by content inside `create_texture`.
        let mut prepared = prepared
            .filter(|prepared| prepared.textures.len() == model.textures.len())
            .map(|prepared| prepared.textures.into_iter().map(Some).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut textures = Vec::with_capacity(model.textures.len());
        for (index, texture) in model.textures.iter().enumerate() {
            let image = TextureImage {
                width: texture.width,
                height: texture.height,
                kind: kinds[index],
                texels: &texture.texels,
            };
            let handle = match prepared.get_mut(index).and_then(Option::take) {
                Some(prepared) => self.create_texture_prepared(&image, prepared)?,
                None => self.create_texture(&image)?,
            };
            textures.push(handle);
        }

        let mut materials = Vec::with_capacity(model.materials.len());
        for material in &model.materials {
            let desc = MaterialDesc {
                base_color: material.base_color,
                roughness: material.roughness,
                metallic: material.metallic,
                emissive: material.emissive,
                normal_scale: material.normal_scale,
                occlusion_strength: material.occlusion_strength,
                alpha_cutoff: material.alpha_cutoff,
                surface: material.surface.into(),
                // Imported assets are Lambert like everything else until the game asks for
                // the BRDF.
                pbr: false,
            };
            // Every map the asset carries; a slot the asset leaves empty stays `None` and
            // resolves to that slot's shared fallback.
            let resolve =
                |index: Option<usize>| index.and_then(|index| textures.get(index).copied());
            let set = MaterialTextureSet {
                base_color: resolve(material.base_color_texture),
                normal: resolve(material.normal_texture),
                orm: resolve(material.orm_texture),
                emissive: resolve(material.emissive_texture),
            };
            materials.push(self.create_material_with_textures(desc, set)?);
        }

        let mut lods = Vec::with_capacity(model.lods.len());
        for lod in &model.lods {
            let mut parts = Vec::with_capacity(lod.submeshes.len());
            for submesh in &lod.submeshes {
                let geometry = match &submesh.geometry {
                    Geometry::Static(vertices) => {
                        let vertices = vertices
                            .iter()
                            .map(|vertex| {
                                MeshVertex::new(vertex.position, vertex.normal, vertex.uv)
                                    .with_tangent(vertex.tangent)
                                    .with_color(vertex.color)
                            })
                            .collect();
                        let data = MeshData::new(vertices, submesh.indices.clone())?;
                        PartGeometry::Rigid(self.create_mesh(&data)?)
                    }
                    Geometry::Skinned(vertices) => {
                        let vertices = vertices
                            .iter()
                            .map(|vertex| {
                                SkinnedVertex::new(
                                    vertex.position,
                                    vertex.normal,
                                    vertex.uv,
                                    vertex.joints,
                                    vertex.weights,
                                )
                                .with_tangent(vertex.tangent)
                                .with_color(vertex.color)
                            })
                            .collect();
                        let data = SkinnedMeshData::new(vertices, submesh.indices.clone())?;
                        PartGeometry::Skinned(self.create_skinned_mesh(&data)?)
                    }
                };
                let Some(material) = materials.get(submesh.material).copied() else {
                    return Err(RendererError::InvalidConfig {
                        reason: format!(
                            "'{}' submesh '{}' names material {}, and validation should have \
                             caught that",
                            model.name, submesh.name, submesh.material
                        ),
                    });
                };
                parts.push(ModelPart {
                    geometry,
                    material,
                    bounds: submesh.bounds,
                });
            }
            lods.push(ModelLodBinding {
                parts,
                min_distance: lod.min_distance,
                bounds: lod.bounds(),
            });
        }

        Ok(ModelBinding {
            lods,
            skinned: model.is_skinned(),
            // Measured from LOD0's own vertices, so a binding is scalable even for a model
            // whose author never said how tall it is.
            model_height: measured_height(model),
        })
    }

    /// Replaces the rigid geometry of a model uploaded by [`Renderer::create_model`] with
    /// `model`'s, keeping every handle — the meshes, the materials and the binding.
    ///
    /// `model` must have the shape `binding` was made from: the same levels of detail with
    /// the same number of parts. Its materials are not looked at.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] when the shapes do not match or a part is
    /// skinned, or [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn replace_model_geometry(
        &mut self,
        binding: &ModelBinding,
        model: &engine_assets::ModelAsset,
    ) -> Result<(), RendererError> {
        use engine_assets::mesh::Geometry;
        let mismatch = || RendererError::InvalidConfig {
            reason: format!(
                "'{}' does not have the levels and parts of the model it replaces",
                model.name
            ),
        };
        if model.lods.len() != binding.lods.len() {
            return Err(mismatch());
        }
        for (lod, bound) in model.lods.iter().zip(&binding.lods) {
            if lod.submeshes.len() != bound.parts.len() {
                return Err(mismatch());
            }
            for (submesh, part) in lod.submeshes.iter().zip(&bound.parts) {
                let (Geometry::Static(vertices), PartGeometry::Rigid(mesh)) =
                    (&submesh.geometry, part.geometry)
                else {
                    return Err(mismatch());
                };
                let vertices = vertices
                    .iter()
                    .map(|vertex| {
                        MeshVertex::new(vertex.position, vertex.normal, vertex.uv)
                            .with_tangent(vertex.tangent)
                            .with_color(vertex.color)
                    })
                    .collect();
                let data = MeshData::new(vertices, submesh.indices.clone())?;
                self.replace_mesh(mesh, &data)?;
            }
        }
        Ok(())
    }

    /// How many materials exist. Identical requests share one, so this counts distinct ones.
    #[must_use]
    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    /// Replaces the UI to draw this frame and batches it into vertices.
    pub fn set_ui(&mut self, list: &engine_render_data::UiDrawList) {
        self.set_ui_with(list, None);
    }

    /// [`Renderer::set_ui`] with the expansion optionally spread across `jobs`' workers.
    pub fn set_ui_with(
        &mut self,
        list: &engine_render_data::UiDrawList,
        jobs: Option<&engine_core::jobs::JobSystem>,
    ) {
        let started = Instant::now();
        self.ui.list.clear();
        self.ui.list.append(list);
        self.ui.batcher.build_with(&self.ui.list, jobs);
        self.ui.stats = self.ui.batcher.stats();
        self.ui.build_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// The list the renderer will draw, for a caller that wants to build into it directly.
    /// The caller must call [`Renderer::batch_ui`] afterwards.
    pub fn ui_list_mut(&mut self) -> &mut engine_render_data::UiDrawList {
        &mut self.ui.list
    }

    /// Batches whatever is currently in the UI list.
    pub fn batch_ui(&mut self) {
        self.batch_ui_with(None);
    }

    /// [`Renderer::batch_ui`] with the expansion optionally spread across `jobs`' workers.
    pub fn batch_ui_with(&mut self, jobs: Option<&engine_core::jobs::JobSystem>) {
        let started = Instant::now();
        self.ui.batcher.build_with(&self.ui.list, jobs);
        self.ui.stats = self.ui.batcher.stats();
        self.ui.build_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// What the last UI batch produced.
    #[must_use]
    pub fn ui_stats(&self) -> UiBatchStats {
        self.ui.stats
    }

    /// Milliseconds the last UI batch build took.
    #[must_use]
    pub fn ui_build_ms(&self) -> f64 {
        self.ui.build_ms
    }

    /// Shows or hides the whole UI pass.
    pub fn set_ui_enabled(&mut self, enabled: bool) {
        self.ui.enabled = enabled;
    }

    /// Whether the UI pass draws.
    #[must_use]
    pub fn ui_enabled(&self) -> bool {
        self.ui.enabled
    }

    /// Uploads the level's map picture for the UI to draw, replacing any before it.
    ///
    /// Straight RGBA8 with a full mip chain filtered on the CPU, so the minimap minifies it
    /// without shimmering. Unlike D3D12 this does not wait for the GPU: a frame still in flight
    /// holds its own reference to the previous picture.
    ///
    /// # Errors
    ///
    /// [`RendererError::InvalidConfig`] for texels that do not match the size, or the Metal
    /// failure creating the texture.
    pub fn set_ui_map_texture(
        &mut self,
        width: u32,
        height: u32,
        texels: &[u8],
    ) -> Result<(), RendererError> {
        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 || texels.len() != expected {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a {width}x{height} map needs {expected} bytes, got {}",
                    texels.len()
                ),
            });
        }
        let chain =
            crate::mipmap::generate(texels, width, height, crate::mipmap::TextureKind::Data);
        let mut levels: Vec<&[u8]> = Vec::with_capacity(chain.len() + 1);
        levels.push(texels);
        levels.extend(chain.iter().map(Vec::as_slice));
        let texture = texture_with_levels(
            self.context.device(),
            width,
            height,
            TextureFormat::new(Compression::None, false),
            &levels,
        )?;
        self.note_upload();
        self.ui.map = Some(UiMapPicture {
            texture,
            size: (width, height),
            bytes: levels.iter().map(|level| level.len()).sum(),
        });
        Ok(())
    }

    /// Forgets the map picture: `UiCommand::MapImage` draws nothing until the next one.
    ///
    /// # Errors
    ///
    /// Never fails; the signature matches the D3D12 renderer's.
    pub fn clear_ui_map_texture(&mut self) -> Result<(), RendererError> {
        self.ui.map = None;
        Ok(())
    }

    /// The map picture's size in texels and its GPU memory in bytes, if one is set.
    #[must_use]
    pub fn ui_map_texture(&self) -> Option<((u32, u32), usize)> {
        self.ui.map.as_ref().map(|map| (map.size, map.bytes))
    }

    /// Asks for the next frame recorded — interface, overlay and all, exactly as presented — to
    /// be kept. Collect it with [`Renderer::take_screenshot`] after that frame is presented.
    ///
    /// # Errors
    ///
    /// The readback buffer's creation failure.
    pub fn request_screenshot(&mut self) -> Result<(), RendererError> {
        let (width, height) = self.layer.size();
        self.screenshot = Some(Readback::new(self.context.device(), width, height)?);
        Ok(())
    }

    /// The frame [`Renderer::request_screenshot`] kept, once it has been presented. Waits for
    /// the GPU to finish it.
    ///
    /// # Errors
    ///
    /// The wait's failure.
    pub fn take_screenshot(
        &mut self,
    ) -> Result<Option<crate::offscreen::CapturedImage>, RendererError> {
        if self.recorded.is_some() {
            // Recorded but not yet presented: not finished, not collectable.
            return Ok(None);
        }
        let Some(readback) = self.screenshot.take() else {
            return Ok(None);
        };
        self.wait_for_idle()?;
        Ok(Some(crate::offscreen::CapturedImage {
            width: readback.width,
            height: readback.height,
            pixels: readback.pixels(),
        }))
    }

    /// Draws `world` through `camera` into the drawable and reads the pixels back, without
    /// presenting them.
    ///
    /// The dungeon map's bake: the real frame — the same culling, materials, textures and
    /// lights the game draws with — seen from a top-down camera, with no particles, no overlay
    /// and no interface on top. The frame is committed and waited for rather than presented,
    /// so it never reaches the screen. Synchronous and slow by design.
    ///
    /// # Errors
    ///
    /// Anything [`Renderer::render_world`] can fail with, or the wait's own failure.
    pub fn capture_world(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
    ) -> Result<crate::offscreen::CapturedImage, RendererError> {
        let (width, height) = self.layer.size();
        self.capture = Some(Readback::new(self.context.device(), width, height)?);
        self.build_render_list(world, camera);
        self.prepare_shadows(world, camera, lighting);
        let drawn = self.draw_render_list(camera, lighting, color);
        let capture = self.capture.take();
        drawn?;
        // Committed rather than presented: the serial covers the submission, the ring moves on
        // exactly as it would have, and the layer never sees the frame.
        if let Some(frame) = self.recorded.take() {
            self.commit(frame.command_buffer, frame.slot);
        }
        self.wait_for_idle()?;
        let Some(capture) = capture else {
            return Err(RendererError::InvalidConfig {
                reason: "the capture target vanished while its frame was recorded".to_owned(),
            });
        };
        Ok(crate::offscreen::CapturedImage {
            width,
            height,
            pixels: capture.pixels(),
        })
    }

    /// Replaces this frame's particles.
    ///
    /// A copy rather than a borrow, for the same reason [`Renderer::set_ui`] copies. Use
    /// [`Renderer::particle_list_mut`] to build into the renderer's own list and skip the copy.
    pub fn set_particles(&mut self, list: &engine_render_data::ParticleDrawList) {
        self.particles.list.clear();
        for blend in engine_render_data::ParticleBlend::ALL {
            for particle in list.of(blend) {
                self.particles.list.push(blend, *particle);
            }
        }
    }

    /// The list the renderer will draw, for a caller that fills it directly.
    pub fn particle_list_mut(&mut self) -> &mut engine_render_data::ParticleDrawList {
        &mut self.particles.list
    }

    /// Particles submitted by the most recent frame.
    #[must_use]
    pub fn particles_last_frame(&self) -> u32 {
        self.particles.drawn
    }

    /// What the camera fades out of its way from the next frame on, or `None` for nothing.
    ///
    /// Only entities whose style is [`engine_render_data::InstanceStyle::see_through`] are
    /// affected, and only by a dither in the pass they are already drawn in. Kept until it is
    /// set again.
    pub fn set_camera_fade(&mut self, fade: Option<engine_render_data::CameraFade>) {
        self.camera_fade = fade;
    }

    /// Turns the whole particle pass on or off.
    pub fn set_particles_enabled(&mut self, enabled: bool) {
        self.particles.enabled = enabled;
    }

    /// Whether the particle pass draws.
    #[must_use]
    pub fn particles_enabled(&self) -> bool {
        self.particles.enabled
    }

    /// Uploads the four shared fallback maps and returns them as a texture set.
    ///
    /// One 1x1 texture per slot, created once at renderer construction and pointed at by every
    /// material that leaves that slot empty. The texels and colour spaces come from
    /// [`MaterialTextureSlot`], so the offscreen paths build the identical set.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a texture cannot be created.
    pub(super) fn create_fallback_textures(&mut self) -> Result<MaterialTextures, RendererError> {
        let mut handles = [TextureHandle(0); MATERIAL_TEXTURE_COUNT as usize];
        for slot in MaterialTextureSlot::ALL {
            handles[slot.register() as usize] = self.create_texture(&TextureImage {
                width: 1,
                height: 1,
                kind: slot.texture_kind(),
                texels: &slot.fallback_texel(),
            })?;
        }
        Ok(MaterialTextures {
            base_color: handles[MaterialTextureSlot::BaseColor.register() as usize],
            normal: handles[MaterialTextureSlot::Normal.register() as usize],
            orm: handles[MaterialTextureSlot::Orm.register() as usize],
            emissive: handles[MaterialTextureSlot::Emissive.register() as usize],
        })
    }

    /// The renderer's shared fallback maps, for tests and diagnostics.
    #[must_use]
    pub fn fallback_textures(&self) -> MaterialTextures {
        self.fallback_textures
    }

    /// Uploads a material with no maps of its own and returns a handle to it.
    ///
    /// Every slot resolves to its shared fallback, so there is no such thing as an untextured
    /// material at the ABI.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters, or
    /// [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn create_material(&mut self, desc: MaterialDesc) -> Result<MaterialHandle, RendererError> {
        self.create_material_with_textures(desc, MaterialTextureSet::default())
    }

    /// Uploads a material that samples `texture` for its base colour.
    ///
    /// `None` binds the shared white texel. The map multiplies the material's base colour
    /// rather than replacing it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters or a texture
    /// handle that names nothing, or [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn create_material_textured(
        &mut self,
        desc: MaterialDesc,
        texture: Option<TextureHandle>,
    ) -> Result<MaterialHandle, RendererError> {
        self.create_material_with_textures(
            desc,
            MaterialTextureSet {
                base_color: texture,
                ..MaterialTextureSet::default()
            },
        )
    }

    /// Uploads a material with a full texture set and returns a handle to it.
    ///
    /// Any slot left `None` resolves to that slot's shared fallback. An identical request is
    /// answered with the material that already exists.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] for out-of-range parameters or a texture
    /// handle that names nothing, or [`RendererError::Api`] if a buffer cannot be allocated.
    pub fn create_material_with_textures(
        &mut self,
        desc: MaterialDesc,
        textures: MaterialTextureSet,
    ) -> Result<MaterialHandle, RendererError> {
        for (slot, handle) in textures.named() {
            if handle.index() >= self.textures.len() {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "material names texture {} for its {} map, and {} exist",
                        handle.index(),
                        slot.name(),
                        self.textures.len()
                    ),
                });
            }
        }
        // Before `resolve`, which replaces every empty slot with a fallback and so loses the
        // one thing the permutation needs to know: whether this material has a normal map.
        let features = ShaderFeatures {
            pbr: desc.pbr,
            normal_map: textures.normal.is_some(),
            shadows: false,
        };
        let textures = textures.resolve(&self.fallback_textures);

        // A material is fixed at creation, so an identical request can share the one that
        // exists; anything rebuilt at run time would otherwise leak one per rebuild.
        if let Some(index) = (0..self.materials.len()).find(|index| {
            self.material_descs[*index] == desc
                && self.material_features[*index] == features
                && self.material_textures[*index] == textures
        }) {
            return Ok(MaterialHandle::new(index as u32, 1));
        }

        desc.validate()?;
        let constants = buffer_with_data(
            self.context.device(),
            &[MaterialConstants::from(desc)],
            "material constants",
        )?;
        self.note_upload();

        let index = self.materials.len() as u32;
        self.materials.push(MetalMaterial { constants });
        self.material_features.push(features);
        self.material_descs.push(desc);
        self.material_surfaces.push(desc.surface);
        self.material_textures.push(textures);
        Ok(MaterialHandle::new(index, 1))
    }

    /// The texture set `material` samples, by slot.
    #[must_use]
    pub fn material_textures(&self, material: MaterialHandle) -> Option<MaterialTextures> {
        self.material_textures.get(material.index()).copied()
    }
}
