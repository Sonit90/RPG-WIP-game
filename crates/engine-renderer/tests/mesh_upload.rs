//! Indexed meshes against a real device.
//!
//! Mesh *construction* is unit-tested in `mesh.rs` without a GPU. What needs a driver is
//! that the vertices and indices actually reach device-local memory, that the views point
//! at them, and that a batch of meshes costs one synchronization rather than one per mesh.
//! An upload that silently wrote nowhere looks identical from the CPU side.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_renderer::RendererError;
use engine_renderer::command::CommandContext;
use engine_renderer::device::RenderDevice;
use engine_renderer::mesh::{GpuMesh, Index, MeshData, MeshVertex};
use engine_renderer::resource::{GpuBuffer, TrackedResource};

use windows::Win32::Graphics::Direct3D12::{
    D3D12_HEAP_TYPE_READBACK, D3D12_RANGE, D3D12_RESOURCE_STATE_COPY_DEST,
    D3D12_RESOURCE_STATE_COPY_SOURCE, ID3D12CommandList,
};
use windows::core::Interface;

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn device_or_skip(test: &str) -> Option<RenderDevice> {
    match RenderDevice::new(false, false) {
        Ok(device) => Some(device),
        Err(RendererError::NoSuitableAdapter { examined }) => {
            println!("SKIP: {test} — no D3D12 adapter ({examined} examined)");
            None
        }
        Err(error) => panic!("device creation failed: {error}"),
    }
}

/// Copies `bytes` from `source` into a readback buffer and returns them.
fn read_back(device: &RenderDevice, source: &GpuBuffer, bytes: usize) -> Vec<u8> {
    let readback = device
        .create_buffer(
            bytes,
            D3D12_HEAP_TYPE_READBACK,
            D3D12_RESOURCE_STATE_COPY_DEST,
        )
        .expect("readback buffer must be creatable");

    let allocator = device
        .create_command_allocator()
        .expect("allocator must be creatable");
    let mut context = CommandContext::new(device, &allocator).expect("context must be creatable");
    context.begin(&allocator).expect("begin must succeed");
    context.transition_to(source, D3D12_RESOURCE_STATE_COPY_SOURCE);
    context.flush_barriers();

    // SAFETY: FFI call on the open list. Both resources outlive the submission, which this
    // function waits for before returning.
    unsafe {
        context
            .list()
            .CopyBufferRegion(&readback, 0, source.resource(), 0, bytes as u64);
    }
    context.close().expect("close must succeed");

    let submission = [Some(
        context
            .list()
            .cast::<ID3D12CommandList>()
            .expect("cast must succeed"),
    )];
    // SAFETY: FFI call submitting a closed list that outlives the call.
    unsafe { device.queue().ExecuteCommandLists(&submission) };
    let mut fence = engine_renderer::sync::GpuFence::new(device).expect("fence must be creatable");
    fence
        .wait_for_idle(device.queue())
        .expect("idle wait must succeed");

    let mut mapped: *mut std::ffi::c_void = std::ptr::null_mut();
    let range = D3D12_RANGE {
        Begin: 0,
        End: bytes,
    };
    // SAFETY: FFI call mapping subresource 0 of a readback buffer this function owns; the
    // read range covers exactly the bytes copied above.
    unsafe { readback.Map(0, Some(&range), Some(&mut mapped)) }.expect("map must succeed");
    // SAFETY: `mapped` addresses at least `bytes` readable bytes, which the copy filled.
    let values = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), bytes) }.to_vec();
    // SAFETY: unmapping the buffer mapped immediately above; a null written range states
    // the CPU wrote nothing.
    unsafe { readback.Unmap(0, None) };

    values
}

/// Reinterprets readback bytes as a slice of `T`.
fn cast_back<T: Copy>(bytes: &[u8], count: usize) -> Vec<T> {
    assert_eq!(bytes.len(), count * size_of::<T>());
    // SAFETY: `bytes` is at least `count * size_of::<T>()` bytes and `T: Copy` has no
    // invalid bit patterns among the plain-data types this test uses.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<T>(), count) }.to_vec()
}

