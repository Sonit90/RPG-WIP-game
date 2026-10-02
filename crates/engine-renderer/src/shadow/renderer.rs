//! The shadow stage: what it decides on the CPU, and what it records on the GPU.
//!
//! ```text
//! prepare()        cascades, caster culling, LOD, local-light selection   (no D3D12)
//!    |
//! write_frame_constants()   the matrices and rectangles the lighting pass samples with
//!    |
//! record()         one depth pass per cascade and per local view          (no visibility work)
//! ```
//!
//! The split is the one [`crate::renderer::Renderer`] already makes between
//! `build_render_list` and `draw_render_list`, and for the same reasons: "what is in the
//! shadow map" is CPU work with no device in it and is testable without a GPU, and recording
//! should only have to record.
//!
//! # What this does not know
//!
//! Nothing here mentions a character, a monster, a prop or a building. It reads
//! [`RenderWorld`] arrays and a [`SkinnedDrawList`], which is what §26 asks for: the player,
//! the enemies and the scenery all cast shadows through the same two paths, and adding a new
//! kind of entity to the game adds no code here at all.

use engine_core::math::{Frustum, Mat4, Vec3};
use engine_render_data::{Camera, Lighting, RenderWorld, SkinnedDrawList};
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_GPU_DESCRIPTOR_HANDLE, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE,
    D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
};

use crate::command::CommandContext;
use crate::descriptor::GpuDescriptorHeap;
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::material::Material;
use crate::mesh::GpuMesh;
use crate::mesh_pipeline::InstanceTransform;
use crate::render_list::RenderList;
use crate::resource::GpuTexture;
use crate::scene::{
    FrameConstants, ObjectConstants, ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_MATERIAL_TABLE,
    ROOT_OBJECT_CBV, ROOT_SKIN_CBV,
};
use crate::upload::UploadRing;

use super::atlas::AtlasRect;
use super::cascade::{CascadeView, build_cascades};
use super::caster::{ShadowCasterCollector, skinned_caster_visible};
use super::contact::{ContactShadowPipeline, ContactShadowTarget};
use super::local::{LocalShadowView, SelectedLight, select_shadow_lights};
use super::pipeline::{ShadowGeometry, ShadowPipelineSet, ShadowPipelines, ShadowSurface};
use super::resources::{SHADOW_CLEAR, ShadowResources};
use super::settings::ShadowSettings;
use super::stats::{ShadowPassStats, ShadowStats};

/// Alignment for a per-instance stream in the upload ring.
///
/// Sixteen, for the reason `renderer.rs` documents at length: a vertex buffer view needs an
/// address its elements do not straddle, and the 96-byte instance record is not a power of
/// two.
const INSTANCE_ALIGNMENT: usize = 16;

/// What one depth pass is rendered with.
///
/// Deliberately *not* the frame constants: a shadow pass runs once per cascade and once per
/// local light, and re-uploading two kilobytes of lighting data for each of them to change
/// one matrix would be most of the pass's CPU cost.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShadowViewConstants {
    /// The light's view-projection, column-major.
    pub view_projection: [f32; 16],
    /// `[1 for a cascade, 0 for a local view, 0, 0]`.
    pub params: [f32; 4],
}

impl ShadowViewConstants {
    /// The constants for one view.
    #[must_use]
    pub fn new(view_projection: Mat4, is_cascade: bool) -> Self {
        Self {
            view_projection: view_projection.to_cols_array(),
            params: [f32::from(u8::from(is_cascade)), 0.0, 0.0, 0.0],
        }
    }
}

/// The GPU resources a shadow pass draws with, borrowed from the renderer for one call.
///
/// A borrowed view rather than ownership: the meshes and materials are the renderer's, they
/// outlive any one frame, and a shadow renderer that owned copies of them would be a second
/// scene.
pub struct ShadowScene<'a> {
    /// Rigid meshes, indexed by handle.
    pub meshes: &'a [GpuMesh],
    /// Skinned meshes, indexed by handle.
    pub skinned_meshes: &'a [GpuMesh],
    /// Materials, indexed by handle.
    pub materials: &'a [Material],
    /// Each material's base colour map, indexed by handle. `None` is a material with no
    /// shader-visible map, which the alpha-tested path cannot draw.
    pub material_tables: &'a [Option<D3D12_GPU_DESCRIPTOR_HANDLE>],
    /// This frame's skinned draws, unfiltered — a character behind the camera still casts
    /// into a cascade.
    pub skinned: &'a SkinnedDrawList,
    /// Where this frame uploaded each of `skinned`'s palettes. A palette missing from it is
    /// uploaded by the pass that needs it, as every pass used to.
    pub palettes: &'a crate::palette::FramePalettes,
}

