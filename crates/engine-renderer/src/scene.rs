//! The 3D scene: what the renderer draws, and the pipeline that draws it.
//!
//! `RenderWorld` is the boundary between the game and the renderer. The game fills it with
//! transforms, mesh handles and material handles; the renderer reads it and records draws.
//! It contains no gameplay concept — a `RenderEntity` is a slot in three arrays, not a
//! character.

use engine_core::math::{Mat4, Sphere, Vec3};
use engine_core::transform::Transform;

use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::{
    D3D_ROOT_SIGNATURE_VERSION_1, D3D12_DEPTH_STENCIL_DESC, D3D12_GRAPHICS_PIPELINE_STATE_DESC,
    D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, D3D12_INPUT_ELEMENT_DESC, D3D12_INPUT_LAYOUT_DESC,
    D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE, D3D12_ROOT_DESCRIPTOR, D3D12_ROOT_PARAMETER,
    D3D12_ROOT_PARAMETER_0, D3D12_ROOT_PARAMETER_TYPE_CBV, D3D12_ROOT_SIGNATURE_DESC,
    D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT, D3D12_SHADER_VISIBILITY_ALL,
    D3D12SerializeRootSignature, ID3D12Device, ID3D12PipelineState, ID3D12RootSignature,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R32G32_FLOAT, DXGI_FORMAT_R32G32B32_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::core::s;

use crate::depth::DEPTH_FORMAT;
use crate::error::RendererError;
use crate::mesh::MeshVertex;
use crate::pipeline::{blend_desc, depth_stencil_desc, rasterizer_desc, wireframe_rasterizer_desc};
use crate::shader::{MESH_PS, MESH_VS, bytecode};
use crate::swapchain::BACK_BUFFER_FORMAT;

/// Root parameter slots, matching `shaders/mesh.hlsl`.
///
/// Ordered by how often they change: the frame constants are bound once, the material once
/// per material run, the object once per draw. A root signature is re-bound wholesale when
/// it changes, but individual root arguments are not, so putting the most volatile last
/// keeps the cheapest thing changing most often.
pub const ROOT_FRAME_CBV: u32 = 0;
/// Material constants, `b2` in the shader.
pub const ROOT_MATERIAL_CBV: u32 = 1;
/// Per-object constants, `b1` in the shader.
pub const ROOT_OBJECT_CBV: u32 = 2;

/// One directional light and an ambient term.
///
/// Directional because a sun is the only light a third-person outdoor scene cannot do
/// without, and because it needs no attenuation, no position and no per-object light list —
/// point and spot lights need all three, and §46 rules out clustered shading, which is what
/// makes many of them affordable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lighting {
    /// The direction the light *travels*, normalized on use.
    ///
    /// Authored the way it is described — "the sun points down and to the left" — and
    /// negated once, in the shader, rather than at every call site.
    pub direction: Vec3,
    /// Linear light colour.
    pub color: Vec3,
    /// Light strength. Not in physical units; there is no exposure control to calibrate it
    /// against, so 1.0 means "a fully lit surface shows its base colour".
    pub intensity: f32,
    /// Linear ambient colour, standing in for everything that is not the sun.
    pub ambient_color: Vec3,
    /// Ambient strength. This is what stops an unlit face from being pure black, which is
    /// what a single light with no bounce would otherwise produce.
    pub ambient_intensity: f32,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            // Down, forward and slightly to the side: a face pointing up is fully lit and
            // the two visible side faces of a cube get different amounts, which makes a
            // wrong normal obvious at a glance.
            direction: Vec3::new(-0.4, -1.0, -0.6),
            color: Vec3::ONE,
            intensity: 1.0,
            ambient_color: Vec3::ONE,
            ambient_intensity: 0.15,
        }
    }
}

