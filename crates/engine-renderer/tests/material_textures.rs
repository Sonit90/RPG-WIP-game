//! The material texture set: four maps, one contiguous descriptor range, frozen order.
//!
//! ## What this protects
//!
//! Milestone 2 replaced the material's single base colour map with a four-texture set bound
//! as one contiguous SRV range at root parameter 3:
//!
//! ```text
//! t0  base colour
//! t1  normal
//! t2  ORM
//! t3  emissive
//! ```
//!
//! That order is an **ABI**. Assets authored against it outlive any one version of the
//! shader, so a reordering is not a refactor — it is every material in the project shading
//! its roughness with an emissive map, silently, with no validation error and no `HRESULT`.
//! The descriptor table is bound by a slot index and a GPU address, and D3D12 validates
//! neither against what the shader declares.
//!
//! ## Why the assertions are about descriptors and not pixels
//!
//! At this milestone the lighting model reads only the base colour map: normal mapping,
//! specular and emissive scaling are later milestones. So "is the normal map bound
//! correctly?" cannot be answered by looking at the picture — there is nothing in the
//! picture that depends on it yet. It is answered here at the two layers that do exist: the
//! renderer's own record of what each material's range holds, and a GPU capture through the
//! shader's material debug views, which sample each slot raw and return it.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_core::math::{Vec2, Vec3};
use engine_platform::{Window, WindowConfig};
use engine_renderer::device::RenderDevice;
use engine_renderer::material::{
    MATERIAL_TEXTURE_COUNT, MaterialDesc, MaterialTextureSet, MaterialTextureSlot,
};
use engine_renderer::mipmap::TextureKind;
use engine_renderer::offscreen::render_material_maps;
use engine_renderer::{Renderer, RendererConfig, RendererError, TextureHandle, TextureImage};

/// The process-wide GPU lock every GPU test in this crate takes.
static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn gpu_available() -> bool {
    match RenderDevice::new(false, false) {
        Ok(_) => true,
        Err(RendererError::NoSuitableAdapter { examined }) => {
            eprintln!("SKIP: no D3D12 adapter on this machine ({examined} examined)");
            false
        }
        Err(error) => panic!("unexpected D3D12 initialization failure: {error}"),
    }
}

/// A renderer on a hidden window, with the debug layer on so a bad descriptor is named
/// rather than merely drawn.
fn renderer_for(title: &str) -> (Window, Renderer) {
    let window = Window::new(&WindowConfig::new(title, 320, 240).with_visible(false))
        .expect("hidden window creation must succeed");
    let config = RendererConfig::new(320, 240)
        .with_debug_layer(true)
        .with_break_on_validation_error(false);
    let renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");
    (window, renderer)
}

/// Uploads a 1x1 texture of the given colour.
fn texel(renderer: &mut Renderer, rgba: [u8; 4], srgb: bool) -> TextureHandle {
    let kind = if srgb {
        TextureKind::Color
    } else {
        TextureKind::Data
    };
    renderer
        .create_texture(&TextureImage {
            width: 1,
            height: 1,
            kind,
            texels: &rgba,
        })
        .expect("a 1x1 texture")
}

