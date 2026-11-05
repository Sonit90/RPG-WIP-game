//! Recording the camera pass: the same passes, in the same order, as the D3D12 renderer.
//!
//! ```text
//! opaque batches -> dithered -> skinned -> transparent -> translucent characters
//!                -> particles -> debug overlay -> UI
//! ```
//!
//! All of it goes into one render command encoder, opened once the shadow stage (`shadow.rs`:
//! the atlas, the depth prepass and the contact march) has recorded its own passes into the
//! same command buffer. The depth buffer is cleared, or loaded when the prepass wrote it.

use engine_render_data::{Camera, Lighting, MaterialHandle};
use objc2_metal::MTLPrimitiveType;

use super::pipelines::MeshKind;
use super::recorder::{
    CLUSTER_SLOT, FRAME_SLOT, INSTANCE_STREAM, MATERIAL_SLOT, OBJECT_SLOT, Recorder, SKIN_SLOT,
    VERTEX_STREAM,
};
use super::renderer::{MetalMaterial, Renderer, UploadedTexture};
use super::ui::{ATLAS_SLOT, FONT_SLOT, MAP_SLOT, UI_VERTEX_ALIGNMENT};
use super::upload::{Allocation, CONSTANT_ALIGNMENT, DEFAULT_ALIGNMENT, UploadRing};
use crate::error::RendererError;
use crate::material::{MaterialTextureSlot, MaterialTextures};
use crate::mesh_pipeline::{CharacterPass, DepthMode, InstanceTransform, PipelineVariant};
use crate::particle_pipeline::ParticleConstants;
use crate::render_list::{INSTANCE_STREAM_ALIGNMENT, RenderList};
use crate::scene::{FrameConstants, ObjectConstants};
use crate::shader::ShaderFeatures;
use crate::ui_pipeline::{UiFrameConstants, UiVertex};
use engine_core::math::Mat4;

/// Fragment texture slot of the shadow atlas, which `mesh*.metal` declare unconditionally.
const SHADOW_ATLAS_SLOT: usize = 4;
/// Fragment texture slot of the contact-shadow target.
const CONTACT_SLOT: usize = 7;

/// Which of the camera's opaque lists a recording loop walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CameraList {
    /// Everything the depth prepass draws.
    Visible,
    /// What the camera may dither this frame, which the prepass leaves out.
    Dithered,
}

/// Whether a skinned instance is drawn see-through: blended over the world in its own passes
/// rather than with the other characters. See `Renderer::draw_translucent_characters`.
fn is_translucent(instance: &engine_render_data::SkinnedInstance) -> bool {
    instance.style.is_faded()
}

/// This frame's uploaded bone palettes, by the range of the frame's palette they came from.
///
/// Each distinct palette is uploaded once, before anything is recorded, and every pass that
/// draws the character binds the allocation it got: the same table as `crate::palette`'s,
/// holding upload-ring allocations in place of GPU addresses.
#[derive(Default)]
pub(super) struct FramePalettes {
    /// `(first bone, bone count, allocation)`, in upload order. A few dozen at most.
    entries: Vec<(u32, u32, Allocation)>,
}

impl FramePalettes {
    /// An empty table.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Forgets last frame's allocations, which point into upload memory being reused.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Where this frame uploaded the palette starting at bone `start`, `len` bones long.
    pub(super) fn address(&self, start: u32, len: u32) -> Option<Allocation> {
        self.entries
            .iter()
            .find(|entry| entry.0 == start && entry.1 == len)
            .map(|entry| entry.2)
    }

    /// Uploads every palette `list`'s draws name that this frame has not uploaded yet.
    ///
    /// A draw whose range lies outside the list's palette is skipped here and reported by the
    /// pass that draws it.
    fn upload_all(
        &mut self,
        list: &engine_render_data::SkinnedDrawList,
        uploads: &mut UploadRing,
        slot: usize,
        scratch: &mut Vec<[f32; 16]>,
    ) -> Result<(), RendererError> {
        for (index, instance) in list.instances().iter().enumerate() {
            if self
                .address(instance.palette_start, instance.palette_len)
                .is_some()
            {
                continue;
            }
            let Some(palette) = list.palette_of(index) else {
                continue;
            };
            let allocation = upload_padded(palette, scratch, uploads, slot)?;
            self.entries
                .push((instance.palette_start, instance.palette_len, allocation));
        }
        Ok(())
    }
}

