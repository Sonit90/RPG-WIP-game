//! Whether the pipelines really share one root signature object.
//!
//! They are all created from the same serialized bytecode, and the renderer's binding
//! tracker treats a root signature change as invalidating every root argument — which is
//! what D3D12 does. If two pipelines that are meant to share a signature turn out to hold
//! *different* objects, then binding constants and then binding the other pipeline discards
//! them, and the symptom is a shader reading an unbound root descriptor.
//!
//! This test exists because that question decided the order of two lines in the shadow pass.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use windows::core::Interface;

#[test]
fn every_mesh_pipeline_shares_one_root_signature_object() {
    let Ok(context) = engine_renderer::device::RenderDevice::new(false, false) else {
        eprintln!("skipping: no D3D12 device");
        return;
    };
    let device = context.device();
    let mesh = engine_renderer::mesh_pipeline::MeshPipeline::new(device).expect("mesh pipeline");
    let instanced = engine_renderer::mesh_pipeline::InstancedMeshPipeline::new(device)
        .expect("instanced pipeline");
    let shadow = engine_renderer::shadow::pipeline::ShadowPipelines::new(
        device,
        engine_renderer::shadow::ShadowBias::default(),
    )
    .expect("shadow pipelines");

    let mesh_id = mesh.root_signature().as_raw() as usize;
    let instanced_id = instanced.root_signature().as_raw() as usize;
    let shadow_id = shadow.root_signature().as_raw() as usize;
    println!("mesh {mesh_id:x} instanced {instanced_id:x} shadow {shadow_id:x}");
    assert_eq!(
        mesh_id, instanced_id,
        "the mesh and instanced pipelines must share one root signature object"
    );
    assert_eq!(
        mesh_id, shadow_id,
        "the shadow pipelines must share the mesh root signature object"
    );
}
