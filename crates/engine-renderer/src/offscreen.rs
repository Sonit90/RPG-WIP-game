//! Offscreen rendering with CPU readback.
//!
//! Renders into a texture the swap chain never sees, copies it into a readback buffer and
//! returns the pixels. This is how the engine proves it actually draws something: a
//! recorded draw call and a silent validation layer are not evidence — a culled triangle
//! produces both. It needs no window, so it also runs where no desktop is available.
//!
//! Milestone 9 builds its automated GPU test on this; Milestone 4 uses it to verify the
//! triangle is visible.

use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CLEAR_FLAG_DEPTH, D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_CPU_DESCRIPTOR_HANDLE,
    D3D12_DESCRIPTOR_HEAP_DESC, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_READBACK,
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RANGE, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET,
    D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
    D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ID3D12CommandAllocator, ID3D12DescriptorHeap, ID3D12Resource,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::core::Interface;

use engine_core::transform::Transform;

use crate::command::{CommandContext, scissor_for, transition_barrier, viewport_for};
use crate::depth::{DEPTH_CLEAR, DepthBuffer};
use crate::device::RenderDevice;
use crate::error::RendererError;
use crate::material::{Material, MaterialDesc};
use crate::mesh::{GpuMesh, MeshData, SkinnedMeshData, VertexStream};
use crate::mesh_pipeline::{
    InstanceTransform, InstancedMeshPipeline, MeshPipeline, SkinnedMeshPipeline,
};
use crate::pipeline::{ScenePipeline, TRIANGLE_VERTICES, Vertex};
use crate::scene::{
    FrameConstants, ObjectConstants, ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_MATERIAL_TABLE,
    ROOT_OBJECT_CBV, ROOT_SKIN_CBV,
};
use crate::swapchain::BACK_BUFFER_FORMAT;
use crate::upload::UploadRing;
use engine_render_data::Camera;
use engine_render_data::{Lighting, MAX_PALETTE_BONES};

/// Bytes per pixel of [`BACK_BUFFER_FORMAT`].
pub const BYTES_PER_PIXEL: usize = 4;

/// The 1×1 white base colour map both scene paths bind, and the heap holding its view.
///
/// Every mesh material samples a base colour map, and an unbound descriptor table is not a
/// validation error — it is a sample from whatever the heap happens to contain, which is why
/// forgetting it here showed up as the two paths disagreeing about a thousand pixels rather
/// than as a failure.
///
/// The heap and texture are returned so the caller can keep them alive until the submission
/// completes; the handle is what gets bound.
struct WhiteMap {
    heap: crate::descriptor::GpuDescriptorHeap,
    /// Held, not read: the descriptor points into it, so dropping it would leave a
    /// shader-visible SRV addressing released memory.
    #[allow(dead_code, reason = "keeps the texture the descriptor points at alive")]
    texture: crate::resource::GpuTexture,
    table: windows::Win32::Graphics::Direct3D12::D3D12_GPU_DESCRIPTOR_HANDLE,
}

/// Prints whatever the validation layer stored, if it is running.
///
/// Silent when the layer is off, which is the usual case: `GetNumStoredMessages` on a device
/// with no info queue returns nothing rather than failing.
fn drain_validation_messages(context: &RenderDevice) {
    use windows::Win32::Graphics::Direct3D12::ID3D12InfoQueue;
    use windows::core::Interface;

    let Ok(queue) = context.device().cast::<ID3D12InfoQueue>() else {
        return;
    };
    // SAFETY: FFI calls on a live info queue. The message buffer is sized by the same call
    // that fills it, which is the documented two-call pattern.
    unsafe {
        let count = queue.GetNumStoredMessages();
        for index in 0..count {
            let mut length = 0_usize;
            if queue.GetMessage(index, None, &mut length).is_err() || length == 0 {
                continue;
            }
            let mut bytes = vec![0_u8; length];
            let message = bytes.as_mut_ptr().cast();
            if queue.GetMessage(index, Some(message), &mut length).is_err() {
                continue;
            }
            let message = &*(message as *const windows::Win32::Graphics::Direct3D12::D3D12_MESSAGE);
            let text = std::slice::from_raw_parts(
                message.pDescription.cast::<u8>(),
                message.DescriptionByteLength.saturating_sub(1),
            );
            eprintln!("d3d12: {}", String::from_utf8_lossy(text));
        }
    }
}

/// Uploads the shared 1x1 white texture.
///
/// Split from [`white_map`] so a path that owns its own descriptor heap — the shadowed one,
/// which has to share a heap with the shadow atlas — can put the view somewhere else.
fn white_texture(context: &RenderDevice) -> Result<crate::resource::GpuTexture, RendererError> {
    use crate::resource::GpuTexture;
    use crate::upload::StagingUploader;
    use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

    let texture = GpuTexture::new(
        context,
        1,
        1,
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        D3D12_RESOURCE_STATE_COPY_DEST,
    )?;
    let mut uploader = StagingUploader::new(context)?;
    uploader.queue_texture(
        context,
        &texture,
        &[255, 255, 255, 255],
        D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    )?;
    uploader.flush(context)?;
    Ok(texture)
}

/// Writes the white texture's shader-resource view into `handle`.
fn create_white_view(
    context: &RenderDevice,
    texture: &crate::resource::GpuTexture,
    handle: crate::descriptor::DescriptorHandle,
) {
    use crate::resource::TrackedResource;
    use windows::Win32::Graphics::Direct3D12::{
        D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
        D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM_SRGB;

    let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: 0x1688,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                PlaneSlice: 0,
                ResourceMinLODClamp: 0.0,
            },
        },
    };
    // SAFETY: FFI call. The texture outlives the submission, and the CPU handle addresses a
    // slot the caller just allocated.
    unsafe {
        context.device().CreateShaderResourceView(
            Some(TrackedResource::resource(texture)),
            Some(&desc),
            handle.cpu,
        );
    }
}

