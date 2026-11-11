//! Pins the root-parameter ↔ shader-register mapping.
//!
//! ## What this protects
//!
//! The mesh root signature has three constant-buffer views. Their **root parameter slots**
//! are ordered by how often the argument changes — frame, then material, then object —
//! because the most volatile argument should be the cheapest thing to rewrite. Their
//! **shader registers** are ordered by declaration in `shaders/mesh.hlsl`:
//!
//! ```text
//! cbuffer Frame    : register(b0)
//! cbuffer Object   : register(b1)
//! cbuffer Material : register(b2)
//! ```
//!
//! The two orderings disagree. Root slot 1 binds `b2`, root slot 2 binds `b1`, and the
//! parameter array is therefore built as `[b0, b2, b1]`.
//!
//! Getting that wrong is a silent failure. `SetGraphicsRootConstantBufferView` takes a slot
//! index and a GPU address and validates neither against what the shader expects, so binding
//! the material where the model matrix belongs produces no validation-layer message, no
//! `HRESULT`, and a correctly recorded draw call. What it produces on screen is every object
//! collapsed to the origin and scaled by a colour — an image that looks like a transform bug
//! and sends you looking in entirely the wrong place. That happened once already; this test
//! exists so it cannot happen twice.
//!
//! ## Why this and not shader reflection
//!
//! Reflection would read the registers out of the DXIL and compare them. That is the right
//! answer for a renderer with dozens of shaders and generated root signatures. This one has
//! three constant buffers in one shader, and reflection would be a dependency plus a parsing
//! layer to protect nine lines. An explicit assertion is cheaper, reads as documentation,
//! and fails in the same edit that breaks it.
//!
//! No GPU is needed: the test parses the HLSL source and inspects the parameter table.
//! Neither requires a device.

use engine_renderer::material::{MATERIAL_TEXTURE_COUNT, MaterialTextureSlot};
use engine_renderer::mesh_pipeline::{MATERIAL_TEXTURE_REGISTERS, SCENE_TEXTURE_REGISTERS};
use engine_renderer::scene::{
    ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_OBJECT_CBV, ROOT_PARAMETER_REGISTERS,
};

/// The sources the root signature describes, read at compile time so the test cannot drift
/// away from the files the build compiles.
///
/// Three of them, because the declarations moved when shadows arrived: the three mesh
/// pipelines now share one frame buffer and one material buffer out of headers, and only the
/// per-object buffer is still written per pipeline. The registers are the same registers.
const SOURCES: [(&str, &str); 3] = [
    ("shaders/mesh.hlsl", include_str!("../shaders/mesh.hlsl")),
    (
        "shaders/scene_frame.hlsli",
        include_str!("../shaders/scene_frame.hlsli"),
    ),
    (
        "shaders/material.hlsli",
        include_str!("../shaders/material.hlsli"),
    ),
];

