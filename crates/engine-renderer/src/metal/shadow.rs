//! The shadow stage on Metal: the atlas passes, the camera depth prepass and the half-resolution
//! contact-shadow march.
//!
//! The planning is `crate::shadow::ShadowPlan`'s, shared with D3D12; this file records what the
//! plan decided.
//!
//! ```text
//! atlas pass     one render pass, the atlas as its depth attachment, one viewport per view
//! prepass        a render pass with only the camera's depth buffer, then sampled below
//! contact pass   a full-screen triangle into the contact target, reading the depth buffer
//!                directly (no copy: a later pass may sample a texture an earlier one wrote)
//! camera pass    begun by the caller, loading the depth the prepass wrote
//! ```
//!
//! # Differences from the D3D12 recording that follow from the API
//!
//! * **Depth bias and clip mode are encoder state.** D3D12 bakes `DepthBias`,
//!   `SlopeScaledDepthBias` and `DepthClipEnable` into two sets of pipeline states (biased for
//!   the atlas, unbiased for the prepass). Here there is one set, and the atlas pass sets the
//!   bias and `Clamp` on its encoder while the prepass keeps the defaults (no bias, `Clip`).
//!
//!   The bias numbers pass through unchanged. For a float depth target D3D12 offsets by
//!   `DepthBias * 2^(exponent(max z of the primitive) - 23) + SlopeScaledDepthBias * max slope`,
//!   i.e. the integer counts units of the smallest resolvable difference at that primitive's
//!   depth. Metal's `setDepthBias:slopeScale:clamp:` defines its constant term the same way
//!   (a multiple of the smallest value guaranteed to produce a resolvable difference) and the
//!   atlas is `Depth32Float` in both, so `constant` becomes the `f32` depth bias, `slope` the
//!   slope scale and the clamp stays zero (no clamp), as in the D3D12 state.
//! * **The atlas is cleared whole**, as D3D12 clears it: the static-shadow cache keeps caster
//!   *lists* across frames, not depth, so every frame redraws every view's casters into a
//!   cleared atlas. No per-rectangle clear exists or is needed.
//! * **No depth copy.** The contact march samples the camera's depth buffer in place.

use std::time::Instant;

use engine_core::math::Frustum;
use engine_render_data::{Camera, MaterialHandle, SkinnedDrawList};
use objc2_metal::{
    MTLCommandBuffer, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRenderPassDescriptor,
    MTLStoreAction,
};

use super::draw::{FramePalettes, upload_instances, upload_padded};
use super::library::{Function, Libraries};
use super::pipelines::{DEPTH_FORMAT, DepthRule, DepthStates, Pipeline, Raster, build_pass_state};
use super::recorder::{
    FRAME_SLOT, INSTANCE_STREAM, MATERIAL_SLOT, OBJECT_SLOT, Recorder, SKIN_SLOT, VERTEX_STREAM,
};
use super::renderer::{MetalMaterial, Renderer, UploadedTexture};
use super::resources::{GpuMesh, color_target_texture, depth_texture};
use super::upload::{Allocation, UploadRing};
use super::{Device, Texture};
use crate::depth::DEPTH_CLEAR;
use crate::error::RendererError;
use crate::material::{MaterialDesc, MaterialTextureSlot, MaterialTextures};
use crate::mesh_pipeline::InstanceTransform;
use crate::render_list::{INSTANCE_STREAM_ALIGNMENT, RenderList};
use crate::scene::ObjectConstants;
use crate::shadow::atlas::AtlasRect;
use crate::shadow::caster::skinned_caster_visible;
use crate::shadow::pipeline::{ShadowGeometry, ShadowSurface};
use crate::shadow::plan::ShadowViewConstants;
use crate::shadow::resources::SHADOW_CLEAR;
use crate::shadow::static_cache::{StaticShadowCache, StaticView};
use crate::shadow::stats::ShadowPassStats;
use crate::shadow::{ShadowAtlasLayout, ShadowSettings};