/// Per-frame constants, matching `cbuffer Frame`.
///
/// `repr(C)`, and padded to whole 16-byte registers for the same reason
/// [`crate::material::MaterialConstants`] is.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameConstants {
    /// `projection * view`, column-major — the layout `Mat4` already has.
    pub view_projection: [f32; 16],
    /// The camera's world position.
    pub camera_position: [f32; 3],
    /// Padding to a whole register.
    pub padding: f32,
    /// The direction the light travels.
    pub light_direction: [f32; 3],
    /// Light strength. Shares a register with the direction.
    pub light_intensity: f32,
    /// Linear light colour.
    pub light_color: [f32; 3],
    /// Ambient strength. Shares a register with the light colour.
    pub ambient_intensity: f32,
    /// Linear ambient colour.
    pub ambient_color: [f32; 3],
    /// Padding to a whole register.
    pub padding_2: f32,
}

impl FrameConstants {
    /// Builds the constants for a camera and a light.
    #[must_use]
    pub fn new(view_projection: Mat4, camera_position: Vec3, lighting: Lighting) -> Self {
        let direction = lighting.direction.normalize();
        Self {
            view_projection: view_projection.to_cols_array(),
            camera_position: [camera_position.x, camera_position.y, camera_position.z],
            padding: 0.0,
            light_direction: [direction.x, direction.y, direction.z],
            light_intensity: lighting.intensity,
            light_color: [lighting.color.x, lighting.color.y, lighting.color.z],
            ambient_intensity: lighting.ambient_intensity,
            ambient_color: [
                lighting.ambient_color.x,
                lighting.ambient_color.y,
                lighting.ambient_color.z,
            ],
            padding_2: 0.0,
        }
    }
}

/// Per-object constants, matching `cbuffer Object`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ObjectConstants {
    /// The local-to-world matrix, column-major.
    pub model: [f32; 16],
}

impl ObjectConstants {
    /// Builds the constants for a world transform.
    #[must_use]
    pub fn new(model: Mat4) -> Self {
        Self {
            model: model.to_cols_array(),
        }
    }
}

/// An index and a generation, packed into eight bytes.
///
/// A stale handle is *detected* rather than dereferenced: the generation in the handle is
/// compared against the generation in the slot, and a freed-and-reused slot fails the
/// comparison instead of silently addressing someone else's object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    index: u32,
    generation: u32,
}

impl Handle {
    /// The slot this handle refers to.
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation this handle was issued with.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Declares a distinct handle type, so one kind cannot be passed where another belongs.
macro_rules! declare_handle {
    ($name:ident, $what:literal) => {
        #[doc = concat!("A handle to ", $what, ".")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(Handle);

        impl $name {
            /// A handle that names nothing.
            ///
            /// Generation 0 never matches a live slot, so this cannot be mistaken for a
            /// real handle. It exists for a world built only to measure culling, where the
            /// bounds matter and the geometry does not; `Renderer::render_world` rejects it.
            pub const INVALID: Self = Self(Handle {
                index: u32::MAX,
                generation: 0,
            });

            /// Wraps a raw handle.
            #[must_use]
            pub(crate) fn new(index: u32, generation: u32) -> Self {
                Self(Handle { index, generation })
            }

            /// The underlying index and generation.
            #[must_use]
            pub fn raw(self) -> Handle {
                self.0
            }

            /// The slot this handle refers to.
            #[must_use]
            pub fn index(self) -> usize {
                self.0.index as usize
            }
        }
    };
}

declare_handle!(MeshHandle, "an uploaded mesh");
declare_handle!(MaterialHandle, "an uploaded material");
declare_handle!(RenderEntity, "one drawable in a `RenderWorld`");

/// What a `RenderWorld` slot holds, in parallel arrays.
///
/// Structure-of-arrays because culling reads `bounds` and `visible` and nothing else:
/// streaming mesh handles and materials it does not look at would waste the cache line it
/// is reading through.
#[derive(Debug, Default, Clone)]
pub struct RenderWorld {
    transforms: Vec<Transform>,
    meshes: Vec<MeshHandle>,
    materials: Vec<MaterialHandle>,
    /// World-space bounding spheres, refreshed whenever a transform changes.
    bounds: Vec<Sphere>,
    visible: Vec<bool>,
    generations: Vec<u32>,
    /// Slots whose entity was despawned, most recent first.
    free_slots: Vec<u32>,
    live: usize,
}

/// What a caller must supply to add something drawable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderEntityDesc {
    /// Where it is.
    pub transform: Transform,
    /// What to draw.
    pub mesh: MeshHandle,
    /// How it looks.
    pub material: MaterialHandle,
    /// The mesh's local-space bounds, which the caller reads from the mesh.
    pub bounds: Sphere,
    /// Whether it should be drawn at all.
    pub visible: bool,
}