/// Creates the shared white map for one offscreen submission.
fn white_map(context: &RenderDevice) -> Result<WhiteMap, RendererError> {
    use crate::descriptor::GpuDescriptorHeap;

    let texture = white_texture(context)?;
    let mut heap = GpuDescriptorHeap::new(context, 1, 0, 1)?;
    let descriptor = heap.allocate_persistent()?;
    create_white_view(context, &texture, descriptor);

    let table = descriptor.gpu.ok_or_else(|| RendererError::InvalidConfig {
        reason: "the offscreen white map descriptor is not shader visible".to_owned(),
    })?;
    Ok(WhiteMap {
        heap,
        texture,
        table,
    })
}

/// A rendered image read back to system memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Tightly packed RGBA8 rows — the readback buffer's row padding is removed.
    pub pixels: Vec<u8>,
}

impl CapturedImage {
    /// Returns the RGBA value at `(x, y)`, or `None` if it is outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y as usize * self.width as usize + x as usize) * BYTES_PER_PIXEL;
        self.pixels
            .get(offset..offset + BYTES_PER_PIXEL)
            .map(|slice| [slice[0], slice[1], slice[2], slice[3]])
    }

    /// Counts pixels for which `predicate` holds.
    pub fn count_pixels(&self, predicate: impl Fn([u8; 4]) -> bool) -> usize {
        self.pixels
            .chunks_exact(BYTES_PER_PIXEL)
            .filter(|chunk| predicate([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .count()
    }
}

/// Renders [`TRIANGLE_VERTICES`] offscreen and reads the result back.
///
/// # Errors
///
/// Returns [`RendererError`] if the device, any resource, the pipeline or the readback
/// fails.
pub fn render_triangle(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
) -> Result<CapturedImage, RendererError> {
    render_vertices(width, height, clear_color, &TRIANGLE_VERTICES)
}

/// Renders arbitrary vertices offscreen and reads the result back.
///
/// Creates its own device, so it is independent of any swap chain or window.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
pub fn render_vertices(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    vertices: &[Vertex],
) -> Result<CapturedImage, RendererError> {
    render_with(width, height, clear_color, |context, commands, _, _| {
        let list = commands.list();
        let pipeline = ScenePipeline::new(context.device())?;
        let mesh = VertexStream::upload(context, vertices)?;

        // SAFETY: FFI calls recording into the open list. The pipeline and the mesh are
        // returned below and kept alive by the caller until the submission completes.
        unsafe {
            list.SetGraphicsRootSignature(pipeline.root_signature());
            list.SetPipelineState(pipeline.pipeline_state());
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.IASetVertexBuffers(0, Some(&[*mesh.view()]));
            list.DrawInstanced(mesh.vertex_count(), 1, 0, 0);
        }

        Ok(Box::new((pipeline, mesh)) as Box<dyn std::any::Any>)
    })
}

/// One object in an offscreen scene.
pub struct SceneObject<'a> {
    /// Its geometry.
    pub mesh: &'a MeshData,
    /// Its surface.
    pub material: MaterialDesc,
    /// Where it is in the world.
    pub transform: Transform,
}

/// One skinned object in an offscreen scene.
pub struct SkinnedObject<'a> {
    /// Its geometry, in bind pose.
    pub mesh: &'a SkinnedMeshData,
    /// Its surface.
    pub material: MaterialDesc,
    /// Where the character is in the world.
    pub transform: Transform,
    /// The bone palette its vertices index. Shorter than the shader's array is fine — the
    /// remainder is filled with the identity.
    pub palette: &'a [engine_core::math::Mat4],
}