/// The contact target's format: red the shadow factor, green the view distance
/// (D3D12 `CONTACT_FORMAT`, `R16G16_FLOAT`).
const CONTACT_FORMAT: MTLPixelFormat = MTLPixelFormat::RG16Float;

/// Fragment texture slot the contact march reads the camera's depth from (`contact.metal`).
const SCENE_DEPTH_SLOT: usize = 5;

/// The four depth-only pipelines, shared by the atlas and the prepass.
///
/// D3D12 keeps two sets because bias is part of its pipeline state; here it is encoder state, so
/// one set serves both.
pub(super) struct DepthPipelines {
    instanced_opaque: Pipeline,
    instanced_alpha: Pipeline,
    skinned_opaque: Pipeline,
    skinned_alpha: Pipeline,
}

impl DepthPipelines {
    /// Builds the pipelines: no colour attachment, `Depth32Float`, and no fragment function for
    /// the opaque ones.
    fn new(device: &Device, libraries: &mut Libraries) -> Result<Self, RendererError> {
        let instanced = libraries.function("shadow_depth", "shadow_depth_instanced_vs")?;
        let skinned = libraries.function("shadow_depth", "shadow_depth_skinned_vs")?;
        let alpha = libraries.function("shadow_depth", "shadow_depth_alpha_fs")?;
        let make = |vertex: &Function,
                    fragment: Option<&Function>,
                    raster: Raster,
                    what: &'static str|
         -> Result<Pipeline, RendererError> {
            Ok(Pipeline {
                state: build_pass_state(device, vertex, fragment, None, Some(DEPTH_FORMAT), what)?,
                // LESS_EQUAL, writing: `shadow::pipeline::depth_stencil`.
                depth: DepthRule::Write,
                raster,
            })
        };
        Ok(Self {
            // Back faces culled for opaque geometry, exactly as in the camera pass; cutouts are
            // drawn two-sided (`shadow::pipeline::rasterizer`).
            instanced_opaque: make(
                &instanced,
                None,
                Raster::SOLID,
                "shadow depth, instanced, opaque",
            )?,
            instanced_alpha: make(
                &instanced,
                Some(&alpha),
                Raster::DOUBLE_SIDED,
                "shadow depth, instanced, alpha-tested",
            )?,
            skinned_opaque: make(
                &skinned,
                None,
                Raster::SOLID,
                "shadow depth, skinned, opaque",
            )?,
            skinned_alpha: make(
                &skinned,
                Some(&alpha),
                Raster::DOUBLE_SIDED,
                "shadow depth, skinned, alpha-tested",
            )?,
        })
    }

    /// The pipeline for one combination of geometry and surface.
    fn get(&self, geometry: ShadowGeometry, surface: ShadowSurface) -> &Pipeline {
        match (geometry, surface) {
            (ShadowGeometry::Instanced, ShadowSurface::Opaque) => &self.instanced_opaque,
            (ShadowGeometry::Instanced, ShadowSurface::AlphaTested) => &self.instanced_alpha,
            (ShadowGeometry::Skinned, ShadowSurface::Opaque) => &self.skinned_opaque,
            (ShadowGeometry::Skinned, ShadowSurface::AlphaTested) => &self.skinned_alpha,
        }
    }
}

/// The shadow atlas, the contact-shadow target and the pipelines that fill them.
///
/// Both textures always exist and are always valid to bind: one texel when shadows (or contact
/// shadows) are off, as on D3D12, so the camera pass needs no separate placeholder.
pub(super) struct ShadowStage {
    /// `Depth32Float`, rendered into by the atlas pass and sampled by the camera pass.
    pub(super) atlas: Texture,
    atlas_size: (u32, u32),
    /// `RG16Float`, rendered into by the contact pass and sampled by the camera pass.
    pub(super) contact: Texture,
    contact_size: (u32, u32),
    /// Whether `contact` is a real target rather than the one-texel stand-in.
    contact_live: bool,
    depth_pipelines: DepthPipelines,
    /// Built the first time contact shadows are drawn.
    contact_pipeline: Option<Pipeline>,
}