impl RenderWorld {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty world with room for `capacity` entities.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            transforms: Vec::with_capacity(capacity),
            meshes: Vec::with_capacity(capacity),
            materials: Vec::with_capacity(capacity),
            bounds: Vec::with_capacity(capacity),
            visible: Vec::with_capacity(capacity),
            generations: Vec::with_capacity(capacity),
            free_slots: Vec::new(),
            live: 0,
        }
    }

    /// Adds an entity, reusing a freed slot when one is available.
    pub fn spawn(&mut self, desc: RenderEntityDesc) -> RenderEntity {
        self.live += 1;
        let world_bounds = world_bounds(desc.bounds, &desc.transform);

        if let Some(index) = self.free_slots.pop() {
            let slot = index as usize;
            self.transforms[slot] = desc.transform;
            self.meshes[slot] = desc.mesh;
            self.materials[slot] = desc.material;
            self.bounds[slot] = world_bounds;
            self.visible[slot] = desc.visible;
            return RenderEntity::new(index, self.generations[slot]);
        }

        let index = self.transforms.len() as u32;
        self.transforms.push(desc.transform);
        self.meshes.push(desc.mesh);
        self.materials.push(desc.material);
        self.bounds.push(world_bounds);
        self.visible.push(desc.visible);
        // Generation 1, not 0: a default-constructed handle has generation 0 and must not
        // accidentally address a live slot.
        self.generations.push(1);
        RenderEntity::new(index, 1)
    }

    /// Removes an entity. A stale handle is ignored and reported.
    pub fn despawn(&mut self, entity: RenderEntity) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let slot = entity.index();
        // Bumping the generation is what makes every outstanding handle to this slot stale.
        self.generations[slot] = self.generations[slot].wrapping_add(1).max(1);
        self.visible[slot] = false;
        self.free_slots.push(entity.raw().index());
        self.live -= 1;
        true
    }

    /// Whether the handle still refers to a live entity.
    #[must_use]
    pub fn contains(&self, entity: RenderEntity) -> bool {
        let slot = entity.index();
        slot < self.generations.len()
            && self.generations[slot] == entity.raw().generation()
            && !self.free_slots.contains(&entity.raw().index())
    }

    /// Moves an entity, refreshing its world-space bounds.
    ///
    /// Returns `false` for a stale handle rather than panicking: a game that despawns and
    /// then updates in the same frame is doing something ordinary.
    pub fn set_transform(
        &mut self,
        entity: RenderEntity,
        transform: Transform,
        local: Sphere,
    ) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let slot = entity.index();
        self.bounds[slot] = world_bounds(local, &transform);
        self.transforms[slot] = transform;
        true
    }

    /// Shows or hides an entity.
    pub fn set_visible(&mut self, entity: RenderEntity, visible: bool) -> bool {
        if !self.contains(entity) {
            return false;
        }
        self.visible[entity.index()] = visible;
        true
    }

    /// Live entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether the world holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Slots ever allocated, live or free. The length of the parallel arrays.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.transforms.len()
    }

    /// The transforms, indexed by slot.
    #[must_use]
    pub fn transforms(&self) -> &[Transform] {
        &self.transforms
    }

    /// The mesh handles, indexed by slot.
    #[must_use]
    pub fn meshes(&self) -> &[MeshHandle] {
        &self.meshes
    }

    /// The material handles, indexed by slot.
    #[must_use]
    pub fn materials(&self) -> &[MaterialHandle] {
        &self.materials
    }

    /// The world-space bounding spheres, indexed by slot.
    #[must_use]
    pub fn bounds(&self) -> &[Sphere] {
        &self.bounds
    }

    /// Per-slot visibility flags.
    #[must_use]
    pub fn visible_flags(&self) -> &[bool] {
        &self.visible
    }

    /// Whether a slot holds something that should be drawn.
    ///
    /// A freed slot keeps its arrays populated — clearing them would be work for data
    /// nobody reads — so it is `visible = false` that makes it invisible.
    #[must_use]
    pub fn is_drawable(&self, slot: usize) -> bool {
        self.visible.get(slot).copied().unwrap_or(false)
    }

    /// Removes every entity, keeping the allocated capacity.
    pub fn clear(&mut self) {
        self.transforms.clear();
        self.meshes.clear();
        self.materials.clear();
        self.bounds.clear();
        self.visible.clear();
        self.generations.clear();
        self.free_slots.clear();
        self.live = 0;
    }
}

