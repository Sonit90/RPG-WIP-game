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

use engine_renderer::scene::{
    ROOT_FRAME_CBV, ROOT_MATERIAL_CBV, ROOT_OBJECT_CBV, ROOT_PARAMETER_REGISTERS,
};

/// The shader the root signature describes, read at compile time so the test cannot drift
/// away from the file the build compiles.
const MESH_HLSL: &str = include_str!("../shaders/mesh.hlsl");

/// The register a named `cbuffer` is declared at in `MESH_HLSL`.
///
/// Deliberately a crude parse: it looks for `cbuffer <name>` and then the next
/// `register(bN)`. Anything more would be a shader parser, and the point is to read the one
/// number the root signature has to agree with.
fn declared_register(name: &str) -> u32 {
    let declaration = format!("cbuffer {name}");
    let Some(start) = MESH_HLSL.find(&declaration) else {
        panic!("shaders/mesh.hlsl declares no `{declaration}`");
    };
    let tail = &MESH_HLSL[start..];
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