impl ShadowStage {
    /// Creates the textures for `settings` and a camera of `scene_size`, and the depth
    /// pipelines.
    ///
    /// # Errors
    ///
    /// [`RendererError::ShaderCompilation`] or [`RendererError::Api`] if a pipeline or texture
    /// cannot be created.
    pub(super) fn new(
        device: &Device,
        libraries: &mut Libraries,
        settings: &ShadowSettings,
        scene_size: (u32, u32),
    ) -> Result<Self, RendererError> {
        let atlas_size = Self::atlas_extent(settings);
        let (contact_width, contact_height, contact_live) =
            Self::contact_extent(settings, scene_size);
        Ok(Self {
            atlas: depth_texture(device, atlas_size.0, atlas_size.1)?,
            atlas_size,
            contact: color_target_texture(device, contact_width, contact_height, CONTACT_FORMAT)?,
            contact_size: (contact_width, contact_height),
            contact_live,
            depth_pipelines: DepthPipelines::new(device, libraries)?,
            contact_pipeline: None,
        })
    }

    /// The atlas size the layout asks for, at least one texel.
    fn atlas_extent(settings: &ShadowSettings) -> (u32, u32) {
        let (width, height) = ShadowAtlasLayout::new(settings).size();
        (width.max(1), height.max(1))
    }

    /// The contact target's size, and whether it is a real one: one texel unless contact
    /// shadows are on.
    fn contact_extent(settings: &ShadowSettings, scene_size: (u32, u32)) -> (u32, u32, bool) {
        if settings.enabled && settings.contact.enabled {
            let (width, height) = settings
                .contact
                .target_size(scene_size.0.max(1), scene_size.1.max(1));
            (width, height, true)
        } else {
            (1, 1, false)
        }
    }

    /// Recreates whichever texture `settings` and `scene_size` no longer fit.
    ///
    /// The caller must have waited for the GPU.
    ///
    /// # Errors
    ///
    /// [`RendererError::Api`] if a texture cannot be created.
    pub(super) fn resize(
        &mut self,
        device: &Device,
        settings: &ShadowSettings,
        scene_size: (u32, u32),
    ) -> Result<(), RendererError> {
        let atlas_size = Self::atlas_extent(settings);
        if atlas_size != self.atlas_size {
            self.atlas = depth_texture(device, atlas_size.0, atlas_size.1)?;
            self.atlas_size = atlas_size;
        }
        let (width, height, live) = Self::contact_extent(settings, scene_size);
        if (width, height) != self.contact_size {
            self.contact = color_target_texture(device, width, height, CONTACT_FORMAT)?;
            self.contact_size = (width, height);
        }
        self.contact_live = live;
        Ok(())
    }

    /// The atlas texture's size in texels, which the frame constants are written against.
    pub(super) fn atlas_size(&self) -> (u32, u32) {
        self.atlas_size
    }

    /// The contact target's size, or `None` when the pass does not run.
    pub(super) fn contact_target_size(&self) -> Option<(u32, u32)> {
        self.contact_live.then_some(self.contact_size)
    }

    /// Builds the contact pipeline if it does not exist yet.
    fn ensure_contact_pipeline(
        &mut self,
        device: &Device,
        libraries: &mut Libraries,
    ) -> Result<(), RendererError> {
        if self.contact_pipeline.is_some() {
            return Ok(());
        }
        let vertex = libraries.function("contact", "contact_vs")?;
        let fragment = libraries.function("contact", "contact_fs")?;
        self.contact_pipeline = Some(Pipeline {
            state: build_pass_state(
                device,
                &vertex,
                Some(&fragment),
                Some(CONTACT_FORMAT),
                None,
                "contact shadows",
            )?,
            // No depth attachment: nothing tested, nothing written.
            depth: DepthRule::Off,
            // A full-screen triangle has one winding and no back face worth culling.
            raster: Raster::DOUBLE_SIDED,
        });
        Ok(())
    }
}