#[test]
fn a_material_binds_the_four_maps_it_was_given_in_descriptor_order() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("material texture set");

    // Four distinct textures, so a swapped pair cannot pass by coincidence.
    let base = texel(&mut renderer, [200, 10, 10, 255], true);
    let normal = texel(&mut renderer, [10, 200, 10, 255], false);
    let orm = texel(&mut renderer, [10, 10, 200, 255], false);
    let emissive = texel(&mut renderer, [200, 200, 10, 255], true);

    let material = renderer
        .create_material_with_textures(
            MaterialDesc::default(),
            MaterialTextureSet {
                base_color: Some(base),
                normal: Some(normal),
                orm: Some(orm),
                emissive: Some(emissive),
            },
        )
        .expect("a material with four maps");

    let textures = renderer
        .material_textures(material)
        .expect("the material was just created");
    assert_eq!(textures.base_color, base, "t0 must be the base colour map");
    assert_eq!(textures.normal, normal, "t1 must be the normal map");
    assert_eq!(textures.orm, orm, "t2 must be the ORM map");
    assert_eq!(textures.emissive, emissive, "t3 must be the emissive map");

    // And the same thing read through the positional form the table is written from, which
    // is the part the GPU actually sees.
    assert_eq!(
        textures.in_descriptor_order(),
        [base, normal, orm, emissive],
        "the descriptor order is t0 base, t1 normal, t2 ORM, t3 emissive"
    );
    for slot in MaterialTextureSlot::ALL {
        assert_eq!(
            textures.slot(slot),
            textures.in_descriptor_order()[slot.register() as usize],
            "slot {} must agree with register t{}",
            slot.name(),
            slot.register()
        );
    }
}

#[test]
fn a_material_range_is_four_contiguous_descriptors() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("material texture set");

    let before = renderer.descriptor_stats().used;
    let first = renderer
        .create_material(MaterialDesc::default())
        .expect("a material");
    let after_one = renderer.descriptor_stats().used;
    let second = renderer
        .create_material(MaterialDesc::colored(0.0, 1.0, 0.0))
        .expect("a second material");
    let after_two = renderer.descriptor_stats().used;

    assert_eq!(
        after_one - before,
        MATERIAL_TEXTURE_COUNT,
        "one material must cost exactly {MATERIAL_TEXTURE_COUNT} descriptors"
    );
    assert_eq!(
        after_two - after_one,
        MATERIAL_TEXTURE_COUNT,
        "every material costs the same, whether or not it has four distinct maps"
    );

    // Contiguity is the whole reason the range is allocated in one call: a root descriptor
    // table addresses `count` consecutive slots from its base, so a set scattered across the
    // heap would read three unrelated resources for t1..t3.
    let first_base = renderer
        .material_descriptor_range(first)
        .expect("the first material has a range");
    let second_base = renderer
        .material_descriptor_range(second)
        .expect("the second material has a range");
    assert_eq!(
        second_base,
        first_base + MATERIAL_TEXTURE_COUNT,
        "two materials created in a row must take adjacent four-slot runs"
    );
}

#[test]
fn missing_maps_resolve_to_the_shared_fallbacks_rather_than_to_nothing() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("material texture set");

    let base = texel(&mut renderer, [200, 10, 10, 255], true);
    let material = renderer
        .create_material_with_textures(
            MaterialDesc::default(),
            MaterialTextureSet {
                base_color: Some(base),
                ..MaterialTextureSet::default()
            },
        )
        .expect("a material with only a base colour map");

    let textures = renderer
        .material_textures(material)
        .expect("the material was just created");
    let fallbacks = renderer.fallback_textures();

    assert_eq!(textures.base_color, base, "the provided map must survive");
    assert_eq!(
        textures.normal, fallbacks.normal,
        "a missing normal map must resolve to the shared flat normal"
    );
    assert_eq!(
        textures.orm, fallbacks.orm,
        "a missing ORM map must resolve to the shared neutral ORM"
    );
    assert_eq!(
        textures.emissive, fallbacks.emissive,
        "a missing emissive map must resolve to the shared black texel"
    );

    // Shared, not per material: a second material with the same gaps must point at the same
    // textures rather than uploading its own copies.
    let other = renderer
        .create_material(MaterialDesc::colored(0.0, 0.0, 1.0))
        .expect("a second material");
    let other_textures = renderer
        .material_textures(other)
        .expect("the material was just created");
    assert_eq!(
        other_textures.normal, textures.normal,
        "fallback textures are shared across materials, not duplicated per material"
    );
    assert_eq!(other_textures.orm, textures.orm);
    assert_eq!(other_textures.emissive, textures.emissive);
}