/// Writes `palette`, padded with the identity to the shader's array, into this frame's upload
/// memory.
///
/// The identity for the bones the character does not have: zeroes would collapse any vertex
/// that reached them to the origin. Always the shader's whole 8 KB array, so the constant
/// binding never lets the GPU read past its allocation.
pub(super) fn upload_padded(
    palette: &[Mat4],
    scratch: &mut Vec<[f32; 16]>,
    uploads: &mut UploadRing,
    slot: usize,
) -> Result<Allocation, RendererError> {
    crate::palette::pad_palette(palette, scratch);
    debug_assert_eq!(
        size_of_val(scratch.as_slice()),
        crate::palette::PALETTE_BYTES,
        "the shader reads the whole 128-matrix array"
    );
    uploads.write(slot, scratch.as_slice(), CONSTANT_ALIGNMENT)
}

/// Composes every entry's per-instance data into one allocation of this frame's upload ring,
/// in the order the batches walk. `None` for an empty list.
pub(super) fn upload_instances(
    list: &RenderList,
    uploads: &mut UploadRing,
    slot: usize,
    scratch: &mut Vec<InstanceTransform>,
) -> Result<Option<Allocation>, RendererError> {
    if list.order().is_empty() {
        return Ok(None);
    }
    list.compose_instances(scratch);
    Ok(Some(uploads.write(
        slot,
        scratch.as_slice(),
        INSTANCE_STREAM_ALIGNMENT,
    )?))
}

/// Binds a material's constants and its four maps for the fragment stage.
///
/// `context` names the list the handle came from, for the error a bad handle produces.
fn bind_material(
    recorder: &mut Recorder,
    materials: &[MetalMaterial],
    sets: &[MaterialTextures],
    textures: &[UploadedTexture],
    handle: MaterialHandle,
    context: &str,
) -> Result<(), RendererError> {
    let (Some(material), Some(set)) = (materials.get(handle.index()), sets.get(handle.index()))
    else {
        return Err(RendererError::InvalidConfig {
            reason: format!(
                "{context} names material {} which does not exist",
                handle.index()
            ),
        });
    };
    recorder.set_fragment_buffer(MATERIAL_SLOT, &material.constants, 0);
    // In `MaterialTextureSlot::ALL` order, which is texture(0..3) in the shaders.
    for slot in MaterialTextureSlot::ALL {
        let Some(uploaded) = textures.get(set.slot(slot).index()) else {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "material {} names texture {} which does not exist",
                    handle.index(),
                    set.slot(slot).index()
                ),
            });
        };
        recorder.set_fragment_texture(slot.register() as usize, &uploaded.texture);
    }
    Ok(())
}

/// The error for a draw that names a mesh no one created.
fn missing_mesh(context: &str, index: usize) -> RendererError {
    RendererError::InvalidConfig {
        reason: format!("{context} names mesh {index} which does not exist"),
    }
}

impl Renderer {
    /// Draws the stress triangle grid with the position + colour pipeline.
    pub(super) fn draw_stress_scene(&mut self) {
        self.recorder
            .set_pipeline(&self.triangle.pipeline, &self.depth_states);
        self.recorder
            .set_vertex_buffer(VERTEX_STREAM, &self.scene, 0);
        self.recorder
            .draw(MTLPrimitiveType::Triangle, self.scene_vertices, 1);
    }

    /// Records and holds a frame drawing whatever [`Renderer::build_render_list`] left in the
    /// render list.
    ///
    /// Reads the list and the GPU resources it names, and nothing else — in particular it
    /// never looks at the `RenderWorld`. By this point "what to draw" is settled and recording
    /// only has to do it. The frame is committed by [`Renderer::present`].
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if the list names a mesh or material that
    /// was never created, or [`RendererError::Api`] if recording fails.
    pub fn draw_render_list(
        &mut self,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
    ) -> Result<(), RendererError> {
        // Metal hands back autoreleased objects (the drawable, descriptors, the encoder). A game
        // loop that never returns to the run loop would never drain them, so each frame has a
        // pool of its own.
        objc2::rc::autoreleasepool(|_| self.record_render_list(camera, lighting, color))
    }

