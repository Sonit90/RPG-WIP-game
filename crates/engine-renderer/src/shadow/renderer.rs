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

use engine_core::math::Frustum;
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
use super::cascade::CascadeView;
use super::caster::skinned_caster_visible;
use super::contact::{ContactShadowPipeline, ContactShadowTarget};
use super::pipeline::{ShadowGeometry, ShadowPipelineSet, ShadowPipelines, ShadowSurface};
// Moved to `plan.rs` with the rest of the backend-neutral half; re-exported so the old
// paths keep working.
pub use super::plan::{MAX_DEBUG_MODE, ShadowPlan, ShadowViewConstants, safe_light_direction};
use super::resources::{SHADOW_CLEAR, ShadowResources};
use super::settings::ShadowSettings;
use super::static_cache::{StaticShadowCache, StaticView};
use super::stats::{ShadowPassStats, ShadowStats};

/// Where a batch loop's per-instance data comes from.
pub(crate) enum InstanceSource<'a> {
    /// Composed from the list's own transforms into this frame's ring, with this scratch.
    Compose(&'a mut Vec<InstanceTransform>),
    /// Composed already, in the list's batch order, and only copied into this frame's ring:
    /// a static view's casters, kept from the frame they were gathered in.
    Cached(&'a [InstanceTransform]),
    /// Already in the ring, in the list's batch order: what the camera pass uploaded for the
    /// same list, which the depth prepass binds instead of composing it a second time.
    Uploaded(windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW),
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

/// Owns the shadow atlas, decides what goes in it, and records the passes that fill it.
pub struct ShadowRenderer {
    resources: ShadowResources,
    pipelines: ShadowPipelines,
    /// The half-resolution contact-shadow target and the pass that fills it.
    ///
    /// Here rather than in the renderer because it belongs to the same stage: it reads the
    /// depth copy this module makes and it is bracketed by the same GPU timer.
    contact: ContactShadowTarget,
    contact_pipeline: ContactShadowPipeline,
    /// Everything the stage decides on the CPU: settings, cascades, caster lists, light
    /// selection, statistics. The passes below record from its lists.
    plan: ShadowPlan,

    instance_scratch: Vec<InstanceTransform>,
    palette_scratch: Vec<[f32; 16]>,
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
            resources,
            pipelines,
            contact,
            contact_pipeline,
            plan: ShadowPlan::new(settings),
            instance_scratch: Vec::new(),
            palette_scratch: Vec::new(),
        })
    }

    /// What the system is currently configured with.
    #[must_use]
    pub fn settings(&self) -> &ShadowSettings {
        self.plan.settings()
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
        self.plan.apply_settings(settings);
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
        self.resources
            .resize(device, self.plan.settings(), scene_size)?;
        self.contact.resize(
            device,
            &self.plan.settings().contact,
            self.plan.settings().enabled,
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
        self.plan.set_debug_mode(mode);
    }

    /// The current debug view.
    #[must_use]
    pub fn debug_mode(&self) -> u32 {
        self.plan.debug_mode()
    }

    /// What the last frame's shadow stage did.
    #[must_use]
    pub fn stats(&self) -> ShadowStats {
        self.plan.stats()
    }

    /// This frame's cascades, for a debug view that wants to draw their bounds.
    #[must_use]
    pub fn cascades(&self) -> &[CascadeView] {
        self.plan.cascades()
    }

    /// Whether the depth prepass has to run this frame.
    ///
    /// Only contact shadows need it — they march against the depth the prepass wrote — so a
    /// configuration without them pays nothing for it.
    #[must_use]
    pub fn needs_prepass(&self) -> bool {
        self.plan.needs_prepass()
    }

    /// Whether the prepass's depth is copied for, and marched by, the contact shadows.
    ///
    /// Only when contact shadows are on. A prepass that runs for depth priming alone writes
    /// the camera's depth buffer and stops there.
    #[must_use]
    pub fn needs_depth_copy(&self) -> bool {
        self.plan.needs_depth_copy()
    }

    /// Runs the camera prepass even without contact shadows, so the camera pass can test
    /// against it instead of writing its own depth. See `DepthMode::Primed`.
    pub fn set_priming_prepass(&mut self, enabled: bool) {
        self.plan.set_priming_prepass(enabled);
    }

    /// Whether the last cascade draws movers and characters (see the field).
    pub fn set_far_movers(&mut self, enabled: bool) {
        self.plan.set_far_movers(enabled);
    }

    /// Turns the static-caster cache on or off, and sets how far a cached cascade may drift
    /// before it is gathered again, as a fraction of its radius. Off gathers every caster
    /// into every view every frame: the A/B control.
    pub fn set_static_cache(&mut self, enabled: bool, margin_fraction: f32) {
        self.plan.set_static_cache(enabled, margin_fraction);
    }

    /// Whether any shadow work happens at all.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.plan.is_enabled()
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
    ///
    /// `material_count` and `mesh_count` are the lengths of the renderer's handle arrays: the
    /// key space the lists are grouped over, exactly as the camera's list is. Passing anything
    /// larger than the real arrays makes the grouping give up and keep gather order, which is
    /// what every shadow view did before they were passed: a draw per caster.
    pub fn prepare(
        &mut self,
        world: &RenderWorld,
        camera: &Camera,
        lighting: Lighting,
        material_count: usize,
        mesh_count: usize,
    ) {
        self.plan.prepare(
            world,
            camera,
            lighting,
            material_count,
            mesh_count,
            self.resources.layout(),
        );
    }

    /// Writes everything the lighting pass needs to sample what `prepare` selected.
    pub fn write_frame_constants(
        &self,
        constants: &mut FrameConstants,
        lights: &[engine_render_data::LocalLight],
    ) {
        self.plan
            .write_frame_constants(constants, lights, self.resources.atlas_size());
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
        self.plan.stats.cpu_ms = self.plan.prepare_ms;
        self.plan.stats.prepare_ms = self.plan.prepare_ms;
        if !self.plan.settings.enabled {
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

        // Out of `self` for the loops, so a view can lend its static casters to `record_view`
        // while that borrows `self` mutably. Put back below, on every path.
        let cache = std::mem::take(&mut self.plan.static_cache);
        let result = self.record_views(commands, uploads, slot, scene, &cache);
        self.plan.static_cache = cache;
        result?;

        // One transition for the whole atlas, which is the second reason it is one texture.
        commands.transition_to(
            self.resources.atlas(),
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        );

        self.plan.stats.atlas_ms = started.elapsed().as_secs_f64() * 1e3;
        self.plan.stats.cpu_ms += self.plan.stats.atlas_ms;
        Ok(())
    }

    /// Records every cascade and every local view, each with the static casters `cache`
    /// holds for it.
    fn record_views(
        &mut self,
        commands: &mut CommandContext,
        uploads: &mut UploadRing,
        slot: usize,
        scene: &ShadowScene<'_>,
        cache: &StaticShadowCache,
    ) -> Result<(), RendererError> {
        for index in 0..self.plan.cascades.len() {
            let Some(rect) = self.plan.cascade_rects.get(index).copied() else {
                continue;
            };
            let view = ShadowViewConstants::new(self.plan.cascades[index].view_projection, true);
            let frustum = self.plan.cascades[index].frustum();
            let statics = cache.cascade_view(index);
            let characters = self.plan.far_movers || index + 1 < self.plan.cascades.len();
            let list = std::mem::take(&mut self.plan.cascade_lists[index]);
            let pass = self.record_view(
                commands, uploads, slot, scene, rect, view, statics, &list, &frustum, characters,
            );
            self.plan.cascade_lists[index] = list;
            let pass = pass?;
            self.plan.stats.cascades[index].draw_calls = pass.draw_calls;
            self.plan.stats.cascades[index].triangles = pass.triangles;
        }

        for index in 0..self.plan.local_views.len() {
            let Some(rect) = self.plan.local_rects.get(index).copied() else {
                continue;
            };
            let local = self.plan.local_views[index];
            let view = ShadowViewConstants::new(local.view_projection, false);
            let frustum = local.frustum();
            let statics = cache
                .enabled()
                .then(|| cache.local_view(local.light, local.face.unwrap_or(0)))
                .flatten();
            let list = std::mem::take(&mut self.plan.local_lists[index]);
            let pass = self.record_view(
                commands, uploads, slot, scene, rect, view, statics, &list, &frustum, true,
            );
            self.plan.local_lists[index] = list;
            let pass = pass?;
            self.plan.stats.local.draw_calls += pass.draw_calls;
            self.plan.stats.local.triangles += pass.triangles;
        }
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
        camera_instances: Option<windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW>,
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
                match camera_instances {
                    Some(view) => InstanceSource::Uploaded(view),
                    None => InstanceSource::Compose(&mut instances),
                },
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
        self.plan.stats.prepass_ms = started.elapsed().as_secs_f64() * 1e3;
        self.plan.stats.cpu_ms += self.plan.stats.prepass_ms;
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
        if !self.needs_depth_copy() {
            return;
        }
        if self.resources.samples_camera_depth() {
            // No copy: the march reads the depth buffer itself, which only has to be readable
            // until `finish_scene_depth` hands it back to the camera pass.
            commands.transition_to(depth, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
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

    /// Returns the camera's depth buffer to `DEPTH_WRITE` after the contact-shadow march, when
    /// the march sampled it in place. A no-op for the copy path, which never moved it.
    pub fn finish_scene_depth(&mut self, commands: &mut CommandContext, depth: &GpuTexture) {
        if self.needs_depth_copy() && self.resources.samples_camera_depth() {
            commands.transition_to(depth, D3D12_RESOURCE_STATE_DEPTH_WRITE);
        }
    }

    /// Samples the camera's depth buffer in place for the contact-shadow march instead of a
    /// copy of it. See [`ShadowResources::sample_camera_depth`]; called again whenever the
    /// depth buffer or the shadow resources are recreated.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn sample_camera_depth(
        &mut self,
        device: &RenderDevice,
        depth: &GpuTexture,
    ) -> Result<(), RendererError> {
        self.resources.sample_camera_depth(device, depth)
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
        statics: Option<&StaticView>,
        list: &RenderList,
        frustum: &Frustum,
        characters: bool,
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
        // The static casters first, from the cache, then the movers and the characters.
        let statics = statics.map(|view| (&view.list, view.instances.as_slice()));
        let result = statics
            .map_or(Ok(()), |(statics, cached)| {
                self.record_batches(
                    commands,
                    uploads,
                    slot,
                    scene,
                    statics,
                    self.pipelines.shadow(),
                    InstanceSource::Cached(cached),
                )
            })
            .and_then(|()| {
                self.record_batches(
                    commands,
                    uploads,
                    slot,
                    scene,
                    list,
                    self.pipelines.shadow(),
                    InstanceSource::Compose(&mut instances),
                )
            })
            .and_then(|()| {
                if !characters {
                    return Ok(0);
                }
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
        self.plan.stats.skinned_draws += result?;

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
        instances: InstanceSource<'_>,
    ) -> Result<(), RendererError> {
        // One instance stream for the whole list, in batch order, bound once: each batch is
        // then a draw starting at its own `first`.
        let stream = match instances {
            InstanceSource::Uploaded(view) => Some(view),
            InstanceSource::Compose(scratch) => list.upload_instances(uploads, slot, scratch)?,
            InstanceSource::Cached([]) => None,
            InstanceSource::Cached(instances) => {
                let allocation = uploads.write(
                    slot,
                    instances,
                    crate::render_list::INSTANCE_STREAM_ALIGNMENT,
                )?;
                Some(
                    windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                        BufferLocation: allocation.gpu_address(),
                        SizeInBytes: std::mem::size_of_val(instances) as u32,
                        StrideInBytes: size_of::<InstanceTransform>() as u32,
                    },
                )
            }
        };
        let Some(stream) = stream else {
            return Ok(());
        };
        commands.set_vertex_buffer(1, stream);
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

            commands.set_vertex_buffer(0, *mesh.vertex_view());
            commands.set_index_buffer(*mesh.index_view());
            commands.draw_indexed(mesh.index_count(), batch.count, batch.first);
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
        camera_depth: bool,
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
            if camera_depth {
                // The camera's depth, not a light's: every whole character the camera draws
                // is in it, caster or not — the camera pass draws them against this depth
                // without writing their own — and a faded one is not, because the camera
                // draws it see-through over the world rather than into it.
                if instance.style.is_faded() || !frustum.intersects_sphere(&instance.bounds) {
                    continue;
                }
            } else if !skinned_caster_visible(&instance, frustum) {
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
        self.plan.set_gpu_ms(milliseconds);
    }
}