/// Places a local-space bounding sphere in world space.
///
/// The radius is scaled by the *largest* axis: a non-uniformly scaled sphere is an
/// ellipsoid, and keeping it a sphere means over-estimating. Over-estimating draws something
/// that could have been culled; under-estimating culls something that is on screen.
#[must_use]
pub fn world_bounds(local: Sphere, transform: &Transform) -> Sphere {
    Sphere::new(
        transform.transform_point(local.center),
        local.radius * transform.max_scale(),
    )
}

/// Root signature and pipeline state for [`MeshVertex`] geometry.
pub struct MeshPipeline {
    root_signature: ID3D12RootSignature,
    pipeline_state: ID3D12PipelineState,
}

impl MeshPipeline {
    /// Creates the solid-fill root signature and pipeline state from the embedded DXIL.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn new(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, rasterizer_desc())
    }

    /// Creates the same pipeline with wireframe fill.
    ///
    /// A separate pipeline state rather than a dynamic state: D3D12 bakes the fill mode
    /// into the PSO, so a wireframe toggle is a choice between two objects created at
    /// startup, not a per-frame state change.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Api`] if a D3D12 call fails.
    pub fn wireframe(device: &ID3D12Device) -> Result<Self, RendererError> {
        Self::with_rasterizer(device, wireframe_rasterizer_desc())
    }

    /// Creates the pipeline with an explicit rasterizer state.
    fn with_rasterizer(
        device: &ID3D12Device,
        rasterizer: windows::Win32::Graphics::Direct3D12::D3D12_RASTERIZER_DESC,
    ) -> Result<Self, RendererError> {
        let root_signature = create_root_signature(device)?;

        let input_elements = [
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("POSITION"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 0,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("NORMAL"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32B32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 12,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
            D3D12_INPUT_ELEMENT_DESC {
                SemanticName: s!("TEXCOORD"),
                SemanticIndex: 0,
                Format: DXGI_FORMAT_R32G32_FLOAT,
                InputSlot: 0,
                AlignedByteOffset: 24,
                InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA,
                InstanceDataStepRate: 0,
            },
        ];

        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: input_elements.as_ptr(),
                NumElements: input_elements.len() as u32,
            },
            // SAFETY: copies the interface pointer without changing its refcount; the copy
            // lives only for this call and the real reference is owned here.
            pRootSignature: unsafe { std::mem::transmute_copy(&root_signature) },
            VS: bytecode(MESH_VS),
            PS: bytecode(MESH_PS),
            RasterizerState: rasterizer,
            BlendState: blend_desc(),
            DepthStencilState: depth_stencil_desc(),
            DSVFormat: DEPTH_FORMAT,
            SampleMask: u32::MAX,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        desc.RTVFormats[0] = BACK_BUFFER_FORMAT;
        debug_assert_eq!(
            size_of::<MeshVertex>(),
            32,
            "the input layout offsets above describe a 32-byte vertex"
        );
        debug_assert_ne!(
            desc.DepthStencilState,
            D3D12_DEPTH_STENCIL_DESC::default(),
            "the scene pipeline must test depth"
        );

        // SAFETY: FFI call with a fully initialized descriptor whose referenced bytecode
        // and input-element array outlive the call.
        let pipeline_state =
            unsafe { device.CreateGraphicsPipelineState(&desc) }.map_err(|error| {
                RendererError::api("ID3D12Device::CreateGraphicsPipelineState", error)
            })?;
        // `desc.pRootSignature` is a borrowed copy in a ManuallyDrop; leaving it alone is
        // what keeps it from releasing a reference it never took.

        Ok(Self {
            root_signature,
            pipeline_state,
        })
    }

    /// The root signature to bind before drawing.
    #[must_use]
    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    /// The pipeline state to bind before drawing.
    #[must_use]
    pub fn pipeline_state(&self) -> &ID3D12PipelineState {
        &self.pipeline_state
    }
}