    /// The body of [`Renderer::draw_render_list`], inside the frame's autorelease pool.
    fn record_render_list(
        &mut self,
        camera: &Camera,
        lighting: Lighting,
        color: [f32; 4],
    ) -> Result<(), RendererError> {
        let slot = self.begin_frame()?;
        // Until the shadow stage has actually recorded this frame's prepass, nothing may be
        // drawn primed.
        self.depth_primed = false;
        // The camera's instances, once, for every pass that draws the visible list.
        self.camera_instances = upload_instances(
            &self.visible,
            &mut self.uploads,
            slot,
            &mut self.instance_scratch,
        )?;

        // Every character's palette, once, before any pass binds one. The shadow list is the
        // unculled superset and only matters when there are shadows.
        self.frame_palettes.clear();
        if self.shadow.settings().enabled {
            self.frame_palettes.upload_all(
                &self.shadow_skinned,
                &mut self.uploads,
                slot,
                &mut self.palette_scratch,
            )?;
        }
        self.frame_palettes.upload_all(
            &self.skinned,
            &mut self.uploads,
            slot,
            &mut self.palette_scratch,
        )?;

        let mut frame_constants =
            FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        frame_constants.set_camera(camera);
        frame_constants.set_camera_fade(self.camera_fade);
        let (viewport_width, viewport_height) = self.layer.size();
        frame_constants.set_viewport(viewport_width, viewport_height);
        self.shadow.write_frame_constants(
            &mut frame_constants,
            &self.lights,
            self.shadow_stage.atlas_size(),
        );
        frame_constants.set_light_clusters(&self.clusters);
        frame_constants.set_contact_target(self.shadow_stage.contact_target_size());
        // The particle pass is recorded later, from a function with no camera. Its basis is
        // captured here, from the same camera the scene is drawn with.
        self.particles.constants =
            ParticleConstants::new(&camera.view(), &camera.view_projection());
        let frame = self.uploads.write_constants(slot, &frame_constants)?;

        // The atlas, the camera depth prepass and the contact march, all before the camera
        // pass's own draws. The prepass sets `depth_primed`, which the camera pass answers by
        // loading the depth it wrote instead of clearing.
        self.record_shadow_stage(slot, camera, frame)?;
        self.begin_camera_pass(color, self.depth_primed)?;

        self.recorder
            .set_constants(FRAME_SLOT, self.uploads.buffer(slot, frame), frame.offset());
        // The cluster grid rides in the same upload ring: 13.5 KB of masks, read by the
        // lighting through a raw pointer.
        let clusters = self
            .uploads
            .write(slot, self.clusters.masks(), DEFAULT_ALIGNMENT)?;
        self.recorder.set_fragment_buffer(
            CLUSTER_SLOT,
            self.uploads.buffer(slot, clusters),
            clusters.offset(),
        );
        // The shader declares the shadow atlas and the contact target whether or not a function
        // constant has made their reads dead, so both are always bound: the stage keeps a
        // one-texel stand-in for each while shadows or contact shadows are off.
        self.recorder
            .set_fragment_texture(SHADOW_ATLAS_SLOT, &self.shadow_stage.atlas);
        self.recorder
            .set_fragment_texture(CONTACT_SLOT, &self.shadow_stage.contact);

        if self.instancing {
            let primed = self.primed_depth_mode();
            self.draw_opaque_instanced(slot, CameraList::Visible, primed)?;
            self.draw_opaque_instanced(slot, CameraList::Dithered, DepthMode::Write)?;
        } else {
            self.draw_opaque_per_object(slot)?;
        }
        self.draw_skinned(slot)?;
        self.draw_transparent(slot)?;
        self.draw_translucent_characters(slot)?;
        self.finish_frame(slot)
    }

    /// Draws what goes over the finished scene — particles, the overlay, the UI — and closes
    /// the frame.
    pub(super) fn finish_frame(&mut self, slot: usize) -> Result<(), RendererError> {
        // A capture is the level and nothing on top of it: no particles, no overlay, no UI.
        let capturing = self.capture.is_some();
        if !capturing {
            // Before the interface and after the scene: a blended particle has to see the
            // opaque world beneath it, and a health bar under a fireball is unreadable.
            self.draw_particles(slot)?;
            #[cfg(feature = "debug-overlay")]
            self.draw_overlay(slot)?;
            // Last, and on top of everything: the UI blends over the finished scene.
            self.draw_ui(slot)?;
        }
        self.finish_recording(slot)
    }

    /// The depth mode for a surface this frame's prepass covered: [`DepthMode::Primed`] when
    /// the prepass ran and priming is on, [`DepthMode::Write`] otherwise.
    fn primed_depth_mode(&self) -> DepthMode {
        if self.depth_primed && self.depth_priming && !self.wireframe {
            DepthMode::Primed
        } else {
            DepthMode::Write
        }
    }

