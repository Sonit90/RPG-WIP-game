//! Dynamic meshes: geometry rewritten every frame through the ordinary entity path.
//!
//! Skips without a D3D12 adapter. The debug layer is on, so a write that races a frame in
//! flight or a view pointing at the wrong copy would be reported rather than silently drawn;
//! the pure rotation rule is unit-tested in `dynamic_mesh.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::{Sphere, Vec2, Vec3};
use engine_core::transform::Transform;
use engine_platform::{Window, WindowConfig};
use engine_render_data::{
    Camera, InstanceStyle, Lighting, MeshHandle, Projection, RenderEntityDesc, RenderWorld,
};
use engine_renderer::material::MaterialDesc;
use engine_renderer::mesh::{MeshData, MeshVertex};
use engine_renderer::{Renderer, RendererConfig, RendererError};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, [0.0, 0.0, 0.0, 1.0]).is_ok()
}

/// A flat square of `size` on the XY plane, facing +Z.
fn quad(size: f32) -> (Vec<MeshVertex>, Vec<u32>) {
    let normal = Vec3::Z;
    let vertices = vec![
        MeshVertex::new(Vec3::new(-size, -size, 0.0), normal, Vec2::new(0.0, 1.0)),
        MeshVertex::new(Vec3::new(size, -size, 0.0), normal, Vec2::new(1.0, 1.0)),
        MeshVertex::new(Vec3::new(size, size, 0.0), normal, Vec2::new(1.0, 0.0)),
        MeshVertex::new(Vec3::new(-size, size, 0.0), normal, Vec2::new(0.0, 0.0)),
    ];
    (vertices, vec![0, 1, 2, 0, 2, 3])
}

#[test]
fn a_dynamic_mesh_is_rewritten_every_frame_and_validated() {
    let _gpu = gpu_lock();
    if !adapter_available() {
        return;
    }
    let window = Window::new(&WindowConfig::new("dynamic mesh", 320, 240).with_visible(false))
        .expect("hidden window");
    let config = RendererConfig::new(320, 240)
        .with_debug_layer(true)
        .with_break_on_validation_error(false);
    let mut renderer = Renderer::new(window.raw_handle(), &config).expect("renderer");

    let handle = renderer.create_dynamic_mesh(4, 6).expect("dynamic mesh");
    let static_mesh = renderer
        .create_mesh(&MeshData::new(quad(1.0).0, quad(1.0).1).expect("mesh data"))
        .expect("static mesh");
    let material = renderer
        .create_material(MaterialDesc::colored(1.0, 0.0, 0.0).with_opacity(0.5))
        .expect("material");

    // Errors: wrong kind of handle, and over capacity. Neither may disturb the mesh.
    let (vertices, indices) = quad(1.0);
    let bounds = Sphere::new(Vec3::ZERO, 2.0);
    assert!(matches!(
        renderer.write_dynamic_mesh(static_mesh, &vertices, &indices, bounds),
        Err(RendererError::InvalidConfig { .. })
    ));
    let too_many = vec![vertices[0]; 5];
    assert!(matches!(
        renderer.write_dynamic_mesh(handle, &too_many, &indices, bounds),
        Err(RendererError::InvalidConfig { .. })
    ));
    assert!(matches!(
        renderer.write_dynamic_mesh(handle, &vertices, &[0; 7], bounds),
        Err(RendererError::InvalidConfig { .. })
    ));

    let mut world = RenderWorld::new();
    world.spawn(RenderEntityDesc {
        transform: Transform::from_translation(Vec3::ZERO),
        mesh: MeshHandle::new(handle.index() as u32, 1),
        material,
        bounds,
        visible: true,
        style: InstanceStyle::NEUTRAL,
        shadow: engine_render_data::ShadowCasting::CASTS_AND_RECEIVES,
    });
    let mut camera = Camera::new(Projection::perspective(320.0 / 240.0)).expect("camera");
    camera
        .look_at(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y)
        .expect("look at");

    // More frames than frames in flight, rewriting each one, then a double write in one
    // frame, then emptied: every phase must record and present cleanly.
    for frame in 0..12 {
        let (vertices, indices) = quad(0.2 + frame as f32 * 0.05);
        renderer
            .write_dynamic_mesh(handle, &vertices, &indices, bounds)
            .expect("write");
        if frame == 8 {
            renderer
                .write_dynamic_mesh(handle, &vertices, &indices, bounds)
                .expect("second write in the same frame");
        }
        if frame >= 10 {
            renderer
                .write_dynamic_mesh(handle, &vertices, &[], bounds)
                .expect("empty write");
        }
        renderer
            .render_world(&world, &camera, Lighting::default(), [0.0, 0.0, 0.0, 1.0])
            .expect("frame");
        renderer.present().expect("present");
    }
    renderer.wait_for_idle().expect("idle");
}