/// Where a batch loop's per-instance data comes from.
enum InstanceSource<'a> {
    /// Composed from the list's own transforms into this frame's ring.
    Compose,
    /// Composed already, in the list's batch order, and only copied into this frame's ring: a
    /// static view's casters, kept from the frame they were gathered in.
    Cached(&'a [InstanceTransform]),
    /// Already in the ring, in the list's batch order: what the camera pass uploaded for the
    /// same list, which the prepass binds instead of composing it a second time.
    Uploaded(Allocation),
}

/// The renderer's meshes and materials, borrowed for one depth pass.
struct DepthScene<'a> {
    meshes: &'a [GpuMesh],
    skinned_meshes: &'a [GpuMesh],
    materials: &'a [MetalMaterial],
    descs: &'a [MaterialDesc],
    sets: &'a [MaterialTextures],
    textures: &'a [UploadedTexture],
    /// This pass's skinned draws: the unculled set for the atlas, the camera's for the prepass.
    skinned: &'a SkinnedDrawList,
    palettes: &'a FramePalettes,
}

/// Everything a depth pass records with, borrowed from the renderer field by field so the
/// shadow plan stays free to be read and written alongside it.
struct DepthRecording<'a> {
    recorder: &'a mut Recorder,
    uploads: &'a mut UploadRing,
    instances: &'a mut Vec<InstanceTransform>,
    palette: &'a mut Vec<[f32; 16]>,
    depth_states: &'a DepthStates,
    pipelines: &'a DepthPipelines,
    scene: DepthScene<'a>,
    slot: usize,
}