/// The highest debug view `shade_surface` knows.
///
/// ```text
/// 0  none          4  base colour map     8  shading normal, after the TBN transform
/// 1  cascade tint  5  normal map          9  roughness, after the ORM map scales it
/// 2  shadow factor 6  ORM map            10  metallic, likewise
/// 3  the atlas     7  emissive map       11  occlusion
/// ```
///
/// 4 to 7 are the raw texels a descriptor resolved to; 8 to 11 are what the shading made of
/// them, which is the half a texel view cannot answer — a normal map bound correctly and
/// applied with the wrong handedness looks right at 5 and wrong at 8.
///
/// The shader's `if` chain and this number are the same fact stated twice, so raising one
/// without the other silently disables the new view.
pub const MAX_DEBUG_MODE: u32 = 11;

/// Owns the shadow atlas, decides what goes in it, and records the passes that fill it.
pub struct ShadowRenderer {
    settings: ShadowSettings,
    resources: ShadowResources,
    pipelines: ShadowPipelines,
    /// The half-resolution contact-shadow target and the pass that fills it.
    ///
    /// Here rather than in the renderer because it belongs to the same stage: it reads the
    /// depth copy this module makes and it is bracketed by the same GPU timer.
    contact: ContactShadowTarget,
    contact_pipeline: ContactShadowPipeline,

    /// This frame's cascades and where each one lives in the atlas.
    cascades: Vec<CascadeView>,
    cascade_rects: Vec<AtlasRect>,
    /// One render list per cascade, kept across frames so the build stops allocating.
    cascade_lists: Vec<RenderList>,

    /// This frame's local shadow views, the lights that won them, and their rectangles.
    local_views: Vec<LocalShadowView>,
    local_selected: Vec<SelectedLight>,
    local_rects: Vec<AtlasRect>,
    local_lists: Vec<RenderList>,
    /// Scratch for the light sort, kept for the same reason.
    light_scratch: Vec<(usize, f32)>,

    collector: ShadowCasterCollector,
    instance_scratch: Vec<InstanceTransform>,
    palette_scratch: Vec<[f32; 16]>,

    stats: ShadowStats,
    /// 0 none, 1 cascade tint, 2 shadow factor only.
    debug_mode: u32,
    /// Milliseconds the last `prepare` took, folded into the stats by `record`.
    prepare_ms: f64,
}

