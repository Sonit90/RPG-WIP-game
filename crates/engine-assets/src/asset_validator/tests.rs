//! What the validator must catch, one test per rule.
//!
//! Every test asserts on the *code* as well as the verdict. A test that only checked
//! `passed()` would pass just as happily if the wrong rule fired, and "the asset was
//! rejected" is not the thing anybody needs to be true — "the asset was rejected for the
//! reason the specification gives" is.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_core::transform::Transform;

use super::*;
use crate::material::MaterialAsset;
use crate::mesh::{MeshVertex, UNSET_TANGENT};
use crate::texture::{TextureAsset, TextureFormat};

/// A submesh of `count` unit-normal triangles spread over a `size`-metre square.
///
/// Independent triangles, three vertices each, with a real UV triangle per face so that
/// `SubMesh::new` generates usable tangents — which is what makes this the shape of a
/// well-formed asset rather than one that trips six rules by accident. Laid out on a grid
/// rather than a line so that a dense mesh stays inside the footprint it claims: density is
/// one of the rules under test.
fn mesh(name: &str, count: usize, material: usize, size: f32) -> SubMesh {
    let columns = (count as f32).sqrt().ceil().max(1.0);
    let cell = size / columns;
    let extent = cell * 0.9;
    let mut vertices = Vec::with_capacity(count * 3);
    let mut indices = Vec::with_capacity(count * 3);
    for triangle in 0..count {
        let column = (triangle % columns as usize) as f32;
        let row = (triangle / columns as usize) as f32;
        let (x, z) = (column * cell, row * cell);
        let base = (triangle * 3) as u32;
        vertices.push(MeshVertex::new(
            Vec3::new(x, 0.0, z),
            Vec3::Y,
            Vec2::new(0.0, 0.0),
        ));
        vertices.push(MeshVertex::new(
            Vec3::new(x + extent, 0.0, z),
            Vec3::Y,
            Vec2::new(1.0, 0.0),
        ));
        vertices.push(MeshVertex::new(
            Vec3::new(x, 0.0, z + extent),
            Vec3::Y,
            Vec2::new(0.0, 1.0),
        ));
        indices.extend([base, base + 1, base + 2]);
    }
    SubMesh::new(name, Geometry::Static(vertices), indices, material)
}

/// A model of one submesh and one material, `count` triangles across `size` metres.
fn model(count: usize, size: f32) -> ModelAsset {
    ModelAsset::rigid(
        "test_asset",
        vec![mesh("body", count, 0, size)],
        vec![MaterialAsset::colored("stone", 0.4, 0.4, 0.4)],
        vec![],
    )
}

/// The spec for `category`, with nothing else stated.
fn spec(category: AssetCategory) -> AssetSpec {
    AssetSpec::new("test/asset.glb", category)
}

/// Validation with the specification's own limits.
fn run(model: &ModelAsset, spec: &AssetSpec) -> AssetValidationResult {
    validate_asset(model, spec, &AssetLimits::default())
}

/// The vertices of a model's first submesh, for a test that needs to break one.
fn vertices_of(model: &mut ModelAsset) -> &mut Vec<MeshVertex> {
    match &mut model.lods[0].submeshes[0].geometry {
        Geometry::Static(vertices) => vertices,
        Geometry::Skinned(_) => panic!("the test models are rigid"),
    }
}

#[test]
fn a_small_prop_inside_every_budget_passes_with_no_errors() {
    let result = run(&model(400, 1.2), &spec(AssetCategory::SmallProp));
    assert!(
        result.passed(),
        "a well-formed small prop must pass: {:#?}",
        result.errors().collect::<Vec<_>>()
    );
    assert_eq!(result.statistics.triangle_count, 400);
    assert_eq!(result.statistics.vertex_count, 1_200);
    assert_eq!(result.statistics.material_count, 1);
    assert!(result.statistics.has_tangents, "tangents are generated");
    assert!(result.statistics.has_uvs);
}