/// Renders skinned geometry offscreen and reads the result back.
///
/// The offscreen equivalent of the renderer's skinned pass: same pipeline state, same root
/// signature, same padded palette in a constant buffer. It exists so "the character is
/// actually drawn, and posing it actually moves it" can be *asserted* rather than looked at
/// — which is the only kind of visual verification that survives into a regression suite.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails, a material is invalid, or a palette is
/// longer than the shader's array.
pub fn render_skinned_scene(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    objects: &[SkinnedObject<'_>],
    camera: &Camera,
    lighting: Lighting,
) -> Result<CapturedImage, RendererError> {
    render_with(width, height, clear_color, |context, commands, _, _| {
        let list = commands.list();
        let pipeline = SkinnedMeshPipeline::new(context.device())?;
        // Room for one palette and one set of constants per object.
        let mut ring = UploadRing::new(context, 1 << 20, 1)?;
        ring.begin_frame(0);

        let mut meshes = Vec::with_capacity(objects.len());
        let mut materials = Vec::with_capacity(objects.len());
        for object in objects {
            meshes.push(GpuMesh::upload_skinned(context, object.mesh)?);
            materials.push(Material::new(context, object.material)?);
        }

        let white = white_map(context)?;
        let frame = FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        let frame_allocation = ring.write_constants(0, &frame)?;
        // An empty light grid. These paths have no local lights, so the shader never reads
        // it — but a root SRV the signature declares is bound on every draw that could,
        // rather than left for the validation layer to find.
        let cluster_allocation = empty_light_grid(&mut ring)?;

        // SAFETY: FFI calls recording into the open list. Every referenced object is
        // returned below and kept alive until the submission completes.
        unsafe {
            list.SetGraphicsRootSignature(pipeline.root_signature());
            list.SetPipelineState(pipeline.pipeline_state());
            list.SetDescriptorHeaps(&[Some(white.heap.heap().clone())]);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRootConstantBufferView(ROOT_FRAME_CBV, frame_allocation.gpu_address());
            list.SetGraphicsRootShaderResourceView(
                crate::scene::ROOT_CLUSTER_SRV,
                cluster_allocation.gpu_address(),
            );
            list.SetGraphicsRootDescriptorTable(ROOT_MATERIAL_TABLE, white.table);
        }

        let mut palette = Vec::with_capacity(MAX_PALETTE_BONES);
        for (index, object) in objects.iter().enumerate() {
            if object.palette.len() > MAX_PALETTE_BONES {
                return Err(RendererError::InvalidConfig {
                    reason: format!(
                        "a palette of {} bones does not fit the shader's array of {}",
                        object.palette.len(),
                        MAX_PALETTE_BONES
                    ),
                });
            }
            palette.clear();
            palette.extend(
                object
                    .palette
                    .iter()
                    .map(engine_core::math::Mat4::to_cols_array),
            );
            palette.resize(
                MAX_PALETTE_BONES,
                engine_core::math::Mat4::IDENTITY.to_cols_array(),
            );
            let skin = ring.write(0, &palette, crate::upload::CONSTANT_BUFFER_ALIGNMENT)?;

            let constants = ObjectConstants::new(object.transform.matrix());
            let allocation = ring.write_constants(0, &constants)?;
            // SAFETY: as above.
            unsafe {
                list.SetGraphicsRootConstantBufferView(
                    ROOT_MATERIAL_CBV,
                    materials[index].gpu_address(),
                );
                list.SetGraphicsRootConstantBufferView(ROOT_OBJECT_CBV, allocation.gpu_address());
                list.SetGraphicsRootConstantBufferView(ROOT_SKIN_CBV, skin.gpu_address());
                list.IASetVertexBuffers(0, Some(&[*meshes[index].vertex_view()]));
                list.IASetIndexBuffer(Some(meshes[index].index_view()));
                list.DrawIndexedInstanced(meshes[index].index_count(), 1, 0, 0, 0);
            }
        }

        Ok(Box::new((pipeline, ring, meshes, materials, white)) as Box<dyn std::any::Any>)
    })
}

/// Renders a set of transformed, shaded meshes through a camera and reads the result back.
///
/// This is the offscreen equivalent of [`crate::Renderer::render_world`] — same pipeline,
/// same constant buffers, same depth state — so a pixel test here is evidence about the
/// path the window actually runs, rather than about a simplified copy of it.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails or a material is invalid.
pub fn render_scene(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    objects: &[SceneObject<'_>],
    camera: &Camera,
    lighting: Lighting,
) -> Result<CapturedImage, RendererError> {
    render_with(width, height, clear_color, |context, commands, _, _| {
        let list = commands.list();
        let pipeline = MeshPipeline::new(context.device())?;
        // One frame slot: this path submits once and waits, so there is nothing to overlap.
        // A megabyte, not 64 KiB: one 256-byte object constant per draw, and the
        // environment tests place a whole arena. Exhausting the ring fails the frame with a
        // ring-capacity error, which looks nothing like the scene bug it would be mistaken
        // for. Offscreen only, allocated once per capture.
        let mut ring = UploadRing::new(context, 1 << 20, 1)?;
        ring.begin_frame(0);

        let mut meshes = Vec::with_capacity(objects.len());
        let mut materials = Vec::with_capacity(objects.len());
        for object in objects {
            meshes.push(GpuMesh::upload(context, object.mesh)?);
            materials.push(Material::new(context, object.material)?);
        }

        let white = white_map(context)?;
        let frame = FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        let frame_allocation = ring.write_constants(0, &frame)?;
        // An empty light grid. These paths have no local lights, so the shader never reads
        // it — but a root SRV the signature declares is bound on every draw that could,
        // rather than left for the validation layer to find.
        let cluster_allocation = empty_light_grid(&mut ring)?;

        // SAFETY: FFI calls recording into the open list. Every referenced object is
        // returned below and kept alive until the submission completes.
        unsafe {
            list.SetGraphicsRootSignature(pipeline.root_signature());
            list.SetPipelineState(pipeline.pipeline_state());
            list.SetDescriptorHeaps(&[Some(white.heap.heap().clone())]);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRootConstantBufferView(ROOT_FRAME_CBV, frame_allocation.gpu_address());
            list.SetGraphicsRootShaderResourceView(
                crate::scene::ROOT_CLUSTER_SRV,
                cluster_allocation.gpu_address(),
            );
            list.SetGraphicsRootDescriptorTable(ROOT_MATERIAL_TABLE, white.table);
        }

        for (index, object) in objects.iter().enumerate() {
            let constants = ObjectConstants::new(object.transform.matrix());
            let allocation = ring.write_constants(0, &constants)?;
            // SAFETY: as above.
            unsafe {
                list.SetGraphicsRootConstantBufferView(
                    ROOT_MATERIAL_CBV,
                    materials[index].gpu_address(),
                );
                list.SetGraphicsRootConstantBufferView(ROOT_OBJECT_CBV, allocation.gpu_address());
                list.IASetVertexBuffers(0, Some(&[*meshes[index].vertex_view()]));
                list.IASetIndexBuffer(Some(meshes[index].index_view()));
                list.DrawIndexedInstanced(meshes[index].index_count(), 1, 0, 0, 0);
            }
        }

        Ok(Box::new((pipeline, ring, meshes, materials, white)) as Box<dyn std::any::Any>)
    })
}

/// One object in a shadowed scene.
///
/// Separate from [`SceneObject`] because it carries the one thing that scene does not: what
/// the object does about shadows. Adding the field to `SceneObject` would have edited twenty
/// call sites in tests that have no opinion about shadows at all.
pub struct ShadowSceneObject<'a> {
    /// Where it is.
    pub transform: Transform,
    /// Its geometry.
    pub mesh: &'a MeshData,
    /// Its surface.
    pub material: MaterialDesc,
    /// Whether it casts, receives, or neither.
    pub shadow: engine_render_data::ShadowCasting,
}


/// One frame's worth of zeroed cluster masks, for a path that has no local lights.
///
/// # Errors
///
/// Returns [`RendererError`] if the upload ring cannot fit it.
fn empty_light_grid(
    ring: &mut crate::upload::UploadRing,
) -> Result<crate::upload::UploadAllocation, RendererError> {
    let empty = crate::light_cluster::LightClusters::new();
    ring.write(0, empty.masks(), crate::upload::DEFAULT_ALIGNMENT)
}

/// Renders a scene *with the real shadow stage*, and reads the result back.
///
/// This is the offscreen counterpart of what the window renders, and it is deliberately
/// built out of the shipping pieces rather than a copy of them: the real
/// [`ShadowRenderer`](crate::shadow::ShadowRenderer), the real cascade mathematics, the real
/// depth-only pipelines, the real atlas, and the real mesh pipeline sampling it. A pixel test
/// on the result is therefore evidence about the path the game runs.
///
/// ```text
/// shadow.prepare()   cascades and caster lists      (CPU)
/// shadow.record()    the atlas, one viewport per view
/// mesh pass          the same scene, sampling the atlas
/// -> pixels
/// ```
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
#[allow(
    clippy::too_many_arguments,
    reason = "an offscreen frame is a size, a clear, a scene, its lights, a camera, its               lighting and the settings under test; every one of them varies per test"
)]
pub fn render_scene_shadowed(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    objects: &[ShadowSceneObject<'_>],
    lights: &[engine_render_data::LocalLight],
    camera: &Camera,
    lighting: Lighting,
    settings: crate::shadow::ShadowSettings,
) -> Result<CapturedImage, RendererError> {
    render_scene_shadowed_debug(
        width,
        height,
        clear_color,
        objects,
        lights,
        camera,
        lighting,
        settings,
        0,
    )
}

/// [`render_scene_shadowed`], with one of the shadow debug views switched on.
///
/// `debug_mode` is the value [`Renderer::set_shadow_debug_mode`](crate::Renderer::set_shadow_debug_mode)
/// takes. It exists so the debug views can be *tested*: they are the tools a person reaches
/// for when a shadow looks wrong, and a broken tool is worst exactly then — the one moment
/// nobody is in a position to notice it is the tool that is lying.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
#[allow(
    clippy::too_many_arguments,
    reason = "an offscreen frame is this many things"
)]
pub fn render_scene_shadowed_debug(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    objects: &[ShadowSceneObject<'_>],
    lights: &[engine_render_data::LocalLight],
    camera: &Camera,
    lighting: Lighting,
    settings: crate::shadow::ShadowSettings,
    debug_mode: u32,
) -> Result<CapturedImage, RendererError> {
    render_with(
        width,
        height,
        clear_color,
        |context, commands, rtv, depth| {
            let dsv = depth.view().cpu;
            let depth_texture = depth.texture();
            let pipeline = MeshPipeline::new(context.device())?;
            let mut ring = UploadRing::new(context, 4 << 20, 1)?;
            ring.begin_frame(0);

            // One shader-visible heap for everything the frame samples: the white base colour
            // map, the shadow atlas and the depth copy. Only one such heap can be bound at a
            // time, which is exactly why the renderer has one and why this path must too.
            let mut descriptors = crate::descriptor::GpuDescriptorHeap::new(context, 16, 0, 1)?;
            let white_texture = white_texture(context)?;
            let white_handle = descriptors.allocate_persistent()?;
            create_white_view(context, &white_texture, white_handle);

            let mut shadow = crate::shadow::ShadowRenderer::new(
                context,
                &mut descriptors,
                settings,
                (width, height),
            )?;
            shadow.set_debug_mode(debug_mode);

            let mut meshes = Vec::with_capacity(objects.len());
            let mut materials = Vec::with_capacity(objects.len());
            let mut tables = Vec::with_capacity(objects.len());
            let mut world = engine_render_data::RenderWorld::with_capacity(objects.len());
            for (index, object) in objects.iter().enumerate() {
                meshes.push(GpuMesh::upload(context, object.mesh)?);
                materials.push(Material::new(context, object.material)?);
                tables.push(white_handle.gpu);
                world.spawn(engine_render_data::RenderEntityDesc {
                    transform: object.transform,
                    mesh: engine_render_data::MeshHandle::new(index as u32, 1),
                    material: engine_render_data::MaterialHandle::new(index as u32, 1),
                    bounds: object.mesh.bounds(),
                    visible: true,
                    style: engine_render_data::InstanceStyle::NEUTRAL,
                    shadow: object.shadow,
                });
            }
            for light in lights {
                world.push_light(*light);
            }

            commands.set_descriptor_heaps(&[Some(descriptors.heap().clone())]);

            // The shadow stage, exactly as the renderer runs it.
            shadow.prepare(&world, camera, lighting);
            let empty = engine_render_data::SkinnedDrawList::new();
            let scene = crate::shadow::renderer::ShadowScene {
                meshes: &meshes,
                skinned_meshes: &[],
                materials: &materials,
                material_tables: &tables,
                skinned: &empty,
            };
            shadow.record(commands, &mut ring, 0, &scene)?;

            // The camera's depth prepass, when the settings ask for one, so the contact-shadow
            // march has depth to march against — and so this path exercises it at all.
            if shadow.needs_prepass() {
                let mut visible = crate::render_list::RenderList::new();
                let mut culler = crate::culling::Culler::new();
                culler.accept_all(&world, &mut visible);
                visible.group_by_material_and_mesh(usize::MAX, usize::MAX);
                let scene = crate::shadow::renderer::ShadowScene {
                    meshes: &meshes,
                    skinned_meshes: &[],
                    materials: &materials,
                    material_tables: &tables,
                    skinned: &empty,
                };
                shadow.record_prepass(
                    commands,
                    &mut ring,
                    0,
                    &scene,
                    &visible,
                    camera,
                    dsv,
                    (width, height),
                )?;
                shadow.copy_scene_depth(commands, depth_texture);
            }

            // Back to the camera's target, which the shadow pass replaced with the atlas.
            commands.set_render_target_and_depth(rtv, dsv);
            commands.set_viewport(width, height);

            let mut frame =
                FrameConstants::new(camera.view_projection(), camera.position(), lighting);
            frame.set_camera(camera);
            frame.set_viewport(width, height);
            shadow.write_frame_constants(&mut frame, world.lights());
            // The same clustering the window does, from the same camera. This path exists to
            // be evidence about the shipping one, and a local-light test that ran an
            // unclustered shader would be evidence about a path the game does not take.
            let mut clusters = crate::light_cluster::LightClusters::new();
            clusters.build(world.lights(), camera);
            frame.set_light_clusters(&clusters);
            let frame_allocation = ring.write_constants(0, &frame)?;
            let cluster_allocation =
                ring.write(0, clusters.masks(), crate::upload::DEFAULT_ALIGNMENT)?;

            commands.set_pipeline(pipeline.root_signature(), pipeline.pipeline_state());
            commands.set_topology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            commands.set_root_cbv(ROOT_FRAME_CBV, frame_allocation.gpu_address());
            commands.set_root_srv(
                crate::scene::ROOT_CLUSTER_SRV,
                cluster_allocation.gpu_address(),
            );
            if let Some(table) = white_handle.gpu {
                commands.set_root_descriptor_table(ROOT_MATERIAL_TABLE, table);
            }
            if let Some(table) = shadow.atlas_table() {
                commands.set_root_descriptor_table(crate::scene::ROOT_SHADOW_TABLE, table);
            }
            if let Some(table) = shadow.depth_table() {
                commands.set_root_descriptor_table(crate::scene::ROOT_DEPTH_TABLE, table);
            }

            for (index, object) in objects.iter().enumerate() {
                let constants = ObjectConstants::shadowed(
                    object.transform.matrix(),
                    engine_render_data::InstanceStyle::NEUTRAL,
                    object.shadow.receives,
                );
                let allocation = ring.write_constants(0, &constants)?;
                commands.set_root_cbv(ROOT_MATERIAL_CBV, materials[index].gpu_address());
                commands.set_root_cbv(ROOT_OBJECT_CBV, allocation.gpu_address());
                commands.set_vertex_buffer(0, *meshes[index].vertex_view());
                commands.set_index_buffer(*meshes[index].index_view());
                commands.draw_indexed(meshes[index].index_count(), 1, 0);
            }

            Ok(Box::new((
                pipeline,
                ring,
                meshes,
                materials,
                white_texture,
                descriptors,
                shadow,
            )) as Box<dyn std::any::Any>)
        },
    )
}

/// Renders a [`ParticleDrawList`](engine_render_data::ParticleDrawList) through a camera
/// and reads the result back.
///
/// The offscreen equivalent of the renderer's particle pass: the same pipeline states, the
/// same premultiplied blending, the same depth state and the same per-instance stream, so a
/// pixel test here is evidence about the path the window runs rather than about a copy of
/// it. There is no opaque scene beneath it — the depth buffer is cleared to far, which is
/// what a particle in open air tests against anyway.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
pub fn render_particles(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    particles: &engine_render_data::ParticleDrawList,
    camera: &Camera,
) -> Result<CapturedImage, RendererError> {
    use crate::particle_pipeline::{ParticleConstants, ParticlePipeline, ROOT_PARTICLE_FRAME_CBV};
    use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP;
    use windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW;

    render_with(width, height, clear_color, |context, commands, _, _| {
        let list = commands.list();
        let pipeline = ParticlePipeline::new(context.device())?;
        let mut ring = UploadRing::new(context, 1 << 20, 1)?;
        ring.begin_frame(0);

        let constants = ParticleConstants::new(&camera.view(), &camera.view_projection());
        let frame = ring.write_constants(0, &constants)?;

        // Held until the submission completes: the vertex views below address this memory.
        let mut allocations = Vec::with_capacity(2);
        for blend in engine_render_data::ParticleBlend::ALL {
            let instances = particles.of(blend);
            if instances.is_empty() {
                continue;
            }
            let bytes = std::mem::size_of_val(instances);
            // Sixteen, not the 36-byte stride: the ring's aligner requires a power of two,
            // and a vertex buffer needs an indexable address rather than a multiple of its
            // stride. The renderer's own pass uses the same constant.
            let allocation = ring.allocate(0, bytes, 16)?;
            allocation.write(instances)?;

            // SAFETY: FFI calls recording into the open list. Every referenced object is
            // returned below and kept alive until the submission completes.
            unsafe {
                list.SetGraphicsRootSignature(pipeline.root_signature());
                list.SetPipelineState(pipeline.state(blend));
                list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
                list.SetGraphicsRootConstantBufferView(
                    ROOT_PARTICLE_FRAME_CBV,
                    frame.gpu_address(),
                );
                list.IASetVertexBuffers(
                    0,
                    Some(&[D3D12_VERTEX_BUFFER_VIEW {
                        BufferLocation: allocation.gpu_address(),
                        SizeInBytes: bytes as u32,
                        StrideInBytes: size_of::<engine_render_data::ParticleInstance>() as u32,
                    }]),
                );
                list.DrawInstanced(4, instances.len() as u32, 0, 0);
            }
            allocations.push(allocation);
        }

        Ok(Box::new((pipeline, ring, allocations)) as Box<dyn std::any::Any>)
    })
}

/// Renders a [`UiDrawList`] over a cleared target and reads the pixels back.
///
/// The only way to prove alpha blending actually blends. A draw-call count and a silent
/// validation layer say a quad was submitted; they cannot distinguish a translucent panel
/// from an opaque one, and the difference is the whole point of the pass.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
pub fn render_ui(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    list_to_draw: &engine_render_data::UiDrawList,
) -> Result<CapturedImage, RendererError> {
    use windows::Win32::Graphics::Direct3D12::{
        D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        D3D12_SHADER_RESOURCE_VIEW_DESC, D3D12_SHADER_RESOURCE_VIEW_DESC_0,
        D3D12_SRV_DIMENSION_TEXTURE2D, D3D12_TEX2D_SRV, D3D12_VERTEX_BUFFER_VIEW,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;

    use crate::descriptor::GpuDescriptorHeap;
    use crate::resource::{GpuTexture, TrackedResource};
    use crate::ui_atlas::UiAtlas;
    use crate::ui_pipeline::{
        ROOT_UI_ATLAS_TABLE, ROOT_UI_FRAME_CBV, UiBatcher, UiFrameConstants, UiPipeline, UiVertex,
    };
    use crate::upload::StagingUploader;

    render_with(width, height, clear_color, |context, commands, _, _| {
        let list = commands.list();
        let pipeline = UiPipeline::new(context.device())?;

        let atlas_pixels = UiAtlas::new();
        let atlas = GpuTexture::new(
            context,
            atlas_pixels.width(),
            atlas_pixels.height(),
            DXGI_FORMAT_R8G8B8A8_UNORM,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let mut uploader = StagingUploader::new(context)?;
        uploader.queue_texture(
            context,
            &atlas,
            atlas_pixels.texels(),
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        )?;
        uploader.flush(context)?;

        let mut heap = GpuDescriptorHeap::new(context, 1, 0, 1)?;
        let descriptor = heap.allocate_persistent()?;
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: 0x1688,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                    PlaneSlice: 0,
                    ResourceMinLODClamp: 0.0,
                },
            },
        };
        // SAFETY: FFI call. The texture and heap are returned below and outlive the
        // submission; the handle addresses a slot this heap just allocated.
        unsafe {
            context.device().CreateShaderResourceView(
                Some(TrackedResource::resource(&atlas)),
                Some(&srv),
                descriptor.cpu,
            );
        }

        let mut batcher = UiBatcher::new(1 << 16);
        let vertices = batcher.build(list_to_draw).to_vec();
        let mut ring = UploadRing::new(context, 1 << 20, 1)?;
        ring.begin_frame(0);

        let table = descriptor.gpu.ok_or_else(|| RendererError::InvalidConfig {
            reason: "the offscreen atlas descriptor is not shader visible".to_owned(),
        })?;
        let constants = UiFrameConstants::for_viewport(width, height);
        let constant_allocation = ring.write_constants(0, &constants)?;

        if !vertices.is_empty() {
            let bytes = std::mem::size_of_val(vertices.as_slice());
            let allocation = ring.write(0, &vertices, 16)?;
            // SAFETY: FFI calls recording into the open list. Every referenced object is
            // returned below and kept alive until the submission completes.
            unsafe {
                list.SetGraphicsRootSignature(pipeline.root_signature());
                list.SetPipelineState(pipeline.state());
                list.SetDescriptorHeaps(&[Some(heap.heap().clone())]);
                list.SetGraphicsRootConstantBufferView(
                    ROOT_UI_FRAME_CBV,
                    constant_allocation.gpu_address(),
                );
                list.SetGraphicsRootDescriptorTable(ROOT_UI_ATLAS_TABLE, table);
                list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                list.IASetVertexBuffers(
                    0,
                    Some(&[D3D12_VERTEX_BUFFER_VIEW {
                        BufferLocation: allocation.gpu_address(),
                        SizeInBytes: bytes as u32,
                        StrideInBytes: size_of::<UiVertex>() as u32,
                    }]),
                );
                list.DrawInstanced(vertices.len() as u32, 1, 0, 0);
            }
        }

        Ok(Box::new((pipeline, ring, atlas, heap, uploader)) as Box<dyn std::any::Any>)
    })
}