#[test]
fn a_cube_reaches_the_gpu_vertex_for_vertex() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_cube_reaches_the_gpu_vertex_for_vertex") else {
        return;
    };

    let data = MeshData::cube(2.0);
    let mesh = GpuMesh::upload(&device, &data).expect("upload must succeed");

    let vertices: Vec<MeshVertex> = cast_back(
        &read_back(
            &device,
            mesh.vertex_buffer(),
            std::mem::size_of_val(data.vertices()),
        ),
        data.vertices().len(),
    );
    assert_eq!(vertices, data.vertices());

    let indices: Vec<Index> = cast_back(
        &read_back(
            &device,
            mesh.index_buffer(),
            std::mem::size_of_val(data.indices()),
        ),
        data.indices().len(),
    );
    assert_eq!(indices, data.indices());
}

#[test]
fn a_vertexs_tangent_and_colour_reach_the_gpu_at_the_offsets_the_layout_names() {
    // The byte-level half of the vertex format. `a_cube_reaches_the_gpu_vertex_for_vertex`
    // compares whole structs, which would still pass if `MeshVertex` and the input layout
    // agreed with each other and both were wrong. This reads the raw bytes back and finds
    // the tangent at 32 and the colour at 48 — the two offsets `mesh_vertex_elements()`
    // hard-codes, and the ones an HLSL `TANGENT`/`COLOR` semantic will fetch from.
    let _guard = gpu_lock();
    let Some(device) = device_or_skip(
        "a_vertexs_tangent_and_colour_reach_the_gpu_at_the_offsets_the_layout_names",
    ) else {
        return;
    };

    // Values chosen so no two of them could be confused for each other, and so a colour read
    // as floats or a tangent read from the UV would be obvious rather than plausible.
    let tangent = Vec4::new(0.0, 0.0, 1.0, -1.0);
    let color = [17_u8, 34, 51, 68];
    let vertices = vec![
        MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO)
            .with_tangent(tangent)
            .with_color(color),
        MeshVertex::new(Vec3::X, Vec3::Y, Vec2::new(1.0, 0.0))
            .with_tangent(tangent)
            .with_color(color),
        MeshVertex::new(Vec3::Z, Vec3::Y, Vec2::new(0.0, 1.0))
            .with_tangent(tangent)
            .with_color(color),
    ];
    let data = MeshData::new(vertices, vec![0, 1, 2]).expect("the triangle is well formed");
    let mesh = GpuMesh::upload(&device, &data).expect("upload must succeed");

    let stride = size_of::<MeshVertex>();
    assert_eq!(stride, 52, "the offsets below describe a 52-byte vertex");
    let bytes = read_back(
        &device,
        mesh.vertex_buffer(),
        std::mem::size_of_val(data.vertices()),
    );

    for vertex in 0..data.vertices().len() {
        let base = vertex * stride;
        let read_f32 = |at: usize| {
            let mut four = [0_u8; 4];
            four.copy_from_slice(&bytes[at..at + 4]);
            f32::from_le_bytes(four)
        };
        assert_eq!(read_f32(base + 32), tangent.x, "vertex {vertex} tangent.x");
        assert_eq!(read_f32(base + 36), tangent.y, "vertex {vertex} tangent.y");
        assert_eq!(read_f32(base + 40), tangent.z, "vertex {vertex} tangent.z");
        assert_eq!(
            read_f32(base + 44),
            tangent.w,
            "vertex {vertex} handedness — the half of a tangent a mirrored UV island needs"
        );
        assert_eq!(
            &bytes[base + 48..base + 52],
            &color,
            "vertex {vertex} colour, four UNORM bytes rather than four floats"
        );
    }
}

#[test]
fn a_hand_authored_vertex_carries_a_usable_tangent_rather_than_zeros() {
    // Geometry built in this crate — the offscreen helpers, the stress scene, the showcase —
    // has no UVs worth deriving a tangent from, so `MeshVertex::new` gives it the
    // deterministic perpendicular frame. What must never reach a vertex buffer is a zero
    // tangent: normal mapping would read it as a degenerate frame and shade the surface
    // black, and this is the milestone that freezes the format.
    let _guard = gpu_lock();
    let Some(device) =
        device_or_skip("a_hand_authored_vertex_carries_a_usable_tangent_rather_than_zeros")
    else {
        return;
    };

    let data = MeshData::uv_sphere(1.0, 16, 8);
    let mesh = GpuMesh::upload(&device, &data).expect("upload must succeed");
    let vertices: Vec<MeshVertex> = cast_back(
        &read_back(
            &device,
            mesh.vertex_buffer(),
            std::mem::size_of_val(data.vertices()),
        ),
        data.vertices().len(),
    );

    for (at, vertex) in vertices.iter().enumerate() {
        let tangent = vertex.tangent;
        let length = (tangent.x * tangent.x + tangent.y * tangent.y + tangent.z * tangent.z).sqrt();
        assert!(
            (length - 1.0).abs() < 1e-4,
            "vertex {at} tangent is not unit: {length}"
        );
        assert!(
            (tangent.w.abs() - 1.0).abs() < 1e-6,
            "vertex {at} handedness is {}",
            tangent.w
        );
        let dot =
            vertex.normal.x * tangent.x + vertex.normal.y * tangent.y + vertex.normal.z * tangent.z;
        assert!(
            dot.abs() < 1e-4,
            "vertex {at} tangent is not perpendicular to its normal: {dot}"
        );
        assert_eq!(
            vertex.color,
            [255, 255, 255, 255],
            "vertex {at} colour must be the multiplicative identity"
        );
    }
}