#[test]
fn a_material_naming_a_texture_that_does_not_exist_is_rejected_by_slot() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("material texture set");

    let missing = TextureHandle::from_index(9999);
    let error = renderer
        .create_material_with_textures(
            MaterialDesc::default(),
            MaterialTextureSet {
                orm: Some(missing),
                ..MaterialTextureSet::default()
            },
        )
        .expect_err("a handle naming nothing must not become a descriptor");
    let message = error.to_string();
    assert!(
        message.contains("orm"),
        "the error must name the slot that is wrong, got: {message}"
    );
}

// ---------------------------------------------------------------------------------------
// The fallback texels, asserted at the definition rather than through the renderer.
//
// No device is needed: these are the numbers the window path and the offscreen path both
// build their textures from, so pinning them here pins both.

#[test]
fn the_flat_normal_fallback_is_the_tangent_space_identity() {
    // (0.5, 0.5, 1.0) unpacks through the usual `2x - 1` to (0, 0, 1) — +Z in tangent space,
    // which is the interpolated vertex normal unchanged. Anything else tilts every surface
    // that has no normal map.
    assert_eq!(
        MaterialTextureSlot::Normal.fallback_texel(),
        [128, 128, 255, 255],
        "the flat normal must be (0.5, 0.5, 1.0)"
    );
    assert!(
        !MaterialTextureSlot::Normal.is_srgb(),
        "a normal map is data, not colour: linearizing it on read is the classic wrong import"
    );
}

#[test]
fn the_orm_fallback_is_the_multiplicative_identity() {
    // glTF's metallic-roughness texture *scales* the material's factors, so white passes
    // `roughness` and `metallic` through untouched and an occlusion of 1 is "not occluded".
    // Baking a default roughness into the texel instead would apply that default a second
    // time for every material that overrides it.
    assert_eq!(
        MaterialTextureSlot::Orm.fallback_texel(),
        [255, 255, 255, 255],
        "the neutral ORM is AO 1, roughness scale 1, metallic scale 1"
    );
    assert!(
        !MaterialTextureSlot::Orm.is_srgb(),
        "packed occlusion/roughness/metallic is data, not colour"
    );
}

#[test]
fn the_emissive_fallback_is_the_multiplicative_identity_and_a_material_still_emits_nothing() {
    // White, not black — and the reason is worth stating, because black looks obviously
    // right until it is wrong.
    //
    // glTF multiplies `emissiveTexture` by `emissiveFactor`, and Milestone 3's shading does
    // the same. A black fallback would therefore mean a material with an emissive *colour*
    // and no emissive *map* emits nothing at all, which silently deletes the emissive
    // constant for every material that has ever set one. White passes the factor through, and
    // "no emission" comes from the factor being zero — which is glTF's default and this
    // renderer's, so an asset with no emissive still emits nothing.
    assert_eq!(
        MaterialTextureSlot::Emissive.fallback_texel(),
        [255, 255, 255, 255],
        "the map scales the emissive factor, so the identity is white"
    );
    assert_eq!(
        MaterialDesc::default().emissive,
        engine_core::math::Vec3::ZERO,
        "which only emits nothing because the default factor is zero"
    );
    assert!(
        MaterialTextureSlot::Emissive.is_srgb(),
        "emissive is colour"
    );
}

#[test]
fn the_base_colour_fallback_is_white_so_it_multiplies_to_nothing() {
    assert_eq!(
        MaterialTextureSlot::BaseColor.fallback_texel(),
        [255, 255, 255, 255],
        "the map multiplies base_color, so the identity is white"
    );
    assert!(MaterialTextureSlot::BaseColor.is_srgb());
}