impl DepthRecording<'_> {
    /// Uploads and binds a view's constants at buffer 0.
    fn bind_view(&mut self, view: &ShadowViewConstants) -> Result<(), RendererError> {
        let allocation = self.uploads.write_constants(self.slot, view)?;
        self.recorder.set_constants(
            FRAME_SLOT,
            self.uploads.buffer(self.slot, allocation),
            allocation.offset(),
        );
        Ok(())
    }

    /// Binds the two things the alpha-test fragment function reads: the material constants and
    /// the base colour map.
    fn bind_alpha_material(
        &mut self,
        handle: MaterialHandle,
        context: &str,
    ) -> Result<(), RendererError> {
        let (Some(material), Some(set)) = (
            self.scene.materials.get(handle.index()),
            self.scene.sets.get(handle.index()),
        ) else {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{context} names material {} which does not exist",
                    handle.index()
                ),
            });
        };
        let Some(base_color) = self
            .scene
            .textures
            .get(set.slot(MaterialTextureSlot::BaseColor).index())
        else {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "alpha-tested material {} has no base colour map",
                    handle.index()
                ),
            });
        };
        self.recorder
            .set_fragment_buffer(MATERIAL_SLOT, &material.constants, 0);
        self.recorder.set_fragment_texture(
            MaterialTextureSlot::BaseColor.register() as usize,
            &base_color.texture,
        );
        Ok(())
    }

    /// Which depth-only surface `handle`'s material needs: the cutout pipeline if it is
    /// alpha-tested, the fragment-less one otherwise.
    fn surface_of(
        &self,
        handle: MaterialHandle,
        context: &str,
    ) -> Result<ShadowSurface, RendererError> {
        let Some(desc) = self.scene.descs.get(handle.index()) else {
            return Err(RendererError::InvalidConfig {
                reason: format!(
                    "{context} names material {} which does not exist",
                    handle.index()
                ),
            });
        };
        Ok(if desc.is_alpha_tested() {
            ShadowSurface::AlphaTested
        } else {
            ShadowSurface::Opaque
        })
    }

    /// Records a render list's batches as instanced depth-only draws.
    fn record_batches(
        &mut self,
        list: &RenderList,
        source: InstanceSource<'_>,
    ) -> Result<(), RendererError> {
        // One instance stream for the whole list, in batch order, bound once: each batch is then
        // a draw starting at its own `first`.
        let stream = match source {
            InstanceSource::Uploaded(allocation) => Some(allocation),
            InstanceSource::Compose => {
                upload_instances(list, self.uploads, self.slot, self.instances)?
            }
            InstanceSource::Cached([]) => None,
            InstanceSource::Cached(instances) => Some(self.uploads.write(
                self.slot,
                instances,
                INSTANCE_STREAM_ALIGNMENT,
            )?),
        };
        let Some(stream) = stream else {
            return Ok(());
        };
        self.recorder.set_vertex_buffer(
            INSTANCE_STREAM,
            self.uploads.buffer(self.slot, stream),
            stream.offset(),
        );
        for batch in list.batches() {
            let Some(mesh) = self.scene.meshes.get(batch.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a shadow list names mesh {} which does not exist",
                        batch.mesh.index()
                    ),
                });
            };
            let surface = self.surface_of(batch.material, "a shadow list")?;
            self.recorder.set_pipeline(
                self.pipelines.get(ShadowGeometry::Instanced, surface),
                self.depth_states,
            );
            // Only the cutout path reads the material at all: the opaque pipeline has no
            // fragment function.
            if surface == ShadowSurface::AlphaTested {
                self.bind_alpha_material(batch.material, "a shadow list")?;
            }
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

    /// Records this pass's skinned casters, culled against `frustum`. Returns how many were
    /// drawn.
    ///
    /// `camera_depth` selects the prepass's rule over the atlas's: every whole character the
    /// camera draws is in the camera's depth, caster or not, and a faded one is not.
    fn record_skinned(
        &mut self,
        frustum: &Frustum,
        camera_depth: bool,
    ) -> Result<u32, RendererError> {
        if self.scene.skinned.is_empty() {
            return Ok(0);
        }
        let mut drawn = 0;
        let mut bound_palette: Option<(u32, u32)> = None;

        for index in 0..self.scene.skinned.len() {
            let Some(instance) = self.scene.skinned.instances().get(index).copied() else {
                break;
            };
            if camera_depth {
                // The camera's depth, not a light's: a faded character is drawn see-through
                // over the world rather than into it, so the world behind it must be in the
                // depth.
                if instance.style.is_faded() || !frustum.intersects_sphere(&instance.bounds) {
                    continue;
                }
            } else if !skinned_caster_visible(&instance, frustum) {
                continue;
            }
            let Some(mesh) = self.scene.skinned_meshes.get(instance.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a skinned shadow caster names skinned mesh {} which does not exist",
                        instance.mesh.index()
                    ),
                });
            };
            let surface = self.surface_of(instance.material, "a skinned shadow caster")?;
            self.recorder.set_pipeline(
                self.pipelines.get(ShadowGeometry::Skinned, surface),
                self.depth_states,
            );
            if surface == ShadowSurface::AlphaTested {
                self.bind_alpha_material(instance.material, "a skinned shadow caster")?;
            }

            let range = (instance.palette_start, instance.palette_len);
            if bound_palette != Some(range) {
                // Uploaded once for the frame before any pass was recorded; this only binds it.
                // The upload is the fallback for a palette the frame did not know about.
                let allocation = match self.scene.palettes.address(range.0, range.1) {
                    Some(allocation) => allocation,
                    None => {
                        let Some(palette) = self.scene.skinned.palette_of(index) else {
                            return Err(RendererError::InvalidConfig {
                                reason: format!(
                                    "a skinned shadow caster names bones {}..{} of a {}-matrix palette",
                                    instance.palette_start,
                                    instance.palette_start + instance.palette_len,
                                    self.scene.skinned.palette().len()
                                ),
                            });
                        };
                        upload_padded(palette, self.palette, self.uploads, self.slot)?
                    }
                };
                self.recorder.set_vertex_buffer(
                    SKIN_SLOT,
                    self.uploads.buffer(self.slot, allocation),
                    allocation.offset(),
                );
                bound_palette = Some(range);
            }

            let object = ObjectConstants::styled(instance.transform.matrix(), instance.style);
            let allocation = self.uploads.write_constants(self.slot, &object)?;
            self.recorder.set_constants(
                OBJECT_SLOT,
                self.uploads.buffer(self.slot, allocation),
                allocation.offset(),
            );
            self.recorder
                .set_vertex_buffer(VERTEX_STREAM, mesh.vertex_buffer(), 0);
            self.recorder
                .draw_indexed(mesh.index_buffer(), mesh.index_count(), 1, 0);
            drawn += 1;
        }
        Ok(drawn)
    }

    /// Records one atlas view: its tile, its constants, its static casters, then its movers and
    /// characters. Returns what it drew and how many skinned casters that included.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site, and every argument is a different thing the view needs;
                  bundling them would be a struct that exists for one function"
    )]
    fn record_view(
        &mut self,
        rect: AtlasRect,
        view: &ShadowViewConstants,
        statics: Option<&StaticView>,
        list: &RenderList,
        frustum: &Frustum,
        characters: bool,
    ) -> Result<(ShadowPassStats, u32), RendererError> {
        let before = self.recorder.stats();
        self.recorder
            .set_viewport_rect(rect.x, rect.y, rect.size, rect.size);
        self.bind_view(view)?;

        // The static casters first, from the cache, then the movers and the characters.
        if let Some(statics) = statics {
            self.record_batches(&statics.list, InstanceSource::Cached(&statics.instances))?;
        }
        self.record_batches(list, InstanceSource::Compose)?;
        let skinned = if characters {
            // Its shadow, unlike its depth, is whole.
            self.record_skinned(frustum, false)?
        } else {
            0
        };

        let after = self.recorder.stats();
        Ok((
            ShadowPassStats {
                casters: 0,
                draw_calls: after.draw_calls - before.draw_calls,
                triangles: after.triangles - before.triangles,
            },
            skinned,
        ))
    }
}