#[test]
fn more_vertices_than_the_category_allows_is_an_error_that_names_both_numbers() {
    // 700 triangles is 2,100 vertices, against the small prop's 2,000.
    let result = run(&model(700, 1.2), &spec(AssetCategory::SmallProp));
    assert!(!result.passed());
    let Some(found) = result.find(ValidationCode::VertexLimit) else {
        panic!("the vertex rule must fire: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Error);
    assert_eq!(found.actual.and_then(Measure::count), Some(2_100));
    assert_eq!(found.limit.and_then(Measure::count), Some(2_000));
}

#[test]
fn more_triangles_than_the_category_allows_is_an_error() {
    // A tiny prop is allowed 300 triangles and 500 vertices; 400 triangles breaks both, and
    // the triangle rule must be one of the two that fires.
    let result = run(&model(400, 0.4), &spec(AssetCategory::TinyProp));
    assert!(!result.passed());
    let Some(found) = result.find(ValidationCode::TriangleLimit) else {
        panic!("the triangle rule must fire: {result:#?}");
    };
    assert_eq!(found.actual.and_then(Measure::count), Some(400));
    assert_eq!(found.limit.and_then(Measure::count), Some(300));
}

#[test]
fn a_hero_asset_marked_as_an_exception_is_allowed_past_the_category_ceiling() {
    // 11,000 triangles is 33,000 vertices: past the hero prop's 30,000, inside the section
    // 25 exception ceiling of 100,000.
    let heavy = model(11_000, 6.0);
    let normal = run(&heavy, &spec(AssetCategory::HeroProp));
    assert!(normal.has(ValidationCode::VertexLimit), "{normal:#?}");

    let excepted = run(&heavy, &spec(AssetCategory::HeroProp).as_exception(true));
    assert!(
        !excepted.has(ValidationCode::VertexLimit),
        "an explicit exception raises the ceiling: {:#?}",
        excepted.errors().collect::<Vec<_>>()
    );
    assert!(
        excepted.passed(),
        "{:#?}",
        excepted.errors().collect::<Vec<_>>()
    );
}

#[test]
fn an_exception_does_not_lift_the_absolute_ceiling() {
    // 34,000 triangles is 102,000 vertices, which is past even the exception limit.
    let result = run(
        &model(34_000, 8.0),
        &spec(AssetCategory::HeroProp).as_exception(true),
    );
    assert!(result.has(ValidationCode::VertexLimit), "{result:#?}");
}

#[test]
fn more_material_slots_than_the_category_allows_is_an_error_and_the_preferred_count_warns() {
    let mut heavy = model(60, 1.0);
    heavy.materials = (0..6)
        .map(|index| MaterialAsset::colored(format!("slot_{index}"), 0.5, 0.5, 0.5))
        .collect();
    let result = run(&heavy, &spec(AssetCategory::MediumProp));
    let Some(found) = result.find(ValidationCode::MaterialLimit) else {
        panic!("six slots is past the medium prop's four: {result:#?}");
    };
    assert_eq!(found.actual.and_then(Measure::count), Some(6));
    assert_eq!(found.limit.and_then(Measure::count), Some(4));

    // Three is legal for a medium prop and past its preferred two, so it warns and passes.
    let mut modest = model(60, 1.0);
    modest.materials = (0..3)
        .map(|index| MaterialAsset::colored(format!("slot_{index}"), 0.5, 0.5, 0.5))
        .collect();
    let result = run(&modest, &spec(AssetCategory::MediumProp));
    assert!(
        result.passed(),
        "{:#?}",
        result.errors().collect::<Vec<_>>()
    );
    assert!(result.has(ValidationCode::MaterialPreferred), "{result:#?}");
}

#[test]
fn a_texture_past_the_hard_limit_is_an_error_and_one_past_the_preferred_size_is_a_warning() {
    let mut oversized = model(60, 1.0);
    // 8192 on the long side, one texel tall: past the 4096 hard limit without allocating a
    // 268-megabyte image to prove it.
    oversized.textures = vec![TextureAsset::generated(
        "base_color",
        8_192,
        1,
        TextureFormat::Rgba8Srgb,
        |_, _| [255; 4],
    )];
    oversized.materials[0].base_color_texture = Some(0);
    let result = run(&oversized, &spec(AssetCategory::SmallProp));
    assert!(!result.passed());
    let Some(found) = result.find(ValidationCode::TextureLimit) else {
        panic!("8192 is past the maximum: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Error);

    let mut preferred = model(60, 1.0);
    preferred.textures = vec![TextureAsset::generated(
        "base_color",
        2_048,
        1,
        TextureFormat::Rgba8Srgb,
        |_, _| [255; 4],
    )];
    preferred.materials[0].base_color_texture = Some(0);
    let result = run(&preferred, &spec(AssetCategory::SmallProp));
    assert!(
        result.passed(),
        "a preferred target is not a hard limit: {:#?}",
        result.errors().collect::<Vec<_>>()
    );
    let Some(found) = result.find(ValidationCode::TexturePreferred) else {
        panic!("2048 is past the small prop's preferred 1024: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Warning);
}

#[test]
fn a_normal_map_delivered_as_srgb_is_rejected_rather_than_shaded_wrongly() {
    let mut model = model(60, 1.0);
    model.textures = vec![TextureAsset::generated(
        "normal",
        64,
        64,
        TextureFormat::Rgba8Srgb,
        |_, _| [128, 128, 255, 255],
    )];
    model.materials[0].normal_texture = Some(0);
    let result = run(&model, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::TextureFormat), "{result:#?}");
    assert!(!result.passed());
}

#[test]
fn geometry_with_no_normals_is_rejected() {
    let mut broken = model(60, 1.0);
    for vertex in vertices_of(&mut broken) {
        vertex.normal = Vec3::ZERO;
    }
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::MissingNormals), "{result:#?}");
    assert!(!result.passed());
}

#[test]
fn a_normal_that_is_not_unit_length_is_rejected_separately_from_a_missing_one() {
    let mut broken = model(60, 1.0);
    for vertex in vertices_of(&mut broken) {
        vertex.normal = Vec3::new(0.0, 4.0, 0.0);
    }
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::InvalidNormals), "{result:#?}");
    assert!(!result.has(ValidationCode::MissingNormals), "{result:#?}");
}

#[test]
fn missing_tangents_warn_alone_and_fail_the_asset_once_a_normal_map_needs_them() {
    let mut plain = model(60, 1.0);
    plain.lods[0].submeshes[0]
        .geometry
        .set_tangents(&vec![UNSET_TANGENT; 180]);
    let result = run(&plain, &spec(AssetCategory::SmallProp));
    let Some(found) = result.find(ValidationCode::MissingTangents) else {
        panic!("an asset with no tangents must be reported: {result:#?}");
    };
    assert_eq!(
        found.severity,
        Severity::Warning,
        "nothing samples a normal map, so this is a cost rather than a break"
    );

    let mut mapped = plain;
    mapped.textures = vec![TextureAsset::generated(
        "normal",
        64,
        64,
        TextureFormat::Rgba8Unorm,
        |_, _| [128, 128, 255, 255],
    )];
    mapped.materials[0].normal_texture = Some(0);
    let result = run(&mapped, &spec(AssetCategory::SmallProp));
    let Some(found) = result.find(ValidationCode::MissingTangents) else {
        panic!("a normal-mapped asset with no tangents must be reported: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Error, "the shader has no basis");
    assert!(!result.passed());
}

#[test]
fn geometry_with_no_texture_coordinates_is_rejected() {
    let mut broken = model(60, 1.0);
    for vertex in vertices_of(&mut broken) {
        vertex.uv = Vec2::ZERO;
    }
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::MissingUv), "{result:#?}");
    assert!(!result.passed());
}

#[test]
fn an_index_past_the_end_of_the_vertices_is_rejected() {
    // The check that matters most: D3D12 does not bounds-check an index buffer.
    let mut broken = model(60, 1.0);
    broken.lods[0].submeshes[0].indices = vec![0, 1, 9_999];
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    let Some(found) = result.find(ValidationCode::InvalidIndices) else {
        panic!("a dangling index must be rejected: {result:#?}");
    };
    assert!(found.message.contains("past the end"), "{found}");
    assert!(!result.passed());
}

#[test]
fn an_index_count_that_is_not_a_triangle_list_is_rejected() {
    let mut broken = model(60, 1.0);
    broken.lods[0].submeshes[0].indices = vec![0, 1];
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::InvalidIndices), "{result:#?}");
}