    /// The shader permutation `material` asks for, with this frame's shadow state folded in.
    ///
    /// The material half was settled when the material was created; the shadow half is the
    /// renderer's: shadow sampling is compiled in exactly when the shadow system is enabled.
    fn material_variant(&self, material: MaterialHandle) -> PipelineVariant {
        let features = self
            .features_of(material)
            .with_shadows(self.shadow.settings().enabled);
        let surface = self
            .material_surfaces
            .get(material.index())
            .copied()
            .unwrap_or(crate::material::SurfaceClass::Opaque);
        PipelineVariant {
            features,
            wireframe: self.wireframe,
            surface,
            pass: CharacterPass::Lit,
            depth: DepthMode::Write,
        }
    }

    /// The material half of a material's shader features.
    fn features_of(&self, material: MaterialHandle) -> ShaderFeatures {
        self.material_features
            .get(material.index())
            .copied()
            .unwrap_or(ShaderFeatures::ALL)
    }

    /// Records the visible and dithered lists object by object: one draw, one object-constant
    /// bind per entry. The fallback path, kept for A/B measurement of instancing.
    ///
    /// Always with depth writes: the per-object path has its own vertex shader, which is not
    /// the prepass's, so its depth is not promised to equal what the prepass wrote.
    fn draw_opaque_per_object(&mut self, slot: usize) -> Result<(), RendererError> {
        for which in [CameraList::Visible, CameraList::Dithered] {
            let list = match which {
                CameraList::Visible => &self.visible,
                CameraList::Dithered => &self.dithered,
            };
            for batch in list.batches() {
                let Some(mesh) = self.meshes.get(batch.mesh.index()) else {
                    return Err(missing_mesh("render list", batch.mesh.index()));
                };
                bind_material(
                    &mut self.recorder,
                    &self.materials,
                    &self.material_textures,
                    &self.textures,
                    batch.material,
                    "render list",
                )?;
                let variant = self.material_variant(batch.material);
                let pipeline = self.pipelines.get(
                    self.context.device(),
                    &mut self.libraries,
                    MeshKind::Static,
                    variant,
                )?;
                self.recorder.set_pipeline(pipeline, &self.depth_states);
                self.recorder
                    .set_vertex_buffer(VERTEX_STREAM, mesh.vertex_buffer(), 0);

                let first = batch.first as usize;
                let last = first + batch.count as usize;
                for &entry in &list.order()[first..last] {
                    let object = ObjectConstants::shadowed(
                        list.transforms()[entry as usize].matrix(),
                        list.styles()[entry as usize],
                        list.receives_shadow()[entry as usize],
                    );
                    let allocation = self.uploads.write_constants(slot, &object)?;
                    self.recorder.set_constants(
                        OBJECT_SLOT,
                        self.uploads.buffer(slot, allocation),
                        allocation.offset(),
                    );
                    self.recorder
                        .draw_indexed(mesh.index_buffer(), mesh.index_count(), 1, 0);
                }
            }
        }
        Ok(())
    }

    /// Records one of the camera's opaque lists as instanced batches, drawn with `depth`.
    ///
    /// The pipeline and the per-batch binds are what change. Each batch binds its mesh and
    /// material once and issues one instanced draw whose first instance is the batch's offset
    /// into the list's instance stream, which is bound once for the whole list.
    fn draw_opaque_instanced(
        &mut self,
        slot: usize,
        which: CameraList,
        depth: DepthMode,
    ) -> Result<(), RendererError> {
        let stream = match which {
            CameraList::Visible => self.camera_instances,
            CameraList::Dithered => upload_instances(
                &self.dithered,
                &mut self.uploads,
                slot,
                &mut self.instance_scratch,
            )?,
        };
        let Some(stream) = stream else {
            return Ok(());
        };
        self.recorder.set_vertex_buffer(
            INSTANCE_STREAM,
            self.uploads.buffer(slot, stream),
            stream.offset(),
        );
        let list = match which {
            CameraList::Visible => &self.visible,
            CameraList::Dithered => &self.dithered,
        };
        for batch in list.batches() {
            let Some(mesh) = self.meshes.get(batch.mesh.index()) else {
                return Err(missing_mesh("render list", batch.mesh.index()));
            };
            bind_material(
                &mut self.recorder,
                &self.materials,
                &self.material_textures,
                &self.textures,
                batch.material,
                "render list",
            )?;
            let variant = self.material_variant(batch.material).with_depth(depth);
            let pipeline = self.pipelines.get(
                self.context.device(),
                &mut self.libraries,
                MeshKind::Instanced,
                variant,
            )?;
            self.recorder.set_pipeline(pipeline, &self.depth_states);
            self.recorder
                .set_vertex_buffer(VERTEX_STREAM, mesh.vertex_buffer(), 0);
            self.recorder.draw_indexed(
                mesh.index_buffer(),
                mesh.index_count(),
                batch.count,
                batch.first,
            );
        }
        Ok(())
    }