impl Renderer {
    /// Records the shadow stage: the atlas, the camera's depth prepass and the contact march,
    /// each its own render pass, all before the camera pass.
    ///
    /// Sets `depth_primed` when the prepass ran, which is also what tells the camera pass to
    /// load the depth buffer instead of clearing it. `frame` is the frame constants the contact
    /// march reads.
    pub(super) fn record_shadow_stage(
        &mut self,
        slot: usize,
        camera: &Camera,
        frame: Allocation,
    ) -> Result<(), RendererError> {
        if self.shadow.settings().enabled {
            self.record_atlas(slot)?;
        }
        let prepass = self.shadow.needs_prepass();
        if prepass {
            self.record_prepass(slot, camera)?;
            self.depth_primed = true;
        }
        if prepass && self.shadow.needs_depth_copy() {
            self.record_contact(slot, frame)?;
        }
        Ok(())
    }

    /// One depth pass per cascade and per local view, each a viewport into the same texture,
    /// after a single clear of the whole atlas.
    fn record_atlas(&mut self, slot: usize) -> Result<(), RendererError> {
        let started = Instant::now();
        let (width, height) = self.shadow_stage.atlas_size();
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        let depth = pass.depthAttachment();
        depth.setTexture(Some(&self.shadow_stage.atlas));
        depth.setLoadAction(MTLLoadAction::Clear);
        depth.setClearDepth(f64::from(SHADOW_CLEAR));
        depth.setStoreAction(MTLStoreAction::Store);
        self.begin_pass(&pass, width, height)?;

        // Clamping rather than clipping: a caster nearer the light than the cascade's near
        // plane still writes depth, which is what shadows from geometry far behind a cascade.
        self.recorder.set_depth_clamp(true);
        let bias = self.shadow.settings().bias;
        self.recorder
            .set_depth_bias(bias.constant as f32, bias.slope, 0.0);

        // Out of the plan for the loops so a view can lend its static casters to the recording
        // while the plan's statistics are written. Put back below, on every path.
        let cache = std::mem::take(&mut self.shadow.static_cache);
        let result = self.record_atlas_views(slot, &cache);
        self.shadow.static_cache = cache;
        self.end_pass();
        result?;

        self.shadow.stats.atlas_ms = started.elapsed().as_secs_f64() * 1e3;
        self.shadow.stats.cpu_ms += self.shadow.stats.atlas_ms;
        Ok(())
    }