/// Renders the same scene as [`render_scene`], one instanced draw per run of objects
/// sharing a mesh and a material.
///
/// Exists so the two submission paths can be compared pixel for pixel. Milestone 17's
/// instanced pipeline reconstructs the model matrix from four per-instance vertex elements
/// rather than reading it from a constant buffer, and both the semantic indexing and the
/// multiply order are conventions that are invisible on the CPU and catastrophic on the
/// GPU — a wrong one renders every object sheared and enormous while every counter still
/// reads correct.
///
/// Objects are grouped by `(material index, mesh index)` in the order given, matching what
/// `RenderList` does, so a run here is a batch there.
///
/// # Errors
///
/// Returns [`RendererError`] if any D3D12 call fails.
pub fn render_scene_instanced(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    objects: &[SceneObject<'_>],
    camera: &Camera,
    lighting: Lighting,
) -> Result<CapturedImage, RendererError> {
    render_with(width, height, clear_color, |context, commands, _, _| {
        let list = commands.list();
        let pipeline = InstancedMeshPipeline::new(context.device())?;
        // A megabyte, not 64 KiB: one 256-byte object constant per draw, and the
        // environment tests place a whole arena. Exhausting the ring fails the frame with a
        // ring-capacity error, which looks nothing like the scene bug it would be mistaken
        // for. Offscreen only, allocated once per capture.
        let mut ring = UploadRing::new(context, 1 << 20, 1)?;
        ring.begin_frame(0);

        let mut meshes = Vec::with_capacity(objects.len());
        let mut materials = Vec::with_capacity(objects.len());
        for object in objects {
            meshes.push(GpuMesh::upload(context, object.mesh)?);
            materials.push(Material::new(context, object.material)?);
        }

        let white = white_map(context)?;
        let frame = FrameConstants::new(camera.view_projection(), camera.position(), lighting);
        let frame_allocation = ring.write_constants(0, &frame)?;
        // An empty light grid. These paths have no local lights, so the shader never reads
        // it — but a root SRV the signature declares is bound on every draw that could,
        // rather than left for the validation layer to find.
        let cluster_allocation = empty_light_grid(&mut ring)?;

        // SAFETY: FFI calls recording into the open list. Every referenced object is
        // returned below and kept alive until the submission completes.
        unsafe {
            list.SetGraphicsRootSignature(pipeline.root_signature());
            list.SetPipelineState(pipeline.pipeline_state());
            list.SetDescriptorHeaps(&[Some(white.heap.heap().clone())]);
            list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            list.SetGraphicsRootConstantBufferView(ROOT_FRAME_CBV, frame_allocation.gpu_address());
            list.SetGraphicsRootShaderResourceView(
                crate::scene::ROOT_CLUSTER_SRV,
                cluster_allocation.gpu_address(),
            );
            list.SetGraphicsRootDescriptorTable(ROOT_MATERIAL_TABLE, white.table);
        }

        // Consecutive objects sharing a mesh and a material form one batch, which is what
        // grouping produces in the real path.
        let mut start = 0;
        while start < objects.len() {
            let mut end = start + 1;
            while end < objects.len()
                && std::ptr::eq(objects[end].mesh, objects[start].mesh)
                && objects[end].material == objects[start].material
            {
                end += 1;
            }

            let instances: Vec<InstanceTransform> = objects[start..end]
                .iter()
                .map(|object| InstanceTransform::new(object.transform.matrix()))
                .collect();
            // Sixteen, not the record size: every element in the stream is a `float4` or
            // smaller, and the aligner requires a power of two — which 96 is not.
            let allocation = ring.write(0, &instances, 16)?;

            // SAFETY: as above.
            unsafe {
                list.SetGraphicsRootConstantBufferView(
                    ROOT_MATERIAL_CBV,
                    materials[start].gpu_address(),
                );
                list.IASetVertexBuffers(0, Some(&[*meshes[start].vertex_view()]));
                list.IASetVertexBuffers(
                    1,
                    Some(&[
                        windows::Win32::Graphics::Direct3D12::D3D12_VERTEX_BUFFER_VIEW {
                            BufferLocation: allocation.gpu_address(),
                            SizeInBytes: std::mem::size_of_val(instances.as_slice()) as u32,
                            StrideInBytes: size_of::<InstanceTransform>() as u32,
                        },
                    ]),
                );
                list.IASetIndexBuffer(Some(meshes[start].index_view()));
                list.DrawIndexedInstanced(
                    meshes[start].index_count(),
                    (end - start) as u32,
                    0,
                    0,
                    0,
                );
            }
            start = end;
        }

        Ok(Box::new((pipeline, ring, meshes, materials, white)) as Box<dyn std::any::Any>)
    })
}

/// Creates an offscreen target, lets `record` draw into it, and reads the pixels back.
///
/// `record` returns whatever it must keep alive: a command list holds raw pointers, not
/// references, so a pipeline or buffer dropped between recording and submission is a
/// use-after-free the type system cannot see. Handing ownership back is what makes that
/// impossible rather than merely unlikely.
fn render_with(
    width: u32,
    height: u32,
    clear_color: [f32; 4],
    record: impl FnOnce(
        &RenderDevice,
        &mut CommandContext,
        D3D12_CPU_DESCRIPTOR_HANDLE,
        &DepthBuffer,
    ) -> Result<Box<dyn std::any::Any>, RendererError>,
) -> Result<CapturedImage, RendererError> {
    if width == 0 || height == 0 {
        return Err(RendererError::InvalidConfig {
            reason: format!("offscreen size {width}x{height} has a zero dimension"),
        });
    }

    // `OFFSCREEN_DEBUG_LAYER=1` turns on the D3D12 validation layer for this submission and
    // prints whatever it has to say when the submission fails. Off by default because the
    // layer costs several times the CPU time of the work being validated, and on by an
    // environment variable rather than a feature because the moment it is wanted is *after*
    // a failure, when rebuilding is the last thing that helps.
    let debug_layer = std::env::var("OFFSCREEN_DEBUG_LAYER").is_ok();
    let context = RenderDevice::new(debug_layer, false)?;
    let device = context.device();

    let target_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(width),
        Height: height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: BACK_BUFFER_FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        ..Default::default()
    };
    let default_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };

    let mut target: Option<ID3D12Resource> = None;
    // SAFETY: FFI call with fully initialized descriptors that outlive it, and a valid
    // out-parameter. RENDER_TARGET is a legal initial state for this resource.
    unsafe {
        device.CreateCommittedResource(
            &default_heap,
            D3D12_HEAP_FLAG_NONE,
            &target_desc,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            None,
            &mut target,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;
    let Some(target) = target else {
        return Err(RendererError::InvalidConfig {
            reason: "offscreen render target creation produced no resource".to_owned(),
        });
    };

    // Descriptor heap with a single RTV for the offscreen texture.
    let heap_desc = D3D12_DESCRIPTOR_HEAP_DESC {
        Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
        NumDescriptors: 1,
        Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        NodeMask: 0,
    };
    // SAFETY: FFI call with an initialized descriptor that outlives the call.
    let rtv_heap: ID3D12DescriptorHeap = unsafe { device.CreateDescriptorHeap(&heap_desc) }
        .map_err(|error| RendererError::api("ID3D12Device::CreateDescriptorHeap", error))?;
    // SAFETY: FFI calls creating a view of a live resource in our own heap.
    let rtv = unsafe {
        let handle = rtv_heap.GetCPUDescriptorHandleForHeapStart();
        device.CreateRenderTargetView(&target, None, handle);
        handle
    };

    // Readback buffer. Texture rows are padded to D3D12_TEXTURE_DATA_PITCH_ALIGNMENT, so
    // the buffer is larger than width * height * 4 and the padding is stripped later.
    let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total_bytes = 0u64;
    // SAFETY: FFI call. `target_desc` describes the texture being copied and every
    // out-parameter is a valid, owned value.
    unsafe {
        device.GetCopyableFootprints(
            &target_desc,
            0,
            1,
            0,
            Some(&mut footprint),
            None,
            None,
            Some(&mut total_bytes),
        );
    }

    let readback_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: total_bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let readback_heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_READBACK,
        ..Default::default()
    };
    let mut readback: Option<ID3D12Resource> = None;
    // SAFETY: FFI call as above. COPY_DEST is the required initial state for a readback
    // buffer that a copy writes into.
    unsafe {
        device.CreateCommittedResource(
            &readback_heap,
            D3D12_HEAP_FLAG_NONE,
            &readback_desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
            &mut readback,
        )
    }
    .map_err(|error| RendererError::api("ID3D12Device::CreateCommittedResource", error))?;
    let Some(readback) = readback else {
        return Err(RendererError::InvalidConfig {
            reason: "readback buffer creation produced no resource".to_owned(),
        });
    };

    // Every pipeline here declares a depth format, so a depth buffer must be bound: a PSO
    // with a `DSVFormat` and no view bound disables depth silently, which would make this
    // path test something different from what the window path runs.
    let depth = DepthBuffer::new(&context, width, height)?;

    // SAFETY: FFI call on a live device with a valid command list type.
    let allocator: ID3D12CommandAllocator =
        unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }
            .map_err(|error| RendererError::api("ID3D12Device::CreateCommandAllocator", error))?;
    // A `CommandContext` rather than a bare list, so a pass written for the renderer — the
    // shadow stage is one — records here through exactly the same type it records through in
    // the window path, barrier tracking and redundant-bind elision included.
    let mut commands = CommandContext::new(&context, &allocator)?;
    commands.begin(&allocator)?;
    let list = commands.list().clone();

    let source = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: non-owning copy of the interface pointer, kept in a ManuallyDrop so it
        // is never released. `target` outlives this structure.
        pResource: unsafe { std::mem::transmute_copy(&target) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    };
    let destination = D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above; `readback` outlives this structure.
        pResource: unsafe { std::mem::transmute_copy(&readback) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };

    // SAFETY: FFI calls recording into the open list. Every referenced object is owned by
    // this function and outlives the recording, and the resource states match the
    // transitions requested.
    unsafe {
        list.RSSetViewports(&[viewport_for(width, height)]);
        list.RSSetScissorRects(&[scissor_for(width, height)]);
        let dsv = depth.view().cpu;
        list.OMSetRenderTargets(1, Some(&rtv), false, Some(&dsv));
        list.ClearRenderTargetView(rtv, &clear_color, None);
        list.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, DEPTH_CLEAR, 0, None);
    }

    let recorded = match record(&context, &mut commands, rtv, &depth) {
        Ok(recorded) => recorded,
        Err(error) => {
            drain_validation_messages(&context);
            return Err(error);
        }
    };

    // Any barrier the recorded pass left queued has to reach the list before the copy that
    // reads the target — the context batches them, and this is the last chance.
    commands.flush_barriers();

    // SAFETY: FFI calls recording into the same open list; the target and readback buffer
    // outlive the submission this function waits for.
    unsafe {
        list.ResourceBarrier(&[transition_barrier(
            &target,
            D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        )]);
        list.CopyTextureRegion(&destination, 0, 0, 0, &source, None);
    }

    if let Err(error) = commands.close() {
        drain_validation_messages(&context);
        return Err(error);
    }

    let submission = [Some(list.cast().map_err(|error| {
        RendererError::api("ID3D12GraphicsCommandList::cast", error)
    })?)];
    // SAFETY: FFI call submitting the closed list, which outlives the call.
    unsafe { context.queue().ExecuteCommandLists(&submission) };

    let mut fence = crate::sync::GpuFence::new(&context)?;
    let waited = fence.wait_for_idle(context.queue());
    if waited.is_err() {
        // The GPU did not finish. Whatever the validation layer recorded is the only
        // description of why, and it is lost the moment the device is dropped.
        drain_validation_messages(&context);
    }
    waited?;

    if debug_layer {
        // Everything the layer had to say about a submission that *worked*, which is where
        // a silently ignored copy or a mismatched state shows up.
        drain_validation_messages(&context);
    }
    let pixels = read_pixels(&readback, &footprint, width, height, total_bytes)?;
    // Everything the list referenced stayed alive until here, which is the point of
    // handing it back rather than letting the closure drop it.
    drop(recorded);

    Ok(CapturedImage {
        width,
        height,
        pixels,
    })
}