    /// Records this frame's transparent draws, farthest first.
    ///
    /// ```text
    /// opaque batches -> skinned -> TRANSPARENT -> particles -> overlay -> UI
    ///                              ^^^^^^^^^^^ this function
    /// ```
    ///
    /// One draw per object and no batching: the order is fixed by distance, and an instanced
    /// draw gives the GPU no ordering guarantee between its instances. Two adjacent entries
    /// that share a mesh and material still cost one set of binds, because the recorder drops
    /// a redundant bind.
    ///
    /// Returns how many objects were drawn.
    fn draw_transparent(&mut self, slot: usize) -> Result<u32, RendererError> {
        // Zeroed before the early return: a frame with nothing transparent must report zero.
        self.transparent_draws_last_frame = 0;
        if self.transparent.is_empty() {
            return Ok(0);
        }

        let mut drawn = 0;
        for position in 0..self.transparent.order().len() {
            let entry = self.transparent.order()[position] as usize;
            let mesh_handle = self.transparent.meshes()[entry];
            let material_handle = self.transparent.materials()[entry];

            let Some(mesh) = self.meshes.get(mesh_handle.index()) else {
                return Err(missing_mesh("the transparent list", mesh_handle.index()));
            };
            bind_material(
                &mut self.recorder,
                &self.materials,
                &self.material_textures,
                &self.textures,
                material_handle,
                "the transparent list",
            )?;
            // The same permutation the material would get in the opaque phase, with its class
            // folded in: a transparent surface differs only in blend and depth state.
            let variant = self.material_variant(material_handle);
            let pipeline = self.pipelines.get(
                self.context.device(),
                &mut self.libraries,
                MeshKind::Static,
                variant,
            )?;
            self.recorder.set_pipeline(pipeline, &self.depth_states);
            self.recorder
                .set_vertex_buffer(VERTEX_STREAM, mesh.vertex_buffer(), 0);

            let object = ObjectConstants::shadowed(
                self.transparent.transforms()[entry].matrix(),
                self.transparent.styles()[entry],
                self.transparent.receives_shadow()[entry],
            );
            let allocation = self.uploads.write_constants(slot, &object)?;
            self.recorder.set_constants(
                OBJECT_SLOT,
                self.uploads.buffer(slot, allocation),
                allocation.offset(),
            );
            self.recorder
                .draw_indexed(mesh.index_buffer(), mesh.index_count(), 1, 0);
            drawn += 1;
        }

        self.transparent_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Records this frame's skinned characters, one draw per submesh.
    ///
    /// Runs after the rigid batches and inherits the frame constants already bound; it changes
    /// only the pipeline, the palette and the per-draw object constants. No instancing: every
    /// instance would need its own palette.
    fn draw_skinned(&mut self, slot: usize) -> Result<u32, RendererError> {
        if self.skinned.is_empty() {
            self.skinned_draws_last_frame = 0;
            return Ok(0);
        }

        let mut drawn = 0;
        // A whole character is in the prepass, so it tests against its own depth there.
        let depth = self.primed_depth_mode();
        // Consecutive submeshes of one character name the same palette range, so the bind is
        // skipped for all but the first of them.
        let mut bound_palette: Option<(u32, u32)> = None;

        for index in 0..self.skinned.len() {
            let Some(instance) = self.skinned.instances().get(index).copied() else {
                break;
            };
            // A character the camera is looking through is drawn later, over everything else.
            if is_translucent(&instance) {
                continue;
            }
            self.record_skinned_instance(
                slot,
                index,
                instance,
                CharacterPass::Lit,
                depth,
                instance.style,
                &mut bound_palette,
            )?;
            drawn += 1;
        }

        self.skinned_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Records the characters the camera is looking through: each one twice, over the finished
    /// world.
    ///
    /// ```text
    /// opaque -> skinned -> transparent -> TRANSLUCENT CHARACTERS -> particles
    /// ```
    ///
    /// First every one of them writes its depth and no colour ([`CharacterPass::DepthPrime`]),
    /// then every one of them blends at its opacity, tested against that depth and writing
    /// none ([`CharacterPass::Translucent`]) — so each pixel blends only the character's
    /// nearest surface over what is behind it. A character at opacity zero is not drawn.
    fn draw_translucent_characters(&mut self, slot: usize) -> Result<u32, RendererError> {
        let any = self
            .skinned
            .instances()
            .iter()
            .any(|instance| is_translucent(instance) && instance.style.opacity > 0.0);
        if !any {
            self.translucent_character_draws_last_frame = 0;
            return Ok(0);
        }
        let mut drawn = 0;
        for pass in [CharacterPass::DepthPrime, CharacterPass::Translucent] {
            let mut bound_palette: Option<(u32, u32)> = None;
            for index in 0..self.skinned.len() {
                let Some(instance) = self.skinned.instances().get(index).copied() else {
                    break;
                };
                if !is_translucent(&instance) || instance.style.opacity <= 0.0 {
                    continue;
                }
                // The opacity becomes the tint's alpha, which is what the blend reads; the
                // dither is off, because blending is doing the fading.
                let mut style = instance.style;
                style.tint[3] *= style.opacity;
                style.opacity = 1.0;
                self.record_skinned_instance(
                    slot,
                    index,
                    instance,
                    pass,
                    DepthMode::Write,
                    style,
                    &mut bound_palette,
                )?;
                drawn += 1;
            }
        }
        self.translucent_character_draws_last_frame = drawn;
        Ok(drawn)
    }

    /// Records one skinned submesh through `pass`, drawn with `style`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one submesh's draw: which, through which pass and depth mode, styled how, with
                  the palette the run already bound"
    )]
    fn record_skinned_instance(
        &mut self,
        slot: usize,
        index: usize,
        instance: engine_render_data::SkinnedInstance,
        pass: CharacterPass,
        depth: DepthMode,
        style: engine_render_data::InstanceStyle,
        bound_palette: &mut Option<(u32, u32)>,
    ) -> Result<(), RendererError> {
        let Some(mesh) = self.skinned_meshes.get(instance.mesh.index()) else {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "a skinned draw names skinned mesh {} which does not exist",
                    instance.mesh.index()
                ),
            });
        };
        bind_material(
            &mut self.recorder,
            &self.materials,
            &self.material_textures,
            &self.textures,
            instance.material,
            "a skinned draw",
        )?;
        let variant = self
            .material_variant(instance.material)
            .with_pass(pass)
            .with_depth(depth);
        let pipeline = self.pipelines.get(
            self.context.device(),
            &mut self.libraries,
            MeshKind::Skinned,
            variant,
        )?;
        // Per draw, like the rigid batches: a character whose skin has a normal map and a
        // cloth that does not are two permutations, and the recorder skips a repeat bind.
        self.recorder.set_pipeline(pipeline, &self.depth_states);

        let range = (instance.palette_start, instance.palette_len);
        if *bound_palette != Some(range) {
            // Uploaded once for the frame in `draw_render_list`; the upload here is the
            // fallback for a palette that table does not hold.
            let allocation = match self.frame_palettes.address(range.0, range.1) {
                Some(allocation) => allocation,
                None => {
                    let Some(palette) = self.skinned.palette_of(index) else {
                        return Err(RendererError::InvalidConfig {
                            reason: format!(
                                "a skinned draw names bones {}..{} of a {}-matrix palette",
                                instance.palette_start,
                                instance.palette_start + instance.palette_len,
                                self.skinned.palette().len()
                            ),
                        });
                    };
                    upload_padded(palette, &mut self.palette_scratch, &mut self.uploads, slot)?
                }
            };
            self.recorder.set_vertex_buffer(
                SKIN_SLOT,
                self.uploads.buffer(slot, allocation),
                allocation.offset(),
            );
            *bound_palette = Some(range);
        }

        let object =
            ObjectConstants::shadowed(instance.transform.matrix(), style, instance.receives_shadow);
        let allocation = self.uploads.write_constants(slot, &object)?;
        self.recorder.set_constants(
            OBJECT_SLOT,
            self.uploads.buffer(slot, allocation),
            allocation.offset(),
        );
        self.recorder
            .set_vertex_buffer(VERTEX_STREAM, mesh.vertex_buffer(), 0);
        self.recorder
            .draw_indexed(mesh.index_buffer(), mesh.index_count(), 1, 0);
        Ok(())
    }

    /// Records the particle pass: one draw per blend mode, whatever the particle count.
    ///
    /// Each mode's records are uploaded contiguously into this frame's ring, bound as the
    /// per-instance stream, and drawn as a four-vertex triangle strip per particle — the
    /// quad's corners come from `[[vertex_id]]`, so there is no geometry buffer.
    ///
    /// Returns the particles submitted.
    fn draw_particles(&mut self, slot: usize) -> Result<u32, RendererError> {
        self.particles.drawn = 0;
        if !self.particles.enabled || self.particles.list.is_empty() {
            return Ok(0);
        }

        let constants = self.particles.constants;
        let constant_allocation = self.uploads.write_constants(slot, &constants)?;
        self.recorder.set_constants(
            FRAME_SLOT,
            self.uploads.buffer(slot, constant_allocation),
            constant_allocation.offset(),
        );

        let mut drawn = 0;
        for blend in engine_render_data::ParticleBlend::ALL {
            let instances = self.particles.list.of(blend);
            let count = instances.len();
            if count == 0 {
                continue;
            }
            let allocation =
                self.uploads
                    .write(slot, instances, super::particles::PARTICLE_ALIGNMENT)?;
            self.recorder
                .set_pipeline(self.particles.pipelines.get(blend), &self.depth_states);
            // The records are the only vertex stream, so they take slot 16 (`16 + input slot 0`).
            self.recorder.set_vertex_buffer(
                VERTEX_STREAM,
                self.uploads.buffer(slot, allocation),
                allocation.offset(),
            );
            // Four vertices, `count` instances: the whole mode is one draw call.
            self.recorder
                .draw(MTLPrimitiveType::TriangleStrip, 4, count as u32);
            drawn += count as u32;
        }

        self.particles.drawn = drawn;
        Ok(drawn)
    }

    /// Records the debug overlay: the cached developer HUD and the per-frame dynamic layer.
    ///
    /// The overlay draws clip-space position + colour vertices through the triangle pipeline,
    /// which the mesh pipelines' vertex pulling does not describe; binding it here rather than
    /// assuming whatever the scene left bound is what makes both draw paths work.
    #[cfg(feature = "debug-overlay")]
    fn draw_overlay(&mut self, slot: usize) -> Result<(), RendererError> {
        self.overlay.drawn_vertices = 0;
        if self.overlay.enabled
            && let Some((buffer, count)) = self.overlay.cached(self.context.device(), slot)?
        {
            self.recorder
                .set_pipeline(&self.triangle.pipeline, &self.depth_states);
            self.recorder.set_vertex_buffer(VERTEX_STREAM, buffer, 0);
            self.recorder.draw(MTLPrimitiveType::Triangle, count, 1);
            self.overlay.drawn_vertices = count;
        }

        // The dynamic layer is drawn whether or not the developer HUD is enabled: nameplates
        // are game UI, and hiding the statistics must not hide the game's own readouts.
        // Uploaded unconditionally, because it is rebuilt every frame by construction.
        if !self.overlay.dynamic.is_empty() {
            let vertices = self.overlay.dynamic.vertices();
            let count = vertices.len() as u32;
            let allocation = self.uploads.write(slot, vertices, DEFAULT_ALIGNMENT)?;
            self.recorder
                .set_pipeline(&self.triangle.pipeline, &self.depth_states);
            self.recorder.set_vertex_buffer(
                VERTEX_STREAM,
                self.uploads.buffer(slot, allocation),
                allocation.offset(),
            );
            self.recorder.draw(MTLPrimitiveType::Triangle, count, 1);
            self.overlay.drawn_vertices += count;
        }
        Ok(())
    }

    /// Records the UI as one draw call, if there is anything to draw.
    ///
    /// Returns the vertices submitted. The whole interface is one vertex buffer against three
    /// textures through one pipeline state: the icon atlas, the map picture (or the atlas
    /// standing in for it, so a map vertex with no map reads a valid texture) and the font.
    fn draw_ui(&mut self, slot: usize) -> Result<u32, RendererError> {
        if !self.ui.enabled {
            return Ok(0);
        }
        let vertices = self.ui.batcher.vertices();
        if vertices.is_empty() {
            return Ok(0);
        }
        let count = vertices.len() as u32;
        let allocation = self.uploads.write(slot, vertices, UI_VERTEX_ALIGNMENT)?;

        let (width, height) = self.layer.size();
        let constants = UiFrameConstants::for_viewport(width, height);
        let constant_allocation = self.uploads.write_constants(slot, &constants)?;

        self.recorder
            .set_pipeline(&self.ui.pipeline.pipeline, &self.depth_states);
        self.recorder.set_constants(
            FRAME_SLOT,
            self.uploads.buffer(slot, constant_allocation),
            constant_allocation.offset(),
        );
        self.recorder
            .set_fragment_texture(ATLAS_SLOT, &self.ui.atlas);
        let map = self
            .ui
            .map
            .as_ref()
            .map_or(&self.ui.atlas, |map| &map.texture);
        self.recorder.set_fragment_texture(MAP_SLOT, map);
        self.recorder.set_fragment_texture(FONT_SLOT, &self.ui.font);
        self.recorder.set_vertex_buffer(
            VERTEX_STREAM,
            self.uploads.buffer(slot, allocation),
            allocation.offset(),
        );
        self.recorder.draw(MTLPrimitiveType::Triangle, count, 1);
        debug_assert_eq!(size_of::<UiVertex>(), 48, "ui.metal's stride");
        Ok(count)
    }
}