/// Creates the three-constant-buffer root signature the mesh shader expects.
///
/// Root CBVs rather than descriptor tables: each is a GPU address written straight into the
/// root arguments, with no descriptor to allocate and no table to bind. That is the cheapest
/// binding D3D12 offers, and it is available precisely because nothing here samples a
/// texture yet.
fn create_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, RendererError> {
    let parameter = |shader_register: u32| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR {
                ShaderRegister: shader_register,
                RegisterSpace: 0,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };

    // Slot order is by update frequency, not by register number: frame, material, object.
    let parameters = [parameter(0), parameter(2), parameter(1)];

    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
        ..Default::default()
    };

    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: FFI call. `desc` and the parameter array outlive the call, and both
    // out-parameters are valid.
    let result = unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        return Err(RendererError::api("D3D12SerializeRootSignature", error));
    }

    let Some(blob) = blob else {
        return Err(RendererError::InvalidConfig {
            reason: "root signature serialization produced no blob".to_owned(),
        });
    };

    // SAFETY: FFI calls reading the serialized blob's buffer, which stays alive for the
    // duration of the CreateRootSignature call below.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    // SAFETY: FFI call. `bytes` points into the live blob and node mask 0 targets the
    // single-adapter case.
    unsafe { device.CreateRootSignature(0, bytes) }
        .map_err(|error| RendererError::api("ID3D12Device::CreateRootSignature", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::Quat;

    fn desc(mesh: u32, material: u32) -> RenderEntityDesc {
        RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh: MeshHandle::new(mesh, 1),
            material: MaterialHandle::new(material, 1),
            bounds: Sphere::new(Vec3::ZERO, 1.0),
            visible: true,
        }
    }

    #[test]
    fn a_spawned_entity_is_live_and_drawable() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.contains(entity));
        assert!(world.is_drawable(entity.index()));
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn a_despawned_entity_stops_being_drawn() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.despawn(entity));
        assert!(!world.contains(entity));
        assert!(!world.is_drawable(entity.index()));
        assert!(world.is_empty());
    }

    #[test]
    fn a_stale_handle_is_detected_rather_than_dereferenced() {
        // The whole reason handles carry a generation: without it, the second spawn's slot
        // would answer to the first entity's handle.
        let mut world = RenderWorld::new();
        let first = world.spawn(desc(0, 0));
        world.despawn(first);
        let second = world.spawn(desc(1, 1));

        assert_eq!(first.index(), second.index(), "the slot must be reused");
        assert!(!world.contains(first));
        assert!(world.contains(second));
        assert!(!world.set_visible(first, true));
    }

    #[test]
    fn despawning_twice_is_reported_not_ignored() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.despawn(entity));
        assert!(!world.despawn(entity));
        assert_eq!(world.len(), 0);
    }

    #[test]
    fn slots_are_reused_rather_than_growing() {
        let mut world = RenderWorld::new();
        let handles: Vec<_> = (0..4).map(|i| world.spawn(desc(i, i))).collect();
        for handle in &handles {
            world.despawn(*handle);
        }
        for i in 0..4 {
            world.spawn(desc(i, i));
        }
        assert_eq!(world.slot_count(), 4, "freed slots must be reused");
        assert_eq!(world.len(), 4);
    }

    #[test]
    fn bounds_follow_the_transform_into_world_space() {
        let mut world = RenderWorld::new();
        let local = Sphere::new(Vec3::ZERO, 1.0);
        let entity = world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            bounds: local,
            ..desc(0, 0)
        });
        assert_eq!(world.bounds()[entity.index()].center.x, 10.0);

        let moved = Transform::from_translation(Vec3::new(0.0, 5.0, 0.0))
            .with_scale(Vec3::new(2.0, 3.0, 1.0));
        assert!(world.set_transform(entity, moved, local));

        let bounds = world.bounds()[entity.index()];
        assert_eq!(bounds.center, Vec3::new(0.0, 5.0, 0.0));
        // The largest axis, because a sphere scaled non-uniformly is an ellipsoid and
        // under-estimating would cull something that is on screen.
        assert_eq!(bounds.radius, 3.0);
    }

    #[test]
    fn a_rotated_offset_bound_ends_up_where_the_geometry_is() {
        let local = Sphere::new(Vec3::new(0.0, 0.0, -2.0), 0.5);
        let transform =
            Transform::from_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2));
        let bounds = world_bounds(local, &transform);
        // A quarter turn about +Y sends -Z to -X.
        assert!((bounds.center.x + 2.0).abs() < 1e-4, "{bounds:?}");
        assert!(bounds.center.z.abs() < 1e-4, "{bounds:?}");
    }

    #[test]
    fn hiding_an_entity_keeps_it_alive() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.set_visible(entity, false));
        assert!(world.contains(entity), "hidden is not despawned");
        assert!(!world.is_drawable(entity.index()));
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn clearing_empties_the_world() {
        let mut world = RenderWorld::with_capacity(8);
        let entity = world.spawn(desc(0, 0));
        world.clear();
        assert!(world.is_empty());
        assert!(!world.contains(entity));
        assert_eq!(world.slot_count(), 0);
    }

    #[test]
    fn the_constant_layouts_match_the_shader_registers() {
        assert_eq!(size_of::<FrameConstants>(), 128);
        assert_eq!(std::mem::offset_of!(FrameConstants, camera_position), 64);
        assert_eq!(std::mem::offset_of!(FrameConstants, light_direction), 80);
        assert_eq!(std::mem::offset_of!(FrameConstants, light_color), 96);
        assert_eq!(std::mem::offset_of!(FrameConstants, ambient_color), 112);
        assert_eq!(size_of::<ObjectConstants>(), 64);
    }

    #[test]
    fn frame_constants_keep_the_matrix_column_major() {
        // The shader declares column_major, which is the layout `Mat4` already has, so no
        // transpose happens anywhere. A transpose added "for safety" would break this.
        let view_projection = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let constants = FrameConstants::new(view_projection, Vec3::ZERO, Lighting::default());
        assert_eq!(&constants.view_projection[12..15], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn the_light_direction_is_normalized_once_on_the_cpu() {
        // Normalizing per pixel would be a square root per fragment for a value that is the
        // same across the whole frame.
        let lighting = Lighting {
            direction: Vec3::new(0.0, -10.0, 0.0),
            ..Lighting::default()
        };
        let constants = FrameConstants::new(Mat4::IDENTITY, Vec3::ZERO, lighting);
        assert_eq!(constants.light_direction, [0.0, -1.0, 0.0]);
    }

    #[test]
    fn the_default_light_leaves_unlit_faces_visible() {
        // Ambient exists so a single directional light does not render half of every object
        // as pure black — there is no bounce lighting to fill it in.
        let lighting = Lighting::default();
        assert!(lighting.ambient_intensity > 0.0);
        assert!(lighting.direction.y < 0.0, "the sun points downward");
    }
}