#[test]
fn zero_area_triangles_are_counted_and_reported_without_failing_the_asset() {
    let mut broken = model(60, 1.0);
    // A repeated index and a triangle whose three corners are one point.
    broken.lods[0].submeshes[0]
        .indices
        .extend([0, 0, 1, 2, 2, 2]);
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    let Some(found) = result.find(ValidationCode::InvalidGeometry) else {
        panic!("degenerate faces must be reported: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Warning);
    assert_eq!(found.actual.and_then(Measure::count), Some(2));
    assert_eq!(result.statistics.degenerate_triangles, 2);
    assert!(result.passed(), "a zero-area face is waste, not a crash");
}

#[test]
fn a_position_that_is_not_a_finite_number_is_rejected() {
    let mut broken = model(60, 1.0);
    vertices_of(&mut broken)[7].position = Vec3::new(f32::NAN, 0.0, 0.0);
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::InvalidGeometry), "{result:#?}");
    assert!(!result.passed());

    let mut infinite = model(60, 1.0);
    vertices_of(&mut infinite)[3].position = Vec3::new(0.0, f32::INFINITY, 0.0);
    let result = run(&infinite, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::InvalidGeometry), "{result:#?}");
}

#[test]
fn a_texture_coordinate_that_is_not_a_finite_number_is_rejected() {
    let mut broken = model(60, 1.0);
    vertices_of(&mut broken)[2].uv = Vec2::new(f32::NAN, 0.0);
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::InvalidUv), "{result:#?}");
}