impl ShadowRenderer {
    /// Creates the atlas, the pipelines and the per-frame buffers.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(
        device: &RenderDevice,
        descriptors: &mut GpuDescriptorHeap,
        settings: ShadowSettings,
        scene_size: (u32, u32),
    ) -> Result<Self, RendererError> {
        let settings = settings.validated();
        let resources = ShadowResources::new(device, descriptors, &settings, scene_size)?;
        let pipelines = ShadowPipelines::new(device.device(), settings.bias)?;
        let contact = ContactShadowTarget::new(
            device,
            descriptors,
            &settings.contact,
            settings.enabled,
            scene_size,
        )?;
        let contact_pipeline = ContactShadowPipeline::new(device.device())?;
        Ok(Self {
            stats: ShadowStats::for_settings(&settings),
            settings,
            resources,
            pipelines,
            contact,
            contact_pipeline,
            cascades: Vec::new(),
            cascade_rects: Vec::new(),
            cascade_lists: Vec::new(),
            local_views: Vec::new(),
            local_selected: Vec::new(),
            local_rects: Vec::new(),
            local_lists: Vec::new(),
            light_scratch: Vec::new(),
            collector: ShadowCasterCollector::new(),
            instance_scratch: Vec::new(),
            palette_scratch: Vec::new(),
            debug_mode: 0,
            prepare_ms: 0.0,
        })
    }

    /// What the system is currently configured with.
    #[must_use]
    pub fn settings(&self) -> &ShadowSettings {
        &self.settings
    }

    /// Replaces the settings, recreating whatever they invalidate.
    ///
    /// The atlas is recreated only when its size changes, and the pipeline states only when
    /// the bias does — D3D12 bakes depth bias into a pipeline state, so a bias slider is a
    /// pipeline rebuild and not a per-draw parameter.
    ///
    /// **The caller must have waited for the GPU.** Releasing a texture a frame in flight is
    /// still reading is a use-after-free inside the driver; this is the same requirement
    /// resizing the swap chain has, and for the same reason.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn set_settings(
        &mut self,
        device: &RenderDevice,
        settings: ShadowSettings,
        scene_size: (u32, u32),
    ) -> Result<(), RendererError> {
        let settings = settings.validated();
        self.resources.resize(device, &settings, scene_size)?;
        self.contact
            .resize(device, &settings.contact, settings.enabled, scene_size)?;
        self.pipelines.set_bias(device.device(), settings.bias)?;
        self.settings = settings;
        self.stats = ShadowStats::for_settings(&self.settings);
        Ok(())
    }

    /// Resizes the scene-depth copy to a new back-buffer size.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn resize(
        &mut self,
        device: &RenderDevice,
        scene_size: (u32, u32),
    ) -> Result<(), RendererError> {
        self.resources.resize(device, &self.settings, scene_size)?;
        self.contact.resize(
            device,
            &self.settings.contact,
            self.settings.enabled,
            scene_size,
        )?;
        Ok(())
    }

    /// Selects the shader's shadow debug view.
    ///
    /// 0 none, 1 cascade tint, 2 shadow factor, 3 the atlas drawn in the corner, 4 to 7 the
    /// material texture set raw — base colour, normal, ORM, emissive, in the order
    /// [`crate::material::MaterialTextureSlot`] freezes — and 8 to 11 what the shading made
    /// of them: the normal after the TBN transform, roughness, metallic, occlusion. See
    /// [`MAX_DEBUG_MODE`]. Clamped rather than rejected: the shader's `if` chain falls
    /// through to "no debug view" for anything it does not know, and clamping here keeps the
    /// two ends agreeing on how many views there are.
    pub fn set_debug_mode(&mut self, mode: u32) {
        self.debug_mode = mode.min(MAX_DEBUG_MODE);
    }

    /// The current debug view.
    #[must_use]
    pub fn debug_mode(&self) -> u32 {
        self.debug_mode
    }

    /// What the last frame's shadow stage did.
    #[must_use]
    pub fn stats(&self) -> ShadowStats {
        self.stats
    }

    /// This frame's cascades, for a debug view that wants to draw their bounds.
    #[must_use]
    pub fn cascades(&self) -> &[CascadeView] {
        &self.cascades
    }

    /// Whether the depth prepass has to run this frame.
    ///
    /// Only contact shadows need it — they march against the depth the prepass wrote — so a
    /// configuration without them pays nothing for it.
    #[must_use]
    pub fn needs_prepass(&self) -> bool {
        self.settings.enabled && self.settings.contact.enabled
    }

    /// Whether any shadow work happens at all.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.settings.enabled
    }

    /// The shadow atlas's shader-visible handle, for the lighting pass to bind.
    #[must_use]
    pub fn atlas_table(&self) -> Option<D3D12_GPU_DESCRIPTOR_HANDLE> {
        self.resources.atlas_srv().gpu
    }

    /// The scene-depth copy's shader-visible handle.
    #[must_use]
    pub fn contact_table(&self) -> Option<D3D12_GPU_DESCRIPTOR_HANDLE> {
        self.contact.table()
    }

    /// The contact-shadow target's size, or `None` when the pass did not run this frame.
    #[must_use]
    pub fn contact_target_size(&self) -> Option<(u32, u32)> {
        self.contact
            .is_live()
            .then(|| self.contact.size_in_texels())
    }

    /// The scene-depth copy's descriptor table.
    #[must_use]
    pub fn depth_table(&self) -> Option<D3D12_GPU_DESCRIPTOR_HANDLE> {
        self.resources.scene_depth_srv().gpu
    }

    /// Decides what this frame's shadow maps contain. Pure CPU.
    ///
    /// Builds the cascades, culls the casters into one render list per view, and picks which
    /// local lights can afford a shadow map. Nothing here touches D3D12, which is what lets
    /// a test assert on the result and what would let this move onto a worker thread.
    pub fn prepare(&mut self, world: &RenderWorld, camera: &Camera, lighting: Lighting) {
        let started = std::time::Instant::now();
        self.stats = ShadowStats::for_settings(&self.settings);
        self.stats.enabled = self.settings.enabled;
        self.cascades.clear();
        self.cascade_rects.clear();
        self.local_views.clear();
        self.local_selected.clear();
        self.local_rects.clear();

        if !self.settings.enabled {
            self.prepare_ms = started.elapsed().as_secs_f64() * 1e3;
            return;
        }

        build_cascades(
            &self.settings,
            camera,
            lighting.direction,
            &mut self.cascades,
        );

        // One list per cascade, allocated once and kept. `resize_with` only grows, so a
        // quality change that adds a cascade allocates once and never again.
        if self.cascade_lists.len() < self.cascades.len() {
            self.cascade_lists
                .resize_with(self.cascades.len(), RenderList::new);
        }

        let layout = *self.resources.layout();
        let mut lod_counts = [0_u32; 4];
        for index in 0..self.cascades.len() {
            let cascade = self.cascades[index];
            let mut list = std::mem::take(&mut self.cascade_lists[index]);
            self.collector
                .collect_cascade(world, &cascade, index, &self.settings, &mut list);
            // The same grouping the camera pass uses, which is what gives the shadow pass
            // instanced batching for free.
            list.group_by_material_and_mesh(usize::MAX, usize::MAX);
            let stats = self.collector.stats();
            self.stats.cascades[index].casters = stats.visible;
            for (level, count) in stats.lod_counts.iter().enumerate() {
                lod_counts[level] += count;
            }
            lod_counts[3] += stats.too_small;
            self.cascade_lists[index] = list;
            self.cascade_rects
                .push(layout.cascade_rect(index).unwrap_or(AtlasRect {
                    x: 0,
                    y: 0,
                    size: 1,
                }));
        }
        self.stats.lod_counts = lod_counts;

        // Local lights, if the budget allows any.
        let frustum = Frustum::from_view_projection(&camera.view_projection());
        select_shadow_lights(
            world.lights(),
            camera.position(),
            &frustum,
            &self.settings,
            &mut self.light_scratch,
            &mut self.local_selected,
            &mut self.local_views,
        );
        self.stats.local_views = self.local_views.len() as u32;
        self.stats.local_lights = self.local_selected.len() as u32;
        self.stats.local_lights_refused = world
            .lights()
            .iter()
            .filter(|light| light.casts_shadow && light.is_significant())
            .count()
            .saturating_sub(self.local_selected.len())
            as u32;

        if self.local_lists.len() < self.local_views.len() {
            self.local_lists
                .resize_with(self.local_views.len(), RenderList::new);
        }
        for index in 0..self.local_views.len() {
            let view = self.local_views[index];
            let mut list = std::mem::take(&mut self.local_lists[index]);
            self.collector.collect_local(
                world,
                &view,
                layout.local_resolution(),
                &self.settings,
                &mut list,
            );
            list.group_by_material_and_mesh(usize::MAX, usize::MAX);
            self.stats.local.casters += self.collector.stats().visible;
            self.local_lists[index] = list;
            self.local_rects
                .push(layout.local_rect(index).unwrap_or(AtlasRect {
                    x: 0,
                    y: 0,
                    size: 1,
                }));
        }

        self.prepare_ms = started.elapsed().as_secs_f64() * 1e3;
    }

    /// Writes everything the lighting pass needs to sample what `prepare` selected.
    pub fn write_frame_constants(
        &self,
        constants: &mut FrameConstants,
        lights: &[engine_render_data::LocalLight],
    ) {
        let atlas_size = self.resources.atlas_size();
        constants.set_atlas_size(atlas_size);
        constants.set_debug_mode(self.debug_mode);
        if !self.settings.enabled {
            // Explicitly: a disabled shadow system must leave the shader on the unshadowed
            // path rather than sampling a stale atlas.
            constants.set_directional_shadows(&[], &[], atlas_size, &self.settings);
            constants.set_local_lights(lights, &[], &[], &[], atlas_size);
            constants.set_contact_shadows(&super::settings::ContactShadowSettings {
                enabled: false,
                ..self.settings.contact
            });
            return;
        }
        constants.set_directional_shadows(
            &self.cascades,
            &self.cascade_rects,
            atlas_size,
            &self.settings,
        );
        constants.set_local_lights(
            lights,
            &self.local_selected,
            &self.local_views,
            &self.local_rects,
            atlas_size,
        );
        constants.set_contact_shadows(&self.settings.contact);
    }

    /// Records the depth passes that fill the atlas.
    ///
    /// One pass per cascade and one per local shadow view, each a viewport into the same
    /// texture. The atlas is cleared once for all of them: a clear of the whole texture is
    /// one command, and clearing per tile would be one command per view for the same work.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::InvalidConfig`] if a list names a mesh or material that does
    /// not exist, or [`RendererError::Api`] if recording fails.
    pub fn record(
        &mut self,
        commands: &mut CommandContext,
        uploads: &mut UploadRing,
        slot: usize,
        scene: &ShadowScene<'_>,
    ) -> Result<(), RendererError> {
        self.stats.cpu_ms = self.prepare_ms;
        self.stats.prepare_ms = self.prepare_ms;
        if !self.settings.enabled {
            return Ok(());
        }
        let started = std::time::Instant::now();

        commands.transition_to(self.resources.atlas(), D3D12_RESOURCE_STATE_DEPTH_WRITE);
        let dsv = self.resources.atlas_dsv().cpu;
        commands.set_depth_target(dsv);
        commands.clear_depth(dsv, SHADOW_CLEAR);
        commands.set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        // **Before any root argument.** A root constant-buffer view can only be set once a
        // root signature is bound, and the shadow stage is the first thing the frame's
        // command list records — so without this the very first bind is made against no
        // signature at all. D3D12 calls that invalid; the driver's answer to it was a device
        // removal with no other diagnostic.
        self.bind_first_pipeline(commands, true);

        for index in 0..self.cascades.len() {
            let Some(rect) = self.cascade_rects.get(index).copied() else {
                continue;
            };
            let view = ShadowViewConstants::new(self.cascades[index].view_projection, true);
            let frustum = self.cascades[index].frustum();
            let list = std::mem::take(&mut self.cascade_lists[index]);
            let pass =
                self.record_view(commands, uploads, slot, scene, rect, view, &list, &frustum);
            self.cascade_lists[index] = list;
            let pass = pass?;
            self.stats.cascades[index].draw_calls = pass.draw_calls;
            self.stats.cascades[index].triangles = pass.triangles;
        }

        for index in 0..self.local_views.len() {
            let Some(rect) = self.local_rects.get(index).copied() else {
                continue;
            };
            let view = ShadowViewConstants::new(self.local_views[index].view_projection, false);
            let frustum = self.local_views[index].frustum();
            let list = std::mem::take(&mut self.local_lists[index]);
            let pass =
                self.record_view(commands, uploads, slot, scene, rect, view, &list, &frustum);
            self.local_lists[index] = list;
            let pass = pass?;
            self.stats.local.draw_calls += pass.draw_calls;
            self.stats.local.triangles += pass.triangles;
        }

        // One transition for the whole atlas, which is the second reason it is one texture.
        commands.transition_to(
            self.resources.atlas(),
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        );

        self.stats.cpu_ms += started.elapsed().as_secs_f64() * 1e3;
        Ok(())
    }

    /// Records the camera's depth prepass into `depth`.
    ///
    /// The same depth-only pipelines the shadow passes use, unbiased, with the camera's own
    /// view-projection. It exists for the contact shadows — they march against the depth it
    /// writes — and it also front-loads the depth the camera pass then tests against, which
    /// is why the camera pass's depth comparison is less-**or-equal**.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] for the same reasons [`ShadowRenderer::record`] does.
    #[allow(
        clippy::too_many_arguments,
        reason = "a recorded pass names the command list, the ring, the slot, the scene, the                   visible set, the camera and the target it writes; bundling them into a                   struct would move the same fields behind a name that explains less"
    )]
    pub fn record_prepass(
        &mut self,
        commands: &mut CommandContext,
        uploads: &mut UploadRing,
        slot: usize,
        scene: &ShadowScene<'_>,
        list: &RenderList,
        camera: &Camera,
        depth_dsv: windows::Win32::Graphics::Direct3D12::D3D12_CPU_DESCRIPTOR_HANDLE,
        size: (u32, u32),
    ) -> Result<(), RendererError> {
        if !self.needs_prepass() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        commands.set_depth_target(depth_dsv);
        commands.set_viewport(size.0, size.1);
        commands.set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        // As in `record`: the root signature before the root arguments.
        self.bind_first_pipeline(commands, false);

        let view = ShadowViewConstants::new(camera.view_projection(), false);
        let allocation = uploads.write_constants(slot, &view)?;
        commands.set_root_cbv(ROOT_FRAME_CBV, allocation.gpu_address());

        let frustum = Frustum::from_view_projection(&camera.view_projection());
        let mut instances = std::mem::take(&mut self.instance_scratch);
        let mut palette = std::mem::take(&mut self.palette_scratch);
        let result = self
            .record_batches(
                commands,
                uploads,
                slot,
                scene,
                list,
                self.pipelines.prepass(),
                &mut instances,
            )
            .and_then(|()| {
                self.record_skinned(
                    commands,
                    uploads,
                    slot,
                    scene,
                    &frustum,
                    self.pipelines.prepass(),
                    &mut palette,
                    // A character the camera is looking through is not in the camera's
                    // depth: the world behind it is drawn first and seen through it, and a
                    // primed depth here would reject exactly that world.
                    true,
                )
            });
        self.instance_scratch = instances;
        self.palette_scratch = palette;
        result?;
        self.stats.cpu_ms += started.elapsed().as_secs_f64() * 1e3;
        Ok(())
    }

    /// Copies the camera's depth buffer into the texture the contact-shadow march samples.
    ///
    /// A copy rather than a read-only depth-stencil view: sampling the live depth buffer
    /// would mean a second DSV, a second set of camera pipeline states with depth writes
    /// disabled, and a rule that nothing in the camera pass may write depth. The copy is one
    /// command and one full-screen depth buffer of bandwidth, and it only happens when
    /// contact shadows are on. The trade is recorded in `docs/shadows.md`.
    pub fn copy_scene_depth(&mut self, commands: &mut CommandContext, depth: &GpuTexture) {
        if !self.needs_prepass() {
            return;
        }
        commands.transition_to(depth, D3D12_RESOURCE_STATE_COPY_SOURCE);
        commands.transition_to(self.resources.scene_depth(), D3D12_RESOURCE_STATE_COPY_DEST);
        commands.copy_resource(self.resources.scene_depth(), depth);
        commands.transition_to(
            self.resources.scene_depth(),
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        );
        commands.transition_to(depth, D3D12_RESOURCE_STATE_DEPTH_WRITE);
    }

    /// Records the contact-shadow pass into its own half-resolution target.
    ///
    /// Runs immediately after [`ShadowRenderer::copy_scene_depth`], which is the depth it
    /// marches through. The caller must already have bound the frame constants and the
    /// scene-depth table — the pass reads both — and must restore its own render target
    /// afterwards, which the shadow stage already does for the atlas.
    pub fn record_contact(&mut self, commands: &mut CommandContext) {
        self.contact.record(commands, &self.contact_pipeline);
    }

    /// Binds one of the depth-only pipelines, so a root signature is in place.
    ///
    /// Which one hardly matters — every depth-only state shares one root signature with the
    /// mesh pipelines — but *some* pipeline has to be bound before a root argument is, and
    /// the batches that follow will rebind whichever they actually need. A redundant bind is
    /// counted and skipped by the command context, so this costs one comparison per pass.
    fn bind_first_pipeline(&self, commands: &mut CommandContext, biased: bool) {
        let set = if biased {
            self.pipelines.shadow()
        } else {
            self.pipelines.prepass()
        };
        commands.set_pipeline(
            self.pipelines.root_signature(),
            set.get(ShadowGeometry::Instanced, ShadowSurface::Opaque),
        );
    }

    /// Records one shadow view: a viewport, its constants, its casters.
    #[allow(
        clippy::too_many_arguments,
        reason = "one call site, and every argument is a
             different resource the pass needs; bundling them would be a struct that exists
             for one function"
    )]
    fn record_view(
        &mut self,
        commands: &mut CommandContext,
        uploads: &mut UploadRing,
        slot: usize,
        scene: &ShadowScene<'_>,
        rect: AtlasRect,
        view: ShadowViewConstants,
        list: &RenderList,
        frustum: &Frustum,
    ) -> Result<ShadowPassStats, RendererError> {
        let before = commands.stats();
        commands.set_viewport_rect(rect.x, rect.y, rect.size, rect.size);

        let allocation = uploads.write_constants(slot, &view)?;
        commands.set_root_cbv(ROOT_FRAME_CBV, allocation.gpu_address());

        // Taken out of `self` so the two recording calls can hold the pipeline set — which
        // is also a borrow of `self` — and put back afterwards so the allocations survive to
        // the next frame. The same trick `Simulation::apply_pending_commands` uses.
        let mut instances = std::mem::take(&mut self.instance_scratch);
        let mut palette = std::mem::take(&mut self.palette_scratch);
        let result = self
            .record_batches(
                commands,
                uploads,
                slot,
                scene,
                list,
                self.pipelines.shadow(),
                &mut instances,
            )
            .and_then(|()| {
                self.record_skinned(
                    commands,
                    uploads,
                    slot,
                    scene,
                    frustum,
                    self.pipelines.shadow(),
                    &mut palette,
                    // Its shadow, on the other hand, is whole.
                    false,
                )
            });
        self.instance_scratch = instances;
        self.palette_scratch = palette;
        self.stats.skinned_draws += result?;

        let after = commands.stats();
        Ok(ShadowPassStats {
            casters: 0,
            draw_calls: after.draw_calls - before.draw_calls,
            triangles: after.triangles - before.triangles,
        })
    }

    /// Records a render list's batches as instanced depth-only draws.
    ///
    /// `scratch` is lent by the caller rather than taken from `self`: the pipeline set is a
    /// borrow of `self` that has to live across the loop, and staging instances into a field
    /// of the same `self` would be a second, mutable borrow of it.
    #[allow(
        clippy::too_many_arguments,
        reason = "the same list of things one recorded pass needs, plus the scratch buffer                   the caller lends so this method can take &self"
    )]
    fn record_batches(
        &self,
        commands: &mut CommandContext,
        uploads: &mut UploadRing,
        slot: usize,
        scene: &ShadowScene<'_>,
        list: &RenderList,
        pipelines: &ShadowPipelineSet,
        scratch: &mut Vec<InstanceTransform>,
    ) -> Result<(), RendererError> {
        for batch in list.batches() {
            let Some(mesh) = scene.meshes.get(batch.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a shadow list names mesh {} which does not exist",
                        batch.mesh.index()
                    ),
                });
            };
            let Some(material) = scene.materials.get(batch.material.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a shadow list names material {} which does not exist",
                        batch.material.index()
                    ),
                });
            };

            let alpha_tested = material.desc().is_alpha_tested();
            let surface = if alpha_tested {
                ShadowSurface::AlphaTested
            } else {
                ShadowSurface::Opaque
            };
            commands.set_pipeline(
                self.pipelines.root_signature(),
                pipelines.get(ShadowGeometry::Instanced, surface),
            );

            // Only the cutout path reads the material at all: the opaque pipeline has no
            // pixel shader, so binding its constants and its texture would be two root
            // arguments written for a shader that does not exist.
            if alpha_tested {
                let Some(Some(table)) = scene.material_tables.get(batch.material.index()).copied()
                else {
                    return Err(RendererError::InvalidConfig {
                        reason: format!(
                            "alpha-tested material {} has no shader-visible base colour map",
                            batch.material.index()
                        ),
                    });
                };
                commands.set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
                commands.set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            }

            let count = batch.count as usize;
            let bytes = count * size_of::<InstanceTransform>();
            let allocation = uploads.allocate(slot, bytes, INSTANCE_ALIGNMENT)?;

            let first = batch.first as usize;
            scratch.clear();
            scratch.reserve(count);
            for &entry in &list.order()[first..first + count] {
                scratch.push(InstanceTransform::styled(
                    list.transforms()[entry as usize].matrix(),
                    list.styles()[entry as usize],
                ));
            }
            allocation.write(scratch)?;

            commands.set_vertex_buffer(0, *mesh.vertex_view());
            commands.set_vertex_buffer(
                1,
                windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                    BufferLocation: allocation.gpu_address(),
                    SizeInBytes: bytes as u32,
                    StrideInBytes: size_of::<InstanceTransform>() as u32,
                },
            );
            commands.set_index_buffer(*mesh.index_view());
            commands.draw_indexed(mesh.index_count(), batch.count, 0);
        }
        Ok(())
    }

    /// Records this frame's skinned casters into the current view.
    ///
    /// Culled here rather than in `prepare` because the skinned list is rebuilt every frame
    /// and is small: a sphere test per submesh per view is cheaper than gathering a second
    /// copy of it per view would be.
    #[allow(
        clippy::too_many_arguments,
        reason = "as record_batches: the pass's inputs, plus the caller's scratch buffer"
    )]
    fn record_skinned(
        &self,
        commands: &mut CommandContext,
        uploads: &mut UploadRing,
        slot: usize,
        scene: &ShadowScene<'_>,
        frustum: &Frustum,
        pipelines: &ShadowPipelineSet,
        scratch: &mut Vec<[f32; 16]>,
        skip_translucent: bool,
    ) -> Result<u32, RendererError> {
        if scene.skinned.is_empty() {
            return Ok(0);
        }
        let mut drawn = 0;
        let mut bound_palette: Option<(u32, u32)> = None;

        for index in 0..scene.skinned.len() {
            let Some(instance) = scene.skinned.instances().get(index).copied() else {
                break;
            };
            if !skinned_caster_visible(&instance, frustum) {
                continue;
            }
            if skip_translucent && instance.style.is_faded() {
                continue;
            }
            let Some(mesh) = scene.skinned_meshes.get(instance.mesh.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a skinned shadow caster names skinned mesh {} which does not exist",
                        instance.mesh.index()
                    ),
                });
            };
            let Some(material) = scene.materials.get(instance.material.index()) else {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a skinned shadow caster names material {} which does not exist",
                        instance.material.index()
                    ),
                });
            };

            let alpha_tested = material.desc().is_alpha_tested();
            let surface = if alpha_tested {
                ShadowSurface::AlphaTested
            } else {
                ShadowSurface::Opaque
            };
            commands.set_pipeline(
                self.pipelines.root_signature(),
                pipelines.get(ShadowGeometry::Skinned, surface),
            );
            if alpha_tested
                && let Some(Some(table)) = scene
                    .material_tables
                    .get(instance.material.index())
                    .copied()
            {
                commands.set_root_cbv(ROOT_MATERIAL_CBV, material.gpu_address());
                commands.set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            }

            let range = (instance.palette_start, instance.palette_len);
            if bound_palette != Some(range) {
                // Uploaded once for the whole frame, before any pass was recorded; this view
                // only binds it. The upload below is the fallback for a palette the frame did
                // not know about, which is what every view used to do for every character.
                let address = match scene.palettes.address(range.0, range.1) {
                    Some(address) => address,
                    None => {
                        let Some(palette) = scene.skinned.palette_of(index) else {
                            return Err(RendererError::InvalidConfig {
                                reason: format!(
                                    "a skinned shadow caster names bones {}..{} of a {}-matrix palette",
                                    instance.palette_start,
                                    instance.palette_start + instance.palette_len,
                                    scene.skinned.palette().len()
                                ),
                            });
                        };
                        crate::palette::upload_padded(palette, scratch, uploads, slot)?
                    }
                };
                commands.set_root_cbv(ROOT_SKIN_CBV, address);
                bound_palette = Some(range);
            }

            let object = ObjectConstants::styled(instance.transform.matrix(), instance.style);
            let allocation = uploads.write_constants(slot, &object)?;
            commands.set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
            commands.set_vertex_buffer(0, *mesh.vertex_view());
            commands.set_index_buffer(*mesh.index_view());
            commands.draw_indexed(mesh.index_count(), 1, 0);
            drawn += 1;
        }
        Ok(drawn)
    }

    /// Records the GPU time the shadow passes took, once the timer has resolved it.
    pub fn set_gpu_ms(&mut self, milliseconds: Option<f64>) {
        self.stats.gpu_ms = milliseconds;
    }
}