    /// Records every cascade and every local view, each with the static casters `cache` holds
    /// for it, into the open atlas pass.
    fn record_atlas_views(
        &mut self,
        slot: usize,
        cache: &StaticShadowCache,
    ) -> Result<(), RendererError> {
        let mut recording = DepthRecording {
            recorder: &mut self.recorder,
            uploads: &mut self.uploads,
            instances: &mut self.instance_scratch,
            palette: &mut self.palette_scratch,
            depth_states: &self.depth_states,
            pipelines: &self.shadow_stage.depth_pipelines,
            scene: DepthScene {
                meshes: &self.meshes,
                skinned_meshes: &self.skinned_meshes,
                materials: &self.materials,
                descs: &self.material_descs,
                sets: &self.material_textures,
                textures: &self.textures,
                skinned: &self.shadow_skinned,
                palettes: &self.frame_palettes,
            },
            slot,
        };
        let plan = &mut self.shadow;

        for index in 0..plan.cascades.len() {
            let Some(rect) = plan.cascade_rects.get(index).copied() else {
                continue;
            };
            let view = ShadowViewConstants::new(plan.cascades[index].view_projection, true);
            let frustum = plan.cascades[index].frustum();
            let statics = cache.cascade_view(index);
            let characters = plan.far_movers || index + 1 < plan.cascades.len();
            let (pass, skinned) = recording.record_view(
                rect,
                &view,
                statics,
                &plan.cascade_lists[index],
                &frustum,
                characters,
            )?;
            plan.stats.skinned_draws += skinned;
            plan.stats.cascades[index].draw_calls = pass.draw_calls;
            plan.stats.cascades[index].triangles = pass.triangles;
        }

        for index in 0..plan.local_views.len() {
            let Some(rect) = plan.local_rects.get(index).copied() else {
                continue;
            };
            let local = plan.local_views[index];
            let view = ShadowViewConstants::new(local.view_projection, false);
            let frustum = local.frustum();
            let statics = cache
                .enabled()
                .then(|| cache.local_view(local.light, local.face.unwrap_or(0)))
                .flatten();
            let (pass, skinned) = recording.record_view(
                rect,
                &view,
                statics,
                &plan.local_lists[index],
                &frustum,
                true,
            )?;
            plan.stats.skinned_draws += skinned;
            plan.stats.local.draw_calls += pass.draw_calls;
            plan.stats.local.triangles += pass.triangles;
        }
        Ok(())
    }