/// Maps the readback buffer and copies its rows into a tightly packed image.
fn read_pixels(
    readback: &ID3D12Resource,
    footprint: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    width: u32,
    height: u32,
    total_bytes: u64,
) -> Result<Vec<u8>, RendererError> {
    let row_pitch = footprint.Footprint.RowPitch as usize;
    let row_bytes = width as usize * BYTES_PER_PIXEL;

    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    let read_range = D3D12_RANGE {
        Begin: 0,
        End: total_bytes as usize,
    };
    // SAFETY: FFI call mapping subresource 0 of a readback buffer, declaring that the
    // whole range will be read.
    unsafe { readback.Map(0, Some(&read_range), Some(&mut mapped)) }
        .map_err(|error| RendererError::api("ID3D12Resource::Map", error))?;

    let mut pixels = vec![0u8; row_bytes * height as usize];
    // SAFETY: the mapping covers `total_bytes`, which GetCopyableFootprints sized to hold
    // `height` rows of `row_pitch` bytes; each copied row stays inside that range, and
    // source and destination are distinct allocations.
    unsafe {
        let base = mapped.cast::<u8>();
        for row in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                base.add(row * row_pitch),
                pixels.as_mut_ptr().add(row * row_bytes),
                row_bytes,
            );
        }
        // A written range of 0..0 tells the driver the CPU changed nothing.
        readback.Unmap(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }));
    }

    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_lookup_is_row_major_and_bounds_checked() {
        let image = CapturedImage {
            width: 2,
            height: 2,
            pixels: vec![
                1, 2, 3, 4, // (0, 0)
                5, 6, 7, 8, // (1, 0)
                9, 10, 11, 12, // (0, 1)
                13, 14, 15, 16, // (1, 1)
            ],
        };
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
        assert_eq!(image.pixel(1, 0), Some([5, 6, 7, 8]));
        assert_eq!(image.pixel(0, 1), Some([9, 10, 11, 12]));
        assert_eq!(image.pixel(1, 1), Some([13, 14, 15, 16]));
        assert_eq!(image.pixel(2, 0), None);
        assert_eq!(image.pixel(0, 2), None);
    }

    #[test]
    fn counts_matching_pixels() {
        let image = CapturedImage {
            width: 2,
            height: 1,
            pixels: vec![255, 0, 0, 255, 0, 0, 0, 255],
        };
        assert_eq!(image.count_pixels(|p| p[0] > 128), 1);
        assert_eq!(image.count_pixels(|p| p[3] == 255), 2);
    }

    #[test]
    fn rejects_a_zero_sized_target() {
        assert!(render_triangle(0, 16, [0.0; 4]).is_err());
        assert!(render_triangle(16, 0, [0.0; 4]).is_err());
    }
}