#[test]
fn a_tangent_with_no_direction_or_a_bad_handedness_is_rejected() {
    let mut broken = model(60, 1.0);
    broken.lods[0].submeshes[0]
        .geometry
        .set_tangents(&vec![Vec4::new(1.0, 0.0, 0.0, 0.5); 180]);
    let result = run(&broken, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::InvalidTangents), "{result:#?}");
}

#[test]
fn a_degenerate_or_absurd_import_scale_is_reported_and_never_corrected() {
    let asset = model(60, 1.0);
    let flattened = spec(AssetCategory::SmallProp)
        .with_transform(Transform::from_scale(Vec3::new(1.0, 0.0, 1.0)));
    let result = run(&asset, &flattened);
    assert!(result.has(ValidationCode::InvalidScale), "{result:#?}");
    assert!(!result.passed());
    assert_eq!(
        flattened.transform.scale,
        Vec3::new(1.0, 0.0, 1.0),
        "validation must not touch the asset it is measuring"
    );

    let nan =
        spec(AssetCategory::SmallProp).with_transform(Transform::from_scale(Vec3::splat(f32::NAN)));
    assert!(run(&asset, &nan).has(ValidationCode::InvalidScale));

    let huge =
        spec(AssetCategory::SmallProp).with_transform(Transform::from_scale(Vec3::splat(50_000.0)));
    assert!(run(&asset, &huge).has(ValidationCode::InvalidScale));
}