/// Whether the camera pass may dither some of a rigid entity's pixels away this frame.
///
/// Conservative, against the shader's `apply_fade`: true for an entity with its own fade,
/// and for a see-through one whose bounds reach the camera fade's near zone (within
/// `near_end` of the eye) or its tube (within `radius` of the segment from the eye to the
/// target). Outside both the shader's opacity is exactly one and it never discards, so the
/// entity can be drawn into the depth prepass whole. An entity with no bounds is assumed to
/// be reachable.
pub(super) fn may_be_dithered(
    style: engine_render_data::InstanceStyle,
    fade: Option<engine_render_data::CameraFade>,
    eye: engine_core::math::Vec3,
    bounds: Option<&engine_core::math::Sphere>,
) -> bool {
    if style.is_faded() {
        return true;
    }
    if !style.see_through {
        return false;
    }
    let Some(fade) = fade else {
        return false;
    };
    let Some(bounds) = bounds else {
        return true;
    };
    // A millimetre of slack against the shader's own float arithmetic.
    const SLACK: f32 = 1e-3;
    let to_center = bounds.center - eye;
    let near_end = fade.near_end.max(fade.near_start).max(0.0);
    if to_center.length() - bounds.radius < near_end + SLACK {
        return true;
    }
    let axis = fade.target - eye;
    let length_squared = axis.dot(axis);
    let along = if length_squared > 1e-12 {
        (to_center.dot(axis) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = eye + axis * along;
    (bounds.center - closest).length() - bounds.radius < fade.radius.max(0.0) + SLACK
}

#[cfg(test)]
mod tests {
    use super::may_be_dithered;
    use engine_core::math::{Sphere, Vec3};
    use engine_render_data::{CameraFade, InstanceStyle};

    fn fade() -> CameraFade {
        CameraFade {
            target: Vec3::new(0.0, 1.5, 0.0),
            near_start: 0.2,
            near_end: 0.6,
            radius: 1.2,
            min_opacity: 0.25,
        }
    }

    const EYE: Vec3 = Vec3::new(0.0, 3.0, 8.0);

    fn at(center: Vec3, radius: f32) -> Sphere {
        Sphere::new(center, radius)
    }

    #[test]
    fn only_what_the_shader_could_dither_leaves_the_prepass() {
        let see_through = InstanceStyle::NEUTRAL.see_through();
        let solid = InstanceStyle::NEUTRAL;
        let between = at(Vec3::new(0.3, 2.2, 4.0), 0.5);
        let beside = at(Vec3::new(9.0, 1.0, 0.0), 0.5);

        // A see-through prop between the eye and the character, and one at the eye.
        assert!(may_be_dithered(
            see_through,
            Some(fade()),
            EYE,
            Some(&between)
        ));
        assert!(may_be_dithered(
            see_through,
            Some(fade()),
            EYE,
            Some(&at(EYE, 0.1))
        ));
        // Far to one side, or with no fade this frame, it is drawn whole.
        assert!(!may_be_dithered(
            see_through,
            Some(fade()),
            EYE,
            Some(&beside)
        ));
        assert!(!may_be_dithered(see_through, None, EYE, Some(&between)));
        // Solid geometry is never faded by the camera, wherever it is.
        assert!(!may_be_dithered(solid, Some(fade()), EYE, Some(&between)));
        // An entity's own fade always dithers it.
        let faded = InstanceStyle::NEUTRAL.with_opacity(0.5);
        assert!(may_be_dithered(faded, None, EYE, Some(&beside)));
    }

    #[test]
    fn the_test_is_conservative_against_the_shaders_tube() {
        // Sample points along and around the eye-to-target segment that the shader's tube
        // fades; a sphere of any size containing one of them must be classified dithered.
        let style = InstanceStyle::NEUTRAL.see_through();
        let fade = fade();
        let axis = fade.target - EYE;
        for step in 0..=10 {
            let along = EYE + axis * (step as f32 / 10.0);
            for offset in [
                Vec3::ZERO,
                Vec3::new(1.1, 0.0, 0.0),
                Vec3::new(0.0, -1.1, 0.0),
            ] {
                let point = along + offset;
                assert!(
                    may_be_dithered(style, Some(fade), EYE, Some(&at(point, 0.01))),
                    "a point {offset:?} off the line at step {step} is inside the tube"
                );
            }
        }
    }
}