    /// The camera's depth prepass, into the camera's own depth buffer.
    ///
    /// The same depth-only pipelines the atlas uses, unbiased and clipping (the encoder's
    /// defaults), with the camera's view-projection, drawing the camera's own lists: exactly what
    /// the camera pass then draws primed.
    fn record_prepass(&mut self, slot: usize, camera: &Camera) -> Result<(), RendererError> {
        let started = Instant::now();
        let (width, height) = self.layer.size();
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        let depth = pass.depthAttachment();
        depth.setTexture(Some(&self.depth));
        depth.setLoadAction(MTLLoadAction::Clear);
        depth.setClearDepth(f64::from(DEPTH_CLEAR));
        // Read by the camera pass (loaded) and, with contact shadows, by the march.
        depth.setStoreAction(MTLStoreAction::Store);
        self.begin_pass(&pass, width, height)?;

        let mut recording = DepthRecording {
            recorder: &mut self.recorder,
            uploads: &mut self.uploads,
            instances: &mut self.instance_scratch,
            palette: &mut self.palette_scratch,
            depth_states: &self.depth_states,
            pipelines: &self.shadow_stage.depth_pipelines,
            scene: DepthScene {
                meshes: &self.meshes,
                skinned_meshes: &self.skinned_meshes,
                materials: &self.materials,
                descs: &self.material_descs,
                sets: &self.material_textures,
                textures: &self.textures,
                skinned: &self.skinned,
                palettes: &self.frame_palettes,
            },
            slot,
        };
        let view = ShadowViewConstants::new(camera.view_projection(), false);
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let source = match self.camera_instances {
            Some(allocation) => InstanceSource::Uploaded(allocation),
            None => InstanceSource::Compose,
        };
        let result = recording
            .bind_view(&view)
            .and_then(|()| recording.record_batches(&self.visible, source))
            .and_then(|()| recording.record_skinned(&frustum, true).map(|_| ()));
        self.end_pass();
        result?;

        self.shadow.stats.prepass_ms = started.elapsed().as_secs_f64() * 1e3;
        self.shadow.stats.cpu_ms += self.shadow.stats.prepass_ms;
        Ok(())
    }

    /// The contact-shadow march, into its own half-resolution target.
    ///
    /// A full-screen triangle reading the camera's depth buffer (texture 5) and the frame
    /// constants (buffer 0). Nothing is copied: the depth buffer was written by an earlier pass
    /// and is only sampled here.
    fn record_contact(&mut self, slot: usize, frame: Allocation) -> Result<(), RendererError> {
        if self.shadow_stage.contact_target_size().is_none() {
            return Ok(());
        }
        self.shadow_stage
            .ensure_contact_pipeline(self.context.device(), &mut self.libraries)?;
        let (width, height) = self.shadow_stage.contact_size;

        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0 always exists; a render pass has up to eight colour attachments.
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(&self.shadow_stage.contact));
        // Every texel is written by the triangle, so nothing is loaded.
        color.setLoadAction(MTLLoadAction::DontCare);
        color.setStoreAction(MTLStoreAction::Store);
        self.begin_pass(&pass, width, height)?;

        let Some(pipeline) = self.shadow_stage.contact_pipeline.as_ref() else {
            self.end_pass();
            return Err(RendererError::InvalidConfig {
                reason: "the contact-shadow pipeline was not built".to_owned(),
            });
        };
        self.recorder.set_pipeline(pipeline, &self.depth_states);
        self.recorder.set_fragment_buffer(
            FRAME_SLOT,
            self.uploads.buffer(slot, frame),
            frame.offset(),
        );
        self.recorder
            .set_fragment_texture(SCENE_DEPTH_SLOT, &self.depth);
        // Three vertices and no buffers: the vertex shader builds the triangle from its index.
        self.recorder.draw(MTLPrimitiveType::Triangle, 3, 1);
        self.end_pass();
        Ok(())
    }

    /// Opens a render pass on the frame's command buffer and starts recording into it, with the
    /// viewport covering `width` x `height`. The previous pass must have been ended.
    pub(super) fn begin_pass(
        &mut self,
        pass: &MTLRenderPassDescriptor,
        width: u32,
        height: u32,
    ) -> Result<(), RendererError> {
        let Some(frame) = &self.recording else {
            return Err(RendererError::InvalidConfig {
                reason: "a pass was begun without a frame being recorded".to_owned(),
            });
        };
        let Some(encoder) = frame
            .command_buffer
            .renderCommandEncoderWithDescriptor(pass)
        else {
            return Err(super::api_error(
                "MTLCommandBuffer::renderCommandEncoderWithDescriptor",
                "the command buffer returned no render encoder",
            ));
        };
        self.recorder.begin(encoder, width, height);
        Ok(())
    }

    /// Ends the open render pass and adds what it counted to the frame's totals.
    pub(super) fn end_pass(&mut self) {
        let stats = self.recorder.finish();
        super::recorder::accumulate(&mut self.frame_stats, stats);
    }
}