/// The direction a light travels, normalized, with a fallback for a degenerate one.
///
/// A light with no direction at all is authored by accident — a script that zeroed a vector
/// — and it would produce a `NaN` view matrix and a black frame. Straight down is the
/// answer that keeps the frame renderable and looks obviously wrong.
#[must_use]
pub fn safe_light_direction(direction: Vec3) -> Vec3 {
    if direction.length_squared() > 1e-6 {
        direction.normalize()
    } else {
        Vec3::new(0.0, -1.0, 0.0)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::settings::ShadowPreset;

    #[test]
    fn a_shadow_views_constants_carry_the_matrix_untransposed() {
        let matrix = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let constants = ShadowViewConstants::new(matrix, true);
        assert_eq!(&constants.view_projection[12..15], &[1.0, 2.0, 3.0]);
        assert_eq!(constants.params[0], 1.0);
        // Small on purpose: this is uploaded once per cascade and once per local view.
        assert_eq!(size_of::<ShadowViewConstants>(), 80);
    }

    #[test]
    fn a_degenerate_light_direction_does_not_produce_a_nan_matrix() {
        assert_eq!(safe_light_direction(Vec3::ZERO), Vec3::new(0.0, -1.0, 0.0));
        assert!(
            (safe_light_direction(Vec3::new(0.0, -2.0, 0.0)).y + 1.0).abs() < 1e-6,
            "an ordinary direction must be normalized, not replaced"
        );
    }

    #[test]
    fn the_prepass_only_runs_for_contact_shadows() {
        // It is the one thing that needs it, and it costs a second pass over every visible
        // object — so a configuration without contact shadows must not pay for one.
        assert!(ShadowPreset::High.settings().contact.enabled);
        assert!(!ShadowPreset::Medium.settings().contact.enabled);
    }
}