#[test]
fn the_slots_cover_every_register_of_the_range_exactly_once() {
    let registers: Vec<u32> = MaterialTextureSlot::ALL
        .iter()
        .map(|slot| slot.register())
        .collect();
    assert_eq!(
        registers,
        vec![0, 1, 2, 3],
        "the four slots are t0..t3 in declaration order — this is the frozen ABI"
    );
    assert_eq!(
        MaterialTextureSlot::ALL.len() as u32,
        MATERIAL_TEXTURE_COUNT,
        "the range's descriptor count and the slot list are the same fact"
    );
}

// ---------------------------------------------------------------------------------------
// The GPU end of it.

/// Renders one surface with four distinguishable maps and returns the centre pixel of the
/// requested debug view.
fn sampled(debug_mode: u32) -> Option<[u8; 4]> {
    // Deliberately not the fallback values: if the range were mis-ordered or short, reading
    // a neighbouring slot would still land on a plausible-looking colour. These four cannot
    // be confused with each other or with the fallbacks.
    let texels = [
        [255, 0, 0, 255],   // t0 base colour: red
        [0, 255, 0, 255],   // t1 normal: green, an impossible normal on purpose
        [0, 0, 255, 255],   // t2 ORM: blue
        [255, 255, 0, 255], // t3 emissive: yellow
    ];
    let image = render_material_maps(32, 32, texels, debug_mode).ok()?;
    image.pixel(16, 16)
}

#[test]
fn each_debug_view_samples_the_descriptor_its_register_names() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let normal = sampled(5).expect("the normal view renders once the device exists");
    let orm = sampled(6).expect("the ORM view renders once the device exists");
    let emissive = sampled(7).expect("the emissive view renders once the device exists");

    // The maps at t1 and t2 are linear, so what comes back is the texel unchanged. The
    // channel that dominates is the assertion: green at t1, blue at t2. A swapped pair
    // would fail both.
    assert!(
        normal[1] > 200 && normal[0] < 60 && normal[2] < 60,
        "t1 must sample the normal map, got {normal:?}"
    );
    assert!(
        orm[2] > 200 && orm[0] < 60 && orm[1] < 60,
        "t2 must sample the ORM map, got {orm:?}"
    );
    // t3 is sRGB, so 255 stays 255 and 0 stays 0 whatever the transfer function does.
    assert!(
        emissive[0] > 200 && emissive[1] > 200 && emissive[2] < 60,
        "t3 must sample the emissive map, got {emissive:?}"
    );
}

#[test]
fn the_base_colour_view_still_shows_the_base_colour_map() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let base = sampled(4).expect("the base colour view renders once the device exists");
    assert!(
        base[0] > 200 && base[1] < 60 && base[2] < 60,
        "t0 must still be the base colour map, got {base:?}"
    );
}

// ---------------------------------------------------------------------------------------
// The import path.