#[test]
fn an_asset_a_hundred_times_the_size_of_its_category_is_reported_as_centimetres() {
    // 120 metres of small prop: the shape a centimetre-scale export arrives in.
    let result = run(&model(400, 120.0), &spec(AssetCategory::SmallProp));
    let Some(found) = result.find(ValidationCode::BoundsSuspect) else {
        panic!("an asset the size of a building is not a torch: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Warning);
    let Some(hint) = &found.recommendation else {
        panic!("the finding must say what to do");
    };
    assert!(hint.contains("centimetres"), "{hint}");
}

#[test]
fn a_lod_chain_that_reduces_as_recommended_produces_no_findings_about_lods() {
    let asset = model(1_800, 2.0)
        .with_lod(vec![mesh("lod1", 900, 0, 2.0)], 15.0)
        .with_lod(vec![mesh("lod2", 360, 0, 2.0)], 30.0)
        .with_lod(vec![mesh("lod3", 135, 0, 2.0)], 60.0);
    let result = run(&asset, &spec(AssetCategory::MediumProp));
    assert!(
        result.passed(),
        "{:#?}",
        result.errors().collect::<Vec<_>>()
    );
    assert!(!result.has(ValidationCode::LodMissing), "{result:#?}");
    assert!(!result.has(ValidationCode::LodReduction), "{result:#?}");
    assert!(!result.has(ValidationCode::LodInvalid), "{result:#?}");
    assert_eq!(result.statistics.lod_count, 4);
    assert_eq!(result.statistics.lod_triangles, vec![1_800, 900, 360, 135]);
}

#[test]
fn a_lod_that_is_not_a_reduction_is_an_error_and_a_thin_chain_is_a_warning() {
    let backwards = model(1_000, 3.0).with_lod(vec![mesh("lod1", 2_000, 0, 3.0)], 15.0);
    let result = run(&backwards, &spec(AssetCategory::MediumProp));
    let Some(found) = result.find(ValidationCode::LodInvalid) else {
        panic!("a level that costs more far away is a broken chain: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Error);

    let thin = model(1_000, 3.0);
    let result = run(&thin, &spec(AssetCategory::MediumProp));
    let Some(found) = result.find(ValidationCode::LodMissing) else {
        panic!("one level where four are expected must be reported: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Warning);
    assert!(result.passed(), "a missing LOD is a cost, not a break");

    // A level that barely reduces is inside the chain rules and outside the recommendation.
    let lazy = model(1_000, 3.0).with_lod(vec![mesh("lod1", 950, 0, 3.0)], 15.0);
    let result = run(&lazy, &spec(AssetCategory::MediumProp));
    assert!(result.has(ValidationCode::LodReduction), "{result:#?}");
    assert!(result.passed());
}

#[test]
fn levels_of_detail_must_be_strictly_increasing_and_start_at_zero() {
    let duplicate = model(60, 1.0)
        .with_lod(vec![mesh("lod1", 30, 0, 1.0)], 10.0)
        .with_lod(vec![mesh("lod2", 10, 0, 1.0)], 10.0);
    let result = run(&duplicate, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::LodInvalid), "{result:#?}");

    let mut floating = model(60, 1.0);
    floating.lods[0].min_distance = 5.0;
    assert!(run(&floating, &spec(AssetCategory::SmallProp)).has(ValidationCode::LodInvalid));
}

#[test]
fn too_many_collision_primitives_is_an_error_and_a_complex_shape_is_a_warning() {
    let asset = model(2_000, 3.0);
    let result = run(&asset, &spec(AssetCategory::LargeProp).with_collision(25));
    let Some(found) = result.find(ValidationCode::CollisionLimit) else {
        panic!("25 primitives is past the maximum of 20: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Error);
    assert_eq!(found.limit.and_then(Measure::count), Some(20));

    let result = run(&asset, &spec(AssetCategory::LargeProp).with_collision(14));
    assert!(result.passed());
    let Some(found) = result.find(ValidationCode::CollisionLimit) else {
        panic!("14 primitives is past the preferred ten: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Warning);

    let result = run(&asset, &spec(AssetCategory::LargeProp).with_collision(0));
    assert!(result.has(ValidationCode::CollisionMissing), "{result:#?}");
    assert!(result.passed());
}

#[test]
fn collision_that_was_never_supplied_is_a_note_rather_than_a_finding() {
    let result = run(&model(2_000, 3.0), &spec(AssetCategory::LargeProp));
    let Some(found) = result.find(ValidationCode::CollisionMissing) else {
        panic!("the report must say collision went unchecked: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Info);
    assert!(result.passed());
}

#[test]
fn several_submeshes_are_counted_and_the_draw_call_budget_is_enforced() {
    let many = ModelAsset::rigid(
        "multi",
        (0..3)
            .map(|index| mesh(&format!("part_{index}"), 100, 0, 1.5))
            .collect(),
        vec![MaterialAsset::default()],
        vec![],
    );
    let result = run(&many, &spec(AssetCategory::MediumProp));
    assert_eq!(result.statistics.submesh_count, 3);
    assert_eq!(result.statistics.triangle_count, 300);
    assert!(
        result.passed(),
        "{:#?}",
        result.errors().collect::<Vec<_>>()
    );
    assert!(result.has(ValidationCode::SubmeshPreferred), "{result:#?}");

    let too_many = ModelAsset::rigid(
        "multi",
        (0..6)
            .map(|index| mesh(&format!("part_{index}"), 100, 0, 1.5))
            .collect(),
        vec![MaterialAsset::default()],
        vec![],
    );
    let result = run(&too_many, &spec(AssetCategory::MediumProp));
    let Some(found) = result.find(ValidationCode::SubmeshLimit) else {
        panic!("six draws for one prop is past the budget: {result:#?}");
    };
    assert_eq!(found.severity, Severity::Error);
}

#[test]
fn several_materials_are_counted_and_a_dangling_reference_names_the_submesh() {
    let asset = ModelAsset::rigid(
        "multi_material",
        vec![mesh("body", 100, 0, 1.5), mesh("trim", 40, 1, 1.5)],
        vec![
            MaterialAsset::colored("body", 0.5, 0.4, 0.3),
            MaterialAsset::colored("trim", 0.1, 0.1, 0.1),
        ],
        vec![],
    );
    let result = run(&asset, &spec(AssetCategory::MediumProp));
    assert_eq!(result.statistics.material_count, 2);
    assert!(
        result.passed(),
        "{:#?}",
        result.errors().collect::<Vec<_>>()
    );

    let mut dangling = asset;
    dangling.lods[0].submeshes[1].material = 7;
    let result = run(&dangling, &spec(AssetCategory::MediumProp));
    let Some(found) = result.find(ValidationCode::MaterialReference) else {
        panic!("a submesh naming a material that does not exist: {result:#?}");
    };
    assert!(found.subject.to_string().contains("'trim'"), "{found}");
}

#[test]
fn the_same_asset_is_measured_differently_by_category() {
    // 1,600 triangles and 4,800 vertices: a medium prop, and far too much for a small one.
    let asset = model(1_600, 2.0);
    let medium = run(&asset, &spec(AssetCategory::MediumProp));
    assert!(
        medium.passed(),
        "{:#?}",
        medium.errors().collect::<Vec<_>>()
    );

    let small = run(&asset, &spec(AssetCategory::SmallProp));
    assert!(!small.passed());
    assert!(small.has(ValidationCode::TriangleLimit), "{small:#?}");
    assert!(small.has(ValidationCode::VertexLimit), "{small:#?}");

    // And the foliage budget is stricter again, on an asset the rock budget waves through.
    let rock = run(&asset, &spec(AssetCategory::Rock));
    assert!(rock.passed(), "{:#?}", rock.errors().collect::<Vec<_>>());
    let foliage = run(&asset, &spec(AssetCategory::Foliage));
    assert!(foliage.has(ValidationCode::VertexLimit), "{foliage:#?}");
}

#[test]
fn a_dense_asset_inside_every_count_budget_is_still_reported() {
    // 3,000 triangles on a pebble: inside the rock budget, and far past the density section
    // 26 expects of a runtime asset.
    let result = run(&model(3_000, 0.05), &spec(AssetCategory::Rock));
    assert!(result.has(ValidationCode::GeometryDensity), "{result:#?}");
    assert!(result.passed(), "density is advice, not a refusal");
}

#[test]
fn a_report_is_generated_for_a_failing_asset_and_names_every_code_it_found() {
    let result = run(&model(700, 1.2), &spec(AssetCategory::SmallProp));
    let text = report(&result, &AssetLimits::default());
    assert!(text.contains("FAIL"), "{text}");
    assert!(text.contains("Category: small_prop"), "{text}");
    assert!(text.contains("ASSET_VERTEX_LIMIT"), "{text}");
    assert!(text.contains("2,100"), "{text}");
    assert!(text.contains("Recommendations:"), "{text}");

    let line = summary_line(&result);
    assert!(line.starts_with("FAIL"), "{line}");
}

#[test]
fn an_empty_model_is_rejected_rather_than_drawing_nothing() {
    let mut empty = model(60, 1.0);
    empty.lods.clear();
    let result = run(&empty, &spec(AssetCategory::SmallProp));
    assert!(result.has(ValidationCode::EmptyMesh), "{result:#?}");

    let mut empty_lod = model(60, 1.0);
    empty_lod.lods[0].submeshes.clear();
    assert!(run(&empty_lod, &spec(AssetCategory::SmallProp)).has(ValidationCode::EmptyMesh));
}

#[test]
fn the_runtime_limits_apply_the_structural_rules_and_none_of_the_art_direction() {
    // The same asset that fails as a small prop must load, because the runtime path knows
    // only that a file was opened — not what the thing inside it is for.
    let asset = model(700, 1.2);
    let runtime = AssetLimits::runtime(1_000_000, 2_048, 16, 32);
    let result = validate_asset(&asset, &AssetSpec::unclassified("firemage.glb"), &runtime);
    assert!(
        result.passed(),
        "{:#?}",
        result.errors().collect::<Vec<_>>()
    );
    assert!(
        result.diagnostics.is_empty(),
        "no art-direction rule may fire on the runtime path: {result:#?}"
    );

    // And a dangling index still fails it, because that is what the GPU cannot survive.
    let mut broken = asset;
    broken.lods[0].submeshes[0].indices = vec![0, 1, 9_999];
    let result = validate_asset(&broken, &AssetSpec::unclassified("firemage.glb"), &runtime);
    assert!(result.has(ValidationCode::InvalidIndices), "{result:#?}");
}