/// The register a named `cbuffer` is declared at.
///
/// Deliberately a crude parse: it looks for `cbuffer <name>` and then the next
/// `register(bN)`. Anything more would be a shader parser, and the point is to read the one
/// number the root signature has to agree with.
fn declared_register(name: &str) -> u32 {
    let declaration = format!("cbuffer {name}");
    let Some((source, start)) = SOURCES
        .iter()
        .find_map(|(_, text)| text.find(&declaration).map(|start| (*text, start)))
    else {
        panic!("no shader source declares `{declaration}`");
    };
    let tail = &source[start..];
    let Some(marker) = tail.find("register(b") else {
        panic!("`{declaration}` is not followed by a register assignment");
    };
    let digits: String = tail[marker + "register(b".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse() {
        Ok(register) => register,
        Err(error) => panic!("`{declaration}` has an unparsable register {digits:?}: {error}"),
    }
}

/// The `t` register a named `Texture2D` (or `ByteAddressBuffer`) is declared at.
///
/// The same crude parse as [`declared_register`], for the resource declarations rather than
/// the constant buffers: find the name, then the next `register(tN)`. The point is to read
/// the one number the root signature has to agree with, not to parse HLSL.
fn declared_texture_register(name: &str) -> u32 {
    let Some((source, start)) = SOURCES
        .iter()
        .find_map(|(_, text)| text.find(name).map(|start| (*text, start)))
    else {
        panic!("no shader source declares `{name}`");
    };
    let tail = &source[start..];
    let Some(marker) = tail.find("register(t") else {
        panic!("`{name}` is not followed by a t-register assignment");
    };
    let digits: String = tail[marker + "register(t".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    match digits.parse() {
        Ok(register) => register,
        Err(error) => panic!("`{name}` has an unparsable register {digits:?}: {error}"),
    }
}

/// The HLSL name each material texture slot is declared under.
///
/// Paired with the slot rather than with a register, so the assertion below reads as "the
/// map the ABI calls `Normal` is the shader variable `normal_map`, and it is at `t1`".
const MATERIAL_MAP_NAMES: [(MaterialTextureSlot, &str); MATERIAL_TEXTURE_COUNT as usize] = [
    (MaterialTextureSlot::BaseColor, "Texture2D base_color_map"),
    (MaterialTextureSlot::Normal, "Texture2D normal_map"),
    (MaterialTextureSlot::Orm, "Texture2D orm_map"),
    (MaterialTextureSlot::Emissive, "Texture2D emissive_map"),
];

#[test]
fn the_material_texture_set_is_four_registers_starting_at_t0() {
    // The frozen ABI. Assets authored against this order outlive any one version of the
    // shader, so a reordering is not a refactor — it is every material in the project
    // shading its roughness with an emissive map, with no validation error anywhere.
    assert_eq!(
        MATERIAL_TEXTURE_REGISTERS,
        [0, 1, 2, 3],
        "the material texture set is t0 base colour, t1 normal, t2 ORM, t3 emissive"
    );
    assert_eq!(
        MATERIAL_TEXTURE_REGISTERS.len() as u32,
        MATERIAL_TEXTURE_COUNT,
        "the range's descriptor count and its register list are the same fact"
    );
}

#[test]
fn the_material_registers_are_contiguous_so_one_range_can_cover_them() {
    // A descriptor table addresses `count` *consecutive* slots from its base. If the
    // registers were not consecutive the set would need one root parameter per map, four
    // binds per batch, and six more DWORDs of the sixty-four available.
    for window in MATERIAL_TEXTURE_REGISTERS.windows(2) {
        assert_eq!(
            window[1],
            window[0] + 1,
            "the material texture registers must be contiguous, found {MATERIAL_TEXTURE_REGISTERS:?}"
        );
    }
}

#[test]
fn each_material_slot_is_declared_in_the_shader_at_the_register_the_abi_names() {
    for (slot, declaration) in MATERIAL_MAP_NAMES {
        assert_eq!(
            declared_texture_register(declaration),
            slot.register(),
            "`{declaration}` must be at t{}, the register the {} slot owns",
            slot.register(),
            slot.name()
        );
        assert_eq!(
            slot.register(),
            MATERIAL_TEXTURE_REGISTERS[slot.register() as usize],
            "the slot's register and the root signature's range must agree"
        );
    }
}

#[test]
fn no_scene_texture_collides_with_the_material_set() {
    // The material set owns the bottom of the register space, because its size is part of
    // the asset ABI while the scene's own textures are the renderer's business. A scene
    // texture that landed inside t0..t3 would silently shadow one of the maps.
    for register in SCENE_TEXTURE_REGISTERS {
        assert!(
            register >= MATERIAL_TEXTURE_COUNT,
            "scene texture register t{register} is inside the material set t0..t{}",
            MATERIAL_TEXTURE_COUNT - 1
        );
    }
    let mut all: Vec<u32> = MATERIAL_TEXTURE_REGISTERS
        .iter()
        .chain(SCENE_TEXTURE_REGISTERS.iter())
        .copied()
        .collect();
    all.sort_unstable();
    let distinct = all.len();
    all.dedup();
    assert_eq!(
        all.len(),
        distinct,
        "two bindings must never claim the same t register"
    );
}

#[test]
fn the_scene_textures_are_declared_where_the_root_signature_binds_them() {
    // The registers the material set displaced when it grew from one texture to four. Each
    // is checked against the shader rather than against a comment, because a stale comment
    // is exactly what a renumbering leaves behind.
    assert_eq!(
        declared_texture_register("Texture2D<float> shadow_atlas"),
        SCENE_TEXTURE_REGISTERS[0],
        "the shadow atlas must be where the root signature binds it"
    );
    assert_eq!(
        declared_texture_register("Texture2D<float> scene_depth"),
        SCENE_TEXTURE_REGISTERS[1],
        "the depth copy must be where the root signature binds it"
    );
    assert_eq!(
        declared_texture_register("ByteAddressBuffer cluster_light_mask"),
        SCENE_TEXTURE_REGISTERS[2],
        "the clustered light grid must be where the root signature binds it"
    );
    assert_eq!(
        declared_texture_register("Texture2D<float2> contact_shadows"),
        SCENE_TEXTURE_REGISTERS[3],
        "the contact-shadow target must be where the root signature binds it"
    );
}

#[test]
fn the_shader_declares_the_registers_the_mapping_assumes() {
    // If someone renumbers the cbuffers in mesh.hlsl, this is what catches it.
    assert_eq!(declared_register("Frame"), 0, "cbuffer Frame must be b0");
    assert_eq!(declared_register("Object"), 1, "cbuffer Object must be b1");
    assert_eq!(
        declared_register("Material"),
        2,
        "cbuffer Material must be b2"
    );
}

#[test]
fn each_root_slot_binds_the_register_its_constant_buffer_lives_in() {
    // The assertion that matters. Read it as: "root slot ROOT_X binds the register the
    // shader declares cbuffer X at."
    assert_eq!(
        ROOT_PARAMETER_REGISTERS[ROOT_FRAME_CBV as usize],
        declared_register("Frame"),
        "root slot {ROOT_FRAME_CBV} must bind the Frame constant buffer"
    );
    assert_eq!(
        ROOT_PARAMETER_REGISTERS[ROOT_MATERIAL_CBV as usize],
        declared_register("Material"),
        "root slot {ROOT_MATERIAL_CBV} must bind the Material constant buffer"
    );
    assert_eq!(
        ROOT_PARAMETER_REGISTERS[ROOT_OBJECT_CBV as usize],
        declared_register("Object"),
        "root slot {ROOT_OBJECT_CBV} must bind the Object constant buffer"
    );
}

#[test]
fn the_mapping_is_the_by_update_frequency_order_and_not_the_identity() {
    // Stated explicitly so the intent survives. If root slots were ever renumbered to match
    // the registers one-for-one, that is a real decision with a real cost — the object
    // constants, rewritten once per draw, would move to the slot that changes least often —
    // and it should not happen by accident.
    assert_eq!(
        ROOT_PARAMETER_REGISTERS,
        [0, 2, 1],
        "root parameters are ordered frame, material, object — by how often they change, \
         not by shader register"
    );
    assert_ne!(
        ROOT_PARAMETER_REGISTERS,
        [0, 1, 2],
        "an identity mapping would put the per-draw constants in the least volatile slot"
    );
}

#[test]
fn the_slots_are_distinct_and_cover_every_parameter() {
    let slots = [ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_OBJECT_CBV];
    let mut sorted = slots;
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [0, 1, 2],
        "three parameters must occupy slots 0, 1 and 2 exactly once"
    );
    assert_eq!(
        ROOT_PARAMETER_REGISTERS.len(),
        slots.len(),
        "every named slot must have a register in the mapping"
    );

    let mut registers = ROOT_PARAMETER_REGISTERS;
    registers.sort_unstable();
    assert_eq!(
        registers,
        [0, 1, 2],
        "two root parameters must never bind the same register"
    );
}