#[test]
fn create_model_carries_every_map_the_asset_names_instead_of_only_the_base_colour() {
    // The regression this exists for: `create_model` used to read `base_color_texture` and
    // drop `normal_texture` and `orm_texture` on the floor, and there was no emissive slot
    // at all. A model authored with four maps then rendered with one, silently.
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("model material maps");

    // Four distinct 1x1 textures, in an order that is deliberately *not* the descriptor
    // order: the asset's slot names decide where each one lands, not its position in the
    // array. A renderer that shuffled them positionally would pass a same-order test.
    let asset_texture = |name: &str, format, rgba: [u8; 4]| {
        engine_assets::TextureAsset::new(name, 1, 1, format, rgba.to_vec())
            .expect("a 1x1 texture asset")
    };
    let textures = vec![
        asset_texture(
            "orm",
            engine_assets::TextureFormat::Rgba8Unorm,
            [10, 10, 200, 255],
        ),
        asset_texture(
            "base",
            engine_assets::TextureFormat::Rgba8Srgb,
            [200, 10, 10, 255],
        ),
        asset_texture(
            "emissive",
            engine_assets::TextureFormat::Rgba8Srgb,
            [200, 200, 10, 255],
        ),
        asset_texture(
            "normal",
            engine_assets::TextureFormat::Rgba8Unorm,
            [10, 200, 10, 255],
        ),
    ];
    let material = engine_assets::MaterialAsset::colored("four_maps", 1.0, 1.0, 1.0)
        .with_base_color_texture(1)
        .with_normal_texture(3)
        .with_orm_texture(0)
        .with_emissive_texture(2);

    // A single triangle: this test is about materials, and geometry that validates is the
    // cheapest geometry there is.
    let geometry = engine_assets::mesh::Geometry::Static(vec![
        engine_assets::mesh::MeshVertex::new(Vec3::ZERO, Vec3::Z, Vec2::ZERO),
        engine_assets::mesh::MeshVertex::new(Vec3::X, Vec3::Z, Vec2::new(1.0, 0.0)),
        engine_assets::mesh::MeshVertex::new(Vec3::Y, Vec3::Z, Vec2::new(0.0, 1.0)),
    ]);
    let submesh = engine_assets::mesh::SubMesh::new("tri", geometry, vec![0, 1, 2], 0);
    let model =
        engine_assets::ModelAsset::rigid("four_maps", vec![submesh], vec![material], textures);

    let before = renderer.descriptor_stats().used;
    let binding = renderer.create_model(&model).expect("the model uploads");
    let after = renderer.descriptor_stats().used;

    let part = &binding.lods[0].parts[0];
    let bound = renderer
        .material_textures(part.material)
        .expect("the material was just created");
    let fallbacks = renderer.fallback_textures();

    // The point of the test: none of the four is the fallback, so none of them was dropped.
    for (slot, handle) in [
        (MaterialTextureSlot::BaseColor, bound.base_color),
        (MaterialTextureSlot::Normal, bound.normal),
        (MaterialTextureSlot::Orm, bound.orm),
        (MaterialTextureSlot::Emissive, bound.emissive),
    ] {
        assert_ne!(
            handle,
            fallbacks.slot(slot),
            "the {} map came from the asset and must not have fallen back",
            slot.name()
        );
    }

    // And each landed in the slot its *name* names, not its position in the texture array.
    // The asset's array order is ORM, base, emissive, normal.
    assert_eq!(bound.orm.index(), bound.base_color.index() - 1);
    assert_eq!(bound.emissive.index(), bound.base_color.index() + 1);
    assert_eq!(bound.normal.index(), bound.base_color.index() + 2);

    // Four textures at one descriptor each, plus one material at four.
    assert_eq!(
        after - before,
        4 + MATERIAL_TEXTURE_COUNT,
        "a four-texture, one-material model costs four texture SRVs and one four-slot table"
    );
}

#[test]
fn a_model_material_with_no_maps_still_binds_four_valid_descriptors() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("model without maps");

    let geometry = engine_assets::mesh::Geometry::Static(vec![
        engine_assets::mesh::MeshVertex::new(Vec3::ZERO, Vec3::Z, Vec2::ZERO),
        engine_assets::mesh::MeshVertex::new(Vec3::X, Vec3::Z, Vec2::new(1.0, 0.0)),
        engine_assets::mesh::MeshVertex::new(Vec3::Y, Vec3::Z, Vec2::new(0.0, 1.0)),
    ]);
    let submesh = engine_assets::mesh::SubMesh::new("tri", geometry, vec![0, 1, 2], 0);
    let model = engine_assets::ModelAsset::rigid(
        "flat",
        vec![submesh],
        vec![engine_assets::MaterialAsset::colored("flat", 0.5, 0.5, 0.5)],
        Vec::new(),
    );

    let binding = renderer.create_model(&model).expect("the model uploads");
    let bound = renderer
        .material_textures(binding.lods[0].parts[0].material)
        .expect("the material was just created");
    assert_eq!(
        bound,
        renderer.fallback_textures(),
        "an untextured material binds the shared fallbacks in every slot, not nothing"
    );
}