#[test]
fn the_views_describe_the_buffers_they_point_at() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("the_views_describe_the_buffers_they_point_at") else {
        return;
    };

    let data = MeshData::uv_sphere(1.0, 16, 8);
    let mesh = GpuMesh::upload(&device, &data).expect("upload must succeed");

    // A stride that disagrees with the vertex type reads every vertex from the wrong
    // offset, which renders as noise rather than failing.
    assert_eq!(
        mesh.vertex_view().StrideInBytes as usize,
        size_of::<MeshVertex>()
    );
    assert_eq!(
        mesh.vertex_view().SizeInBytes as usize,
        std::mem::size_of_val(data.vertices())
    );
    assert_eq!(
        mesh.index_view().SizeInBytes as usize,
        std::mem::size_of_val(data.indices())
    );
    assert_ne!(mesh.vertex_view().BufferLocation, 0);
    assert_ne!(mesh.index_view().BufferLocation, 0);
    assert_eq!(mesh.index_count() as usize, data.indices().len());
    assert_eq!(mesh.triangle_count() as usize, data.triangle_count());
}

#[test]
fn a_meshs_bounds_survive_the_upload() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_meshs_bounds_survive_the_upload") else {
        return;
    };

    let data = MeshData::cube(3.0);
    let mesh = GpuMesh::upload(&device, &data).expect("upload must succeed");
    assert_eq!(mesh.bounds(), data.bounds());
    // Culling reads this and nothing else, so an upload that dropped it would silently
    // cull the mesh at the origin.
    assert!(mesh.bounds().radius > 0.0);
}

#[test]
fn a_batch_of_meshes_uploads_correctly() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_batch_of_meshes_uploads_correctly") else {
        return;
    };

    let cube = MeshData::cube(1.0);
    let plane = MeshData::plane(10.0);
    let sphere = MeshData::uv_sphere(0.5, 12, 6);
    let sources = [&cube, &plane, &sphere];

    let meshes = GpuMesh::upload_many(&device, &sources).expect("batch upload must succeed");
    assert_eq!(meshes.len(), 3);

    // Every mesh in the batch must be complete, not just the last one queued: a staging
    // buffer released before the copy executed would leave one of these full of garbage.
    for (mesh, data) in meshes.iter().zip(sources) {
        let indices: Vec<Index> = cast_back(
            &read_back(
                &device,
                mesh.index_buffer(),
                std::mem::size_of_val(data.indices()),
            ),
            data.indices().len(),
        );
        assert_eq!(indices, data.indices(), "a batched mesh lost its indices");
        assert_eq!(mesh.bounds(), data.bounds());
    }
}

#[test]
fn a_mesh_can_be_dropped_and_another_uploaded_in_its_place() {
    let _guard = gpu_lock();
    let Some(device) = device_or_skip("a_mesh_can_be_dropped_and_another_uploaded_in_its_place")
    else {
        return;
    };

    // Three full create/use/destroy cycles: the staging uploader waits for its own copies,
    // so dropping a mesh immediately afterwards must be safe.
    for size in [1.0_f32, 2.0, 3.0] {
        let data = MeshData::cube(size);
        let mesh = GpuMesh::upload(&device, &data).expect("upload must succeed");
        let vertices: Vec<MeshVertex> = cast_back(
            &read_back(
                &device,
                mesh.vertex_buffer(),
                std::mem::size_of_val(data.vertices()),
            ),
            data.vertices().len(),
        );
        assert_eq!(vertices, data.vertices());
        drop(mesh);
    }
}
