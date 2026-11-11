//! Asset validation: every rule `docs/style/ASSET_LIMITATIONS.md` states, run in one pass.
//!
//! ```text
//!  Source .glb ──► import_glb ──► fit textures ──► validate_asset ──► ModelAsset ──► GPU
//!                                                        │
//!                                                  AssetValidationResult
//!                                                   ├ errors    the asset does not enter
//!                                                   ├ warnings  it enters, and is reported
//!                                                   └ statistics
//! ```
//!
//! # Where this runs, and where it does not
//!
//! At import, once per asset, before a single GPU resource exists. Never in a frame: nothing
//! here is cheap enough to run per frame and nothing here would learn anything by doing so —
//! a model's triangle count does not change between frames. [`crate::disk::load_glb`] runs
//! the structural half on every load, and [`crate::disk::load_glb_validated`] runs all of it
//! for a caller that knows what kind of asset it is holding.
//!
//! # One pass, no copies
//!
//! Each submesh is walked twice at most: once over its vertices and once over its indices.
//! Nothing is cloned, no texture is decoded — the importer already decoded them — no mesh is
//! duplicated, and the statistics in the result are counters incremented by rules that were
//! reading that data anyway. Validating the whole environment library costs less than
//! building one of its models.
//!
//! # Reporting, not repairing
//!
//! No rule here modifies the asset. Not the scale, not a texture's size, not a normal. A
//! validator that quietly fixed things would make "it passed" mean nothing and would hide
//! the very generator output the specification exists to keep out. Deciding what to do about
//! a finding is the asset pipeline's job; saying precisely what is wrong is this module's.
//!
//! # Adding a rule
//!
//! Add a [`ValidationCode`], give the number it enforces a field in [`AssetLimits`] or
//! [`CategoryLimits`], and push a [`Diagnostic`] from the function that already walks the
//! data it needs. A rule that is about art direction rather than about what the GPU can
//! survive belongs behind `limits.content_rules` — see [`AssetLimits::runtime`].

pub mod diagnostic;
pub mod limits;
pub mod report;
pub mod result;
pub mod spec;

pub use diagnostic::{Diagnostic, Measure, Severity, Subject, ValidationCode};
pub use limits::{AssetCategory, AssetLimits, CATEGORIES, CategoryLimits};
pub use report::{report, summary_line};
pub use result::{AssetStatistics, AssetValidationResult};
pub use spec::{AssetSpec, CategorySource, category_from_path, collision_primitives};

use engine_core::math::{Vec2, Vec3, Vec4};

use crate::mesh::{Geometry, SubMesh};
use crate::model::{ModelAsset, ModelLod};
use crate::texture::TextureAsset;

/// Checks `model` against the limits its category sets, and reports everything found.
///
/// Never fails: a model that cannot be drawn at all still produces a result, because the
/// caller's question is "what is wrong with this" rather than "is anything". Use
/// [`AssetValidationResult::passed`] for the verdict.
#[must_use]
pub fn validate_asset(
    model: &ModelAsset,
    spec: &AssetSpec,
    limits: &AssetLimits,
) -> AssetValidationResult {
    let mut result =
        AssetValidationResult::new(spec.asset_id.clone(), spec.category, spec.exception);
    result.category_source = spec.category_source;

    // The order below is the order a reader wants the first error in: the asset exists, its
    // levels can be selected between, its textures and materials are sound, and only then
    // its geometry. It is also the order the runtime load path has always reported in.
    if !check_lod_structure(model, &mut result) {
        return result;
    }
    check_textures(model, limits, spec, &mut result);
    check_materials(model, limits, spec, &mut result);

    let bone_count = model.skeleton.as_ref().map_or(0, |skeleton| skeleton.len());
    let needs_tangents = model
        .materials
        .iter()
        .any(|material| material.normal_texture.is_some());
    for (level, lod) in model.lods.iter().enumerate() {
        check_lod(
            model,
            lod,
            level,
            LodContext {
                bone_count,
                needs_tangents,
            },
            limits,
            spec,
            &mut result,
        );
    }
    check_clips(model, bone_count, &mut result);
    check_budgets(model, limits, spec, &mut result);
    // Before the rules that read a measurement rather than the model: size, density and the
    // level-of-detail chain all ask the statistics what the asset turned out to contain.
    finish_statistics(model, spec, &mut result);

    if limits.content_rules {
        check_lod_chain(model, limits, spec, &mut result);
        check_collision(limits, spec, &mut result);
        check_transform(limits, spec, &mut result);
        check_size(limits, spec, &mut result);
    }

    result
}

/// What the geometry rules need to know about the model around a submesh.
#[derive(Debug, Clone, Copy)]
struct LodContext {
    /// Bones in the skeleton, or zero when there is none.
    bone_count: usize,
    /// Whether any material samples a normal map, which makes tangents mandatory.
    needs_tangents: bool,
}

/// Levels exist, LOD0 starts at zero, and the thresholds are strictly increasing.
///
/// Returns whether there is anything left to check: a model with no levels of detail has no
/// geometry, and every later rule would report the same absence again.
fn check_lod_structure(model: &ModelAsset, result: &mut AssetValidationResult) -> bool {
    if model.lods.is_empty() {
        result.push(Diagnostic::error(
            ValidationCode::EmptyMesh,
            Subject::Model,
            "a model needs at least one level of detail",
        ));
        return false;
    }
    let Some(lod0) = model.lods.first() else {
        result.push(Diagnostic::error(
            ValidationCode::EmptyMesh,
            Subject::Model,
            "LOD0 is missing",
        ));
        return false;
    };
    if lod0.min_distance != 0.0 {
        result.push(Diagnostic::error(
            ValidationCode::LodInvalid,
            Subject::Lod { level: 0 },
            format!(
                "LOD0 must start at distance 0, not {}; nothing would be drawn up close",
                lod0.min_distance
            ),
        ));
    }
    // Strictly increasing, so selection is unambiguous. Two levels at the same distance is a
    // level that can never be selected, which is dead content nobody notices.
    for (level, pair) in model.lods.windows(2).enumerate() {
        if pair[1].min_distance <= pair[0].min_distance {
            result.push(Diagnostic::error(
                ValidationCode::LodInvalid,
                Subject::Lod { level: level + 1 },
                format!(
                    "levels must be strictly increasing in distance, found {} after {}",
                    pair[1].min_distance, pair[0].min_distance
                ),
            ));
        }
    }
    true
}

/// Every texture is well-formed, inside the hard limit, and near the preferred one.
fn check_textures(
    model: &ModelAsset,
    limits: &AssetLimits,
    spec: &AssetSpec,
    result: &mut AssetValidationResult,
) {
    let preferred = spec.category.limits().preferred_texture_size;
    for (index, texture) in model.textures.iter().enumerate() {
        let subject = Subject::Texture {
            index,
            name: texture.name.clone(),
        };
        let expected =
            texture.width as usize * texture.height as usize * texture.format.bytes_per_texel();
        if texture.width == 0 || texture.height == 0 {
            result.push(Diagnostic::error(
                ValidationCode::TextureData,
                subject.clone(),
                format!(
                    "{}x{} has no texels to sample",
                    texture.width, texture.height
                ),
            ));
            continue;
        }
        if texture.texels.len() != expected {
            result.push(Diagnostic::error(
                ValidationCode::TextureData,
                subject.clone(),
                format!(
                    "{}x{} needs {expected} bytes, got {}",
                    texture.width,
                    texture.height,
                    texture.texels.len()
                ),
            ));
        }
        let largest = texture.width.max(texture.height);
        if largest > limits.max_texture_size {
            result.push(
                Diagnostic::error(
                    ValidationCode::TextureLimit,
                    subject.clone(),
                    format!(
                        "{}x{} is past the maximum texture resolution of {}x{}",
                        texture.width,
                        texture.height,
                        limits.max_texture_size,
                        limits.max_texture_size
                    ),
                )
                .measured(
                    Measure::Resolution {
                        width: texture.width,
                        height: texture.height,
                    },
                    Measure::Resolution {
                        width: limits.max_texture_size,
                        height: limits.max_texture_size,
                    },
                )
                .over(format!("texture '{}' resolution", texture.name))
                .advising("bake the map at the runtime resolution instead of downsizing at load"),
            );
        } else if limits.content_rules && largest > preferred {
            result.push(
                Diagnostic::warning(
                    ValidationCode::TexturePreferred,
                    subject.clone(),
                    format!(
                        "{}x{} exceeds the {} preferred texture resolution of {preferred}x{preferred}",
                        texture.width, texture.height, spec.category
                    ),
                )
                .measured(
                    Measure::Resolution {
                        width: texture.width,
                        height: texture.height,
                    },
                    Measure::Resolution {
                        width: preferred,
                        height: preferred,
                    },
                )
                .advising("4K maps need explicit justification; section 6"),
            );
        }
        if limits.content_rules
            && (!is_power_of_two(texture.width) || !is_power_of_two(texture.height))
        {
            result.push(Diagnostic::warning(
                ValidationCode::TextureData,
                subject,
                format!(
                    "{}x{} is not a power of two, so it cannot carry a full mip chain",
                    texture.width, texture.height
                ),
            ));
        }
    }

    if limits.content_rules && !model.textures.is_empty() {
        let mut sampled = vec![false; model.textures.len()];
        for material in &model.materials {
            for (_, index) in material.texture_indices() {
                if let Some(flag) = sampled.get_mut(index) {
                    *flag = true;
                }
            }
        }
        let unused = sampled.iter().filter(|used| !**used).count();
        if unused > 0 {
            result.push(
                Diagnostic::warning(
                    ValidationCode::TextureCount,
                    Subject::Model,
                    format!(
                        "{unused} of {} textures are sampled by no material and cost memory \
                         for nothing",
                        model.textures.len()
                    ),
                )
                .advising("remove the unused images from the export"),
            );
        }
    }
}

/// Whether `value` is a power of two, and therefore mippable to 1x1.
fn is_power_of_two(value: u32) -> bool {
    value != 0 && value.is_power_of_two()
}

/// Every material's parameters are usable and its texture slots point at real textures.
fn check_materials(
    model: &ModelAsset,
    limits: &AssetLimits,
    spec: &AssetSpec,
    result: &mut AssetValidationResult,
) {
    for (index, material) in model.materials.iter().enumerate() {
        let subject = Subject::Material {
            index,
            name: material.name.clone(),
        };
        for (slot, texture) in material.texture_indices() {
            let Some(asset) = model.textures.get(texture) else {
                result.push(Diagnostic::error(
                    ValidationCode::TextureReference,
                    subject.clone(),
                    format!(
                        "{slot} names texture {texture}, and the model has {}",
                        model.textures.len()
                    ),
                ));
                continue;
            };
            if limits.content_rules {
                check_texture_slot(slot, asset, &subject, result);
            }
        }
        check_material_range("roughness", material.roughness, &subject, result);
        check_material_range("metallic", material.metallic, &subject, result);
        let colors_finite = material.base_color.dot(material.base_color).is_finite()
            && material.emissive.length_squared().is_finite();
        if !colors_finite {
            result.push(Diagnostic::error(
                ValidationCode::MaterialInvalid,
                subject.clone(),
                "a colour is not a finite number",
            ));
        }
        if limits.content_rules
            && material.is_alpha_tested()
            && !(0.0..=1.0).contains(&material.alpha_cutoff)
        {
            result.push(Diagnostic::error(
                ValidationCode::MaterialInvalid,
                subject,
                format!(
                    "an alpha-tested material needs a cutoff in [0, 1], not {}",
                    material.alpha_cutoff
                ),
            ));
        }
    }

    if limits.content_rules {
        let preferred = spec.category.limits().preferred_material_slots;
        let count = model.materials.len() as u64;
        if count > preferred && count <= limits.material_ceiling(spec.category) {
            result.push(
                Diagnostic::warning(
                    ValidationCode::MaterialPreferred,
                    Subject::Model,
                    format!(
                        "{count} material slots where {} prefers {preferred}; each one is a \
                         separate draw",
                        spec.category
                    ),
                )
                .measured(Measure::Count(count), Measure::Count(preferred))
                .advising("share a texture atlas rather than splitting for small differences"),
            );
        }
    }
}

/// A texture bound to a slot that cannot interpret it: colour data read as data, or the
/// other way round.
///
/// The symptom of getting this wrong is a scene that looks washed out or muddy with no
/// obvious cause, which is exactly the class of bug a validator exists to catch — see
/// [`crate::texture`].
fn check_texture_slot(
    slot: &str,
    texture: &TextureAsset,
    subject: &Subject,
    result: &mut AssetValidationResult,
) {
    let srgb = texture.format.is_srgb();
    match (slot, srgb) {
        ("normal" | "orm", true) => result.push(
            Diagnostic::error(
                ValidationCode::TextureFormat,
                subject.clone(),
                format!(
                    "the {slot} map '{}' is sRGB; a map that carries data must be linear",
                    texture.name
                ),
            )
            .advising("re-import the map as Rgba8Unorm"),
        ),
        ("base_color" | "emissive", false) => result.push(Diagnostic::warning(
            ValidationCode::TextureFormat,
            subject.clone(),
            format!(
                "the {slot} map '{}' is linear; colour is normally sRGB",
                texture.name
            ),
        )),
        _ => {}
    }
}

/// One material parameter that has to be a finite number in `[0, 1]`.
fn check_material_range(
    name: &str,
    value: f32,
    subject: &Subject,
    result: &mut AssetValidationResult,
) {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        result.push(Diagnostic::error(
            ValidationCode::MaterialInvalid,
            subject.clone(),
            format!("{name} {value} is not in [0, 1]"),
        ));
    }
}

/// One level of detail: how many submeshes, and whether each of them is drawable.
fn check_lod(
    model: &ModelAsset,
    lod: &ModelLod,
    level: usize,
    context: LodContext,
    limits: &AssetLimits,
    spec: &AssetSpec,
    result: &mut AssetValidationResult,
) {
    if lod.submeshes.is_empty() {
        result.push(Diagnostic::error(
            ValidationCode::EmptyMesh,
            Subject::Lod { level },
            "a level of detail with no submeshes draws nothing",
        ));
        return;
    }
    let submeshes = lod.submeshes.len() as u64;
    let ceiling = limits.submesh_ceiling(spec.category);
    if submeshes > ceiling {
        result.push(
            Diagnostic::error(
                ValidationCode::SubmeshLimit,
                Subject::Lod { level },
                format!(
                    "{submeshes} submeshes is past the {} maximum of {ceiling}; each one is a \
                     draw call per instance",
                    spec.category
                ),
            )
            .measured(Measure::Count(submeshes), Measure::Count(ceiling))
            .over(format!("lods[{level}] submesh"))
            .advising("combine materials so the asset draws in one or two pieces"),
        );
    } else if limits.content_rules && submeshes > spec.category.limits().preferred_submeshes {
        let preferred = spec.category.limits().preferred_submeshes;
        result.push(
            Diagnostic::warning(
                ValidationCode::SubmeshPreferred,
                Subject::Lod { level },
                format!(
                    "{submeshes} submeshes where {} targets {preferred}",
                    spec.category
                ),
            )
            .measured(Measure::Count(submeshes), Measure::Count(preferred)),
        );
    }

    let triangles = lod.triangle_count() as u64;
    let triangle_ceiling = limits.triangle_ceiling(spec.category, spec.exception);
    if triangles > triangle_ceiling {
        result.push(
            Diagnostic::error(
                ValidationCode::TriangleLimit,
                Subject::Lod { level },
                format!(
                    "{} triangles is past the {} maximum of {}",
                    diagnostic::grouped(triangles),
                    spec.category,
                    diagnostic::grouped(triangle_ceiling)
                ),
            )
            .measured(Measure::Count(triangles), Measure::Count(triangle_ceiling))
            .over(format!("lods[{level}] triangle"))
            .advising("decimate or retopologize the mesh, and remove hidden geometry"),
        );
    }

    for (index, submesh) in lod.submeshes.iter().enumerate() {
        check_submesh(model, submesh, level, index, context, limits, result);
    }
}

/// One submesh: indices in range, material in range, and every attribute usable.
///
/// The two passes here — one over the vertices, one over the indices — are the whole cost of
/// validating geometry. Findings are counted rather than reported one per vertex: a mesh
/// exported without normals has fifty thousand vertices without normals, and fifty thousand
/// diagnostics is not a report.
fn check_submesh(
    model: &ModelAsset,
    submesh: &SubMesh,
    level: usize,
    index: usize,
    context: LodContext,
    limits: &AssetLimits,
    result: &mut AssetValidationResult,
) {
    let subject = Subject::Submesh {
        level,
        index,
        name: submesh.name.clone(),
    };
    if submesh.geometry.is_empty() || submesh.indices.is_empty() {
        result.push(Diagnostic::error(
            ValidationCode::EmptyMesh,
            subject,
            "a submesh needs at least one vertex and one index",
        ));
        return;
    }
    if submesh.indices.len() % 3 != 0 {
        result.push(Diagnostic::error(
            ValidationCode::InvalidIndices,
            subject.clone(),
            format!(
                "a triangle list needs a multiple of three indices, got {}",
                submesh.indices.len()
            ),
        ));
    }
    let vertices = submesh.geometry.len();
    let out_of_range = submesh
        .indices
        .iter()
        .find(|value| **value as usize >= vertices);
    if let Some(bad) = out_of_range {
        result.push(Diagnostic::error(
            ValidationCode::InvalidIndices,
            subject.clone(),
            format!("index {bad} is past the end of {vertices} vertices"),
        ));
    }
    if submesh.material >= model.materials.len() {
        result.push(Diagnostic::error(
            ValidationCode::MaterialReference,
            subject.clone(),
            format!(
                "names material {}, and the model has {}",
                submesh.material,
                model.materials.len()
            ),
        ));
    }

    check_vertices(submesh, &subject, context, limits, result);

    if limits.content_rules && out_of_range.is_none() {
        let degenerate = count_degenerate(submesh);
        if degenerate > 0 {
            result.push(
                Diagnostic::warning(
                    ValidationCode::InvalidGeometry,
                    subject,
                    format!(
                        "{} of {} triangles have no area and shade as nothing",
                        diagnostic::grouped(degenerate),
                        diagnostic::grouped(submesh.triangle_count() as u64)
                    ),
                )
                .measured(Measure::Count(degenerate), Measure::Count(0))
                .advising("weld duplicate vertices and delete zero-area faces"),
            );
        }
        result.statistics.degenerate_triangles += degenerate;
    }
}

/// Everything that is checked per vertex, in one pass over the geometry.
fn check_vertices(
    submesh: &SubMesh,
    subject: &Subject,
    context: LodContext,
    limits: &AssetLimits,
    result: &mut AssetValidationResult,
) {
    let count = submesh.geometry.len();
    let mut tally = Tally::default();
    for index in 0..count {
        let Some(vertex) = vertex_at(&submesh.geometry, index) else {
            continue;
        };
        if !vertex.position.length_squared().is_finite() {
            tally.note_at(Finding::NonFinitePosition, index);
        }
        if !limits.content_rules {
            continue;
        }
        let normal_length = vertex.normal.length_squared();
        if !normal_length.is_finite() {
            tally.note_at(Finding::InvalidNormal, index);
        } else if normal_length < 1.0e-8 {
            tally.note_at(Finding::MissingNormal, index);
        } else if (normal_length.sqrt() - 1.0).abs() > 0.05 {
            tally.note_at(Finding::InvalidNormal, index);
        }
        if !vertex.uv.x.is_finite() || !vertex.uv.y.is_finite() {
            tally.note_at(Finding::InvalidUv, index);
        } else if vertex.uv != Vec2::ZERO {
            tally.any_uv = true;
        }
        let tangent = vertex.tangent;
        let directed = tangent.x * tangent.x + tangent.y * tangent.y + tangent.z * tangent.z;
        if !directed.is_finite() || !tangent.w.is_finite() {
            tally.note_at(Finding::InvalidTangent, index);
        } else if directed <= 1.0e-12 {
            tally.note_at(Finding::MissingTangent, index);
        } else if (tangent.w.abs() - 1.0).abs() > 1.0e-3 {
            tally.note_at(Finding::InvalidTangent, index);
        }
    }

    tally.report(subject, count, context, limits, result);
    check_skinning(submesh, subject, context, result);
}

/// The skinning half of the vertex pass: joints in the palette, weights that sum to one.
fn check_skinning(
    submesh: &SubMesh,
    subject: &Subject,
    context: LodContext,
    result: &mut AssetValidationResult,
) {
    let Geometry::Skinned(vertices) = &submesh.geometry else {
        return;
    };
    if context.bone_count == 0 {
        result.push(Diagnostic::error(
            ValidationCode::SkinningInvalid,
            subject.clone(),
            "skinned geometry needs a skeleton, and the model has none",
        ));
        return;
    }
    for (index, vertex) in vertices.iter().enumerate() {
        if let Some(joint) = vertex
            .joints
            .iter()
            .find(|joint| **joint as usize >= context.bone_count)
        {
            // Checked even where the weight is zero: the shader multiplies before it adds,
            // so it reads the palette entry either way.
            result.push(Diagnostic::error(
                ValidationCode::SkinningInvalid,
                subject.clone(),
                format!(
                    "vertex {index} is bound to bone {joint}, and the skeleton has {}",
                    context.bone_count
                ),
            ));
            return;
        }
        let sum = vertex.weight_sum();
        if !(0.99..=1.01).contains(&sum) {
            result.push(Diagnostic::error(
                ValidationCode::SkinningInvalid,
                subject.clone(),
                format!(
                    "vertex {index} has weights summing to {sum}, which deforms the model \
                     towards the origin"
                ),
            ));
            return;
        }
    }
}

/// One vertex's attributes, whichever layout the geometry is in.
#[derive(Debug, Clone, Copy)]
struct VertexView {
    /// Local-space position, metres.
    position: Vec3,
    /// Shading normal.
    normal: Vec3,
    /// Texture coordinate.
    uv: Vec2,
    /// Tangent, with handedness in `w`.
    tangent: Vec4,
}

/// The vertex at `index`, without copying the geometry.
fn vertex_at(geometry: &Geometry, index: usize) -> Option<VertexView> {
    match geometry {
        Geometry::Static(vertices) => vertices.get(index).map(|vertex| VertexView {
            position: vertex.position,
            normal: vertex.normal,
            uv: vertex.uv,
            tangent: vertex.tangent,
        }),
        Geometry::Skinned(vertices) => vertices.get(index).map(|vertex| VertexView {
            position: vertex.position,
            normal: vertex.normal,
            uv: vertex.uv,
            tangent: vertex.tangent,
        }),
    }
}

/// The position at `index`, without copying the geometry.
fn position_at(geometry: &Geometry, index: usize) -> Option<Vec3> {
    match geometry {
        Geometry::Static(vertices) => vertices.get(index).map(|vertex| vertex.position),
        Geometry::Skinned(vertices) => vertices.get(index).map(|vertex| vertex.position),
    }
}

/// Triangles with a repeated index or no area, which draw nothing and shade as nothing.
fn count_degenerate(submesh: &SubMesh) -> u64 {
    let mut degenerate = 0;
    for triangle in submesh.indices.chunks_exact(3) {
        let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
        if a == b || b == c || a == c {
            degenerate += 1;
            continue;
        }
        let (Some(first), Some(second), Some(third)) = (
            position_at(&submesh.geometry, a as usize),
            position_at(&submesh.geometry, b as usize),
            position_at(&submesh.geometry, c as usize),
        ) else {
            continue;
        };
        let area = (second - first).cross(third - first).length_squared();
        if !area.is_finite() || area <= 1.0e-16 {
            degenerate += 1;
        }
    }
    degenerate
}

/// Which per-vertex rule a finding belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finding {
    /// A position that is not a finite number.
    NonFinitePosition,
    /// A normal of zero length: the attribute is absent rather than wrong.
    MissingNormal,
    /// A normal that is not finite, or not unit length.
    InvalidNormal,
    /// A texture coordinate that is not finite.
    InvalidUv,
    /// A tangent of zero length: never generated, or generated and failed.
    MissingTangent,
    /// A tangent that is not finite, or whose handedness is not ±1.
    InvalidTangent,
}

/// How many vertices tripped each rule, and which one tripped it first.
///
/// One diagnostic per rule per submesh, naming the first offender and counting the rest,
/// because a report with one line per vertex is not a report.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    /// Count and first index for each finding, in [`Finding`] order.
    counts: [(u64, usize); 6],
    /// Whether any vertex had a texture coordinate away from the origin.
    any_uv: bool,
}

impl Tally {
    /// Records that `index` tripped `finding`.
    fn note_at(&mut self, finding: Finding, index: usize) {
        let slot = &mut self.counts[finding as usize];
        if slot.0 == 0 {
            slot.1 = index;
        }
        slot.0 += 1;
    }

    /// The count and first offender for `finding`.
    fn of(&self, finding: Finding) -> (u64, usize) {
        self.counts[finding as usize]
    }

    /// Turns the counts into at most one diagnostic per rule.
    fn report(
        &self,
        subject: &Subject,
        vertices: usize,
        context: LodContext,
        limits: &AssetLimits,
        result: &mut AssetValidationResult,
    ) {
        let (count, first) = self.of(Finding::NonFinitePosition);
        if count > 0 {
            result.push(Diagnostic::error(
                ValidationCode::InvalidGeometry,
                subject.clone(),
                format!("vertex {first} has a non-finite position"),
            ));
        }
        if !limits.content_rules {
            return;
        }

        let (count, first) = self.of(Finding::MissingNormal);
        if count > 0 {
            result.push(
                Diagnostic::error(
                    ValidationCode::MissingNormals,
                    subject.clone(),
                    format!(
                        "{} of {} vertices have no normal, starting at vertex {first}",
                        diagnostic::grouped(count),
                        diagnostic::grouped(vertices as u64)
                    ),
                )
                .measured(Measure::Count(count), Measure::Count(0))
                .advising("export with normals, or recalculate them in the DCC tool"),
            );
        }
        let (count, first) = self.of(Finding::InvalidNormal);
        if count > 0 {
            result.push(
                Diagnostic::error(
                    ValidationCode::InvalidNormals,
                    subject.clone(),
                    format!(
                        "{} of {} normals are not unit length, starting at vertex {first}",
                        diagnostic::grouped(count),
                        diagnostic::grouped(vertices as u64)
                    ),
                )
                .measured(Measure::Count(count), Measure::Count(0)),
            );
        }
        let (count, first) = self.of(Finding::InvalidUv);
        if count > 0 {
            result.push(
                Diagnostic::error(
                    ValidationCode::InvalidUv,
                    subject.clone(),
                    format!(
                        "{} texture coordinates are not finite, starting at vertex {first}",
                        diagnostic::grouped(count)
                    ),
                )
                .measured(Measure::Count(count), Measure::Count(0)),
            );
        } else if !self.any_uv && vertices > 0 {
            result.push(
                Diagnostic::error(
                    ValidationCode::MissingUv,
                    subject.clone(),
                    "every texture coordinate is at the origin, so the submesh has no UVs"
                        .to_owned(),
                )
                .advising("unwrap the mesh; section 8"),
            );
        }
        let (count, first) = self.of(Finding::InvalidTangent);
        if count > 0 {
            result.push(
                Diagnostic::error(
                    ValidationCode::InvalidTangents,
                    subject.clone(),
                    format!(
                        "{} tangents are degenerate or carry a handedness other than ±1, \
                         starting at vertex {first}",
                        diagnostic::grouped(count)
                    ),
                )
                .measured(Measure::Count(count), Measure::Count(0)),
            );
        }
        let (count, first) = self.of(Finding::MissingTangent);
        if count > 0 {
            // A normal map without a tangent frame is a lighting bug, not a missing detail:
            // the shader has no basis to transform the map into. Where nothing samples a
            // normal map it is still worth reporting, because the asset cannot gain one
            // later without being re-exported.
            let severity = if context.needs_tangents {
                Severity::Error
            } else {
                Severity::Warning
            };
            let mut diagnostic = Diagnostic::error(
                ValidationCode::MissingTangents,
                subject.clone(),
                format!(
                    "{} of {} vertices have no tangent, starting at vertex {first}",
                    diagnostic::grouped(count),
                    diagnostic::grouped(vertices as u64)
                ),
            )
            .measured(Measure::Count(count), Measure::Count(0))
            .advising("export tangents, or let SubMesh::new generate them at import");
            diagnostic.severity = severity;
            result.push(diagnostic);
        }
    }
}

/// Every clip is bound to this model's skeleton and has usable keys.
fn check_clips(model: &ModelAsset, bone_count: usize, result: &mut AssetValidationResult) {
    for (index, clip) in model.clips.iter().enumerate() {
        let subject = Subject::Clip {
            index,
            name: clip.name.clone(),
        };
        if bone_count == 0 {
            result.push(Diagnostic::error(
                ValidationCode::AnimationInvalid,
                subject,
                "an animation needs a skeleton, and the model has none",
            ));
            continue;
        }
        if !clip.duration.is_finite() || clip.duration < 0.0 {
            result.push(Diagnostic::error(
                ValidationCode::AnimationInvalid,
                subject.clone(),
                format!("duration {} is not a length of time", clip.duration),
            ));
        }
        for track in &clip.tracks {
            if track.bone as usize >= bone_count {
                result.push(Diagnostic::error(
                    ValidationCode::AnimationInvalid,
                    subject.clone(),
                    format!(
                        "a track animates bone {}, and the skeleton has {bone_count}",
                        track.bone
                    ),
                ));
                continue;
            }
            let times = track
                .translation
                .iter()
                .map(|key| key.time)
                .chain(track.rotation.iter().map(|key| key.time))
                .chain(track.scale.iter().map(|key| key.time));
            for time in times {
                if !time.is_finite() || time < 0.0 {
                    result.push(Diagnostic::error(
                        ValidationCode::AnimationInvalid,
                        subject.clone(),
                        format!("a key is at time {time}"),
                    ));
                    break;
                }
            }
        }
    }
}

/// The counts the specification calls hard limits: vertices, triangles, material slots.
fn check_budgets(
    model: &ModelAsset,
    limits: &AssetLimits,
    spec: &AssetSpec,
    result: &mut AssetValidationResult,
) {
    let materials = model.materials.len() as u64;
    let material_ceiling = limits.material_ceiling(spec.category);
    if materials > material_ceiling {
        result.push(
            Diagnostic::error(
                ValidationCode::MaterialLimit,
                Subject::Model,
                format!(
                    "{materials} material slots is past the {} maximum of {material_ceiling}",
                    spec.category
                ),
            )
            .measured(Measure::Count(materials), Measure::Count(material_ceiling))
            .over("material")
            .advising("merge materials into a shared atlas; section 5"),
        );
    }

    let Some(lod0) = model.lods.first() else {
        return;
    };
    let vertices = lod0.vertex_count() as u64;
    let vertex_ceiling = limits.vertex_ceiling(spec.category, spec.exception);
    if vertices > vertex_ceiling {
        result.push(
            Diagnostic::error(
                ValidationCode::VertexLimit,
                Subject::Lod { level: 0 },
                format!(
                    "{} vertices is past the {} maximum of {}",
                    diagnostic::grouped(vertices),
                    spec.category,
                    diagnostic::grouped(vertex_ceiling)
                ),
            )
            .measured(Measure::Count(vertices), Measure::Count(vertex_ceiling))
            .over("vertex")
            .advising("decimate or retopologize the mesh, and remove hidden geometry"),
        );
    }

    if !limits.content_rules {
        return;
    }
    // Per-submesh, against the absolute ceiling rather than the category's: one piece of an
    // asset that is itself larger than any whole asset may be is the shape a raw generator
    // output arrives in, and naming the piece is what makes it fixable.
    let absolute_vertices = limits.vertex_ceiling(AssetCategory::Unclassified, spec.exception);
    let absolute_triangles = limits.triangle_ceiling(AssetCategory::Unclassified, spec.exception);
    for (level, lod) in model.lods.iter().enumerate() {
        for (index, submesh) in lod.submeshes.iter().enumerate() {
            let subject = Subject::Submesh {
                level,
                index,
                name: submesh.name.clone(),
            };
            let vertices = submesh.geometry.len() as u64;
            if vertices > absolute_vertices {
                result.push(
                    Diagnostic::error(
                        ValidationCode::VertexLimit,
                        subject.clone(),
                        format!(
                            "one submesh holds {} vertices, past the absolute maximum of {}",
                            diagnostic::grouped(vertices),
                            diagnostic::grouped(absolute_vertices)
                        ),
                    )
                    .measured(Measure::Count(vertices), Measure::Count(absolute_vertices))
                    .over("vertex"),
                );
            }
            let triangles = submesh.triangle_count() as u64;
            if triangles > absolute_triangles {
                result.push(
                    Diagnostic::error(
                        ValidationCode::TriangleLimit,
                        subject,
                        format!(
                            "one submesh holds {} triangles, past the absolute maximum of {}",
                            diagnostic::grouped(triangles),
                            diagnostic::grouped(absolute_triangles)
                        ),
                    )
                    .measured(
                        Measure::Count(triangles),
                        Measure::Count(absolute_triangles),
                    )
                    .over("triangle"),
                );
            }
        }
    }
}

/// How many levels there are, and whether each is the reduction the specification asks for.
fn check_lod_chain(
    model: &ModelAsset,
    limits: &AssetLimits,
    spec: &AssetSpec,
    result: &mut AssetValidationResult,
) {
    let expected = spec.category.limits().expected_lods;
    let count = model.lods.len() as u64;
    if count < expected {
        result.push(
            Diagnostic::warning(
                ValidationCode::LodMissing,
                Subject::Model,
                format!(
                    "{count} level(s) of detail where {} expects {expected}",
                    spec.category
                ),
            )
            .measured(Measure::Count(count), Measure::Count(expected))
            .advising("generate the missing levels; section 4"),
        );
    }

    let Some(lod0) = model.lods.first() else {
        return;
    };
    let base = lod0.triangle_count() as f32;
    if base <= 0.0 {
        return;
    }
    for (level, lod) in model.lods.iter().enumerate().skip(1) {
        let triangles = lod.triangle_count() as u64;
        let previous = model.lods[level - 1].triangle_count() as u64;
        if triangles > previous {
            result.push(
                Diagnostic::error(
                    ValidationCode::LodInvalid,
                    Subject::Lod { level },
                    format!(
                        "LOD{level} has {} triangles against LOD{}'s {}, so it is not a \
                         reduction and costs more the further away it is",
                        diagnostic::grouped(triangles),
                        level - 1,
                        diagnostic::grouped(previous)
                    ),
                )
                .measured(Measure::Count(triangles), Measure::Count(previous)),
            );
            continue;
        }
        let Some((low, high)) = limits.lod_band(level) else {
            continue;
        };
        let fraction = triangles as f32 / base;
        if fraction < low || fraction > high {
            let target = limits
                .lod_triangle_targets
                .get(level)
                .copied()
                .unwrap_or(0.0);
            result.push(
                Diagnostic::warning(
                    ValidationCode::LodReduction,
                    Subject::Lod { level },
                    format!(
                        "LOD{level} keeps {:.0}% of LOD0 where about {:.0}% is recommended",
                        fraction * 100.0,
                        target * 100.0
                    ),
                )
                .measured(Measure::Scalar(fraction), Measure::Scalar(target))
                .advising("reduce on visual error rather than on a fixed percentage"),
            );
        }
    }
}

/// Collision: present where the category needs it, and simple enough to be worth having.
fn check_collision(limits: &AssetLimits, spec: &AssetSpec, result: &mut AssetValidationResult) {
    let requires = spec.category.limits().requires_collision;
    let Some(primitives) = spec.collision_primitives else {
        if requires {
            result.push(Diagnostic::info(
                ValidationCode::CollisionMissing,
                Subject::Collision,
                "no collision was supplied to the validator, so none was checked",
            ));
        }
        return;
    };
    if primitives > limits.max_collision_primitives {
        result.push(
            Diagnostic::error(
                ValidationCode::CollisionLimit,
                Subject::Collision,
                format!(
                    "{primitives} collision primitives is past the maximum of {}",
                    limits.max_collision_primitives
                ),
            )
            .measured(
                Measure::Count(primitives),
                Measure::Count(limits.max_collision_primitives),
            )
            .over("collision primitive")
            .advising("replace the hull with boxes, capsules and spheres; section 13"),
        );
    } else if primitives > limits.preferred_collision_primitives {
        result.push(
            Diagnostic::warning(
                ValidationCode::CollisionLimit,
                Subject::Collision,
                format!("{primitives} collision primitives where 1-10 is preferred",),
            )
            .measured(
                Measure::Count(primitives),
                Measure::Count(limits.preferred_collision_primitives),
            ),
        );
    }
    if primitives == 0 && requires {
        result.push(
            Diagnostic::warning(
                ValidationCode::CollisionMissing,
                Subject::Collision,
                format!(
                    "a {} has no collision, so the player walks through it",
                    spec.category
                ),
            )
            .advising("add a box, capsule or simple hull; section 13"),
        );
    }
}

/// The import transform: finite, not degenerate, and not a unit conversion in disguise.
fn check_transform(limits: &AssetLimits, spec: &AssetSpec, result: &mut AssetValidationResult) {
    let transform = spec.transform;
    let finite = transform.translation.length_squared().is_finite()
        && transform.scale.length_squared().is_finite()
        && transform.rotation.length_squared().is_finite();
    if !finite {
        result.push(Diagnostic::error(
            ValidationCode::InvalidScale,
            Subject::Model,
            "the transform contains a value that is not a finite number",
        ));
        return;
    }
    let axes = [
        ("x", transform.scale.x),
        ("y", transform.scale.y),
        ("z", transform.scale.z),
    ];
    for (axis, value) in axes {
        let magnitude = value.abs();
        if magnitude < limits.min_scale {
            result.push(
                Diagnostic::error(
                    ValidationCode::InvalidScale,
                    Subject::Model,
                    format!("scale {value} on {axis} collapses the asset to nothing"),
                )
                .measured(Measure::Scalar(value), Measure::Scalar(limits.min_scale)),
            );
        } else if magnitude > limits.max_scale {
            result.push(
                Diagnostic::error(
                    ValidationCode::InvalidScale,
                    Subject::Model,
                    format!(
                        "scale {value} on {axis} is far outside anything a metre-based scene uses"
                    ),
                )
                .measured(Measure::Scalar(value), Measure::Scalar(limits.max_scale)),
            );
        }
    }
    let largest = axes.iter().fold(0.0_f32, |best, (_, v)| best.max(v.abs()));
    let smallest = axes
        .iter()
        .fold(f32::INFINITY, |best, (_, v)| best.min(v.abs()));
    if smallest > 0.0 && largest / smallest > 100.0 {
        result.push(Diagnostic::warning(
            ValidationCode::InvalidScale,
            Subject::Model,
            format!(
                "the scale is {largest} on one axis and {smallest} on another, which will \
                 shear the normals"
            ),
        ));
    }
}

/// Whether the asset measures like the thing it says it is, in metres.
fn check_size(limits: &AssetLimits, spec: &AssetSpec, result: &mut AssetValidationResult) {
    let size = result.statistics.size_meters;
    if !size.is_finite() {
        result.push(Diagnostic::error(
            ValidationCode::InvalidGeometry,
            Subject::Model,
            "the bounding sphere is not a finite size",
        ));
        return;
    }
    if size <= 0.0 {
        result.push(Diagnostic::warning(
            ValidationCode::BoundsSuspect,
            Subject::Model,
            "the asset has no measurable size; every vertex is at one point",
        ));
        return;
    }
    let (low, high) = spec.category.limits().size_range_meters;
    if size < low || size > high {
        let hint = if size > high * 50.0 {
            "the export is about a hundred times too large, which is centimetres; fix the \
             source asset rather than the placement"
        } else {
            "check the asset against section 14's real-world scale"
        };
        result.push(
            Diagnostic::warning(
                ValidationCode::BoundsSuspect,
                Subject::Model,
                format!(
                    "measures {size:.2} m across; {} assets are {low}-{high} m",
                    spec.category
                ),
            )
            .measured(Measure::Scalar(size), Measure::Scalar(high))
            .advising(hint),
        );
    }

    let density = result.statistics.triangle_count as f64 / f64::from(size);
    if density > limits.max_triangles_per_meter {
        result.push(
            Diagnostic::warning(
                ValidationCode::GeometryDensity,
                Subject::Model,
                format!(
                    "{} triangles across {size:.2} m is {density:.0} triangles per metre",
                    diagnostic::grouped(result.statistics.triangle_count)
                ),
            )
            .measured(
                Measure::Scalar(density as f32),
                Measure::Scalar(limits.max_triangles_per_meter as f32),
            )
            .advising("this is the density of unretopologized generator output; section 26"),
        );
    }
}

/// Fills in the measurements the report prints, from data the rules already walked.
fn finish_statistics(model: &ModelAsset, spec: &AssetSpec, result: &mut AssetValidationResult) {
    let statistics = &mut result.statistics;
    statistics.material_count = model.materials.len() as u64;
    statistics.texture_count = model.textures.len() as u64;
    statistics.texture_bytes = model.texture_bytes() as u64;
    statistics.max_texture_size = model.textures.iter().fold((0, 0), |largest, texture| {
        if texture.width.max(texture.height) > largest.0.max(largest.1) {
            (texture.width, texture.height)
        } else {
            largest
        }
    });
    statistics.lod_count = model.lods.len() as u64;
    statistics.lod_triangles = model
        .lods
        .iter()
        .map(|lod| lod.triangle_count() as u64)
        .collect();
    statistics.total_triangle_count = statistics.lod_triangles.iter().sum();
    statistics.total_vertex_count = model.lods.iter().map(|lod| lod.vertex_count() as u64).sum();
    statistics.bone_count = model.skeleton.as_ref().map_or(0, |s| s.len()) as u64;
    statistics.clip_count = model.clips.len() as u64;
    statistics.collision_primitives = spec.collision_primitives;
    if let Some(lod0) = model.lods.first() {
        statistics.vertex_count = lod0.vertex_count() as u64;
        statistics.triangle_count = lod0.triangle_count() as u64;
        statistics.submesh_count = lod0.submeshes.len() as u64;
        statistics.size_meters = lod0.bounds().radius * 2.0;
        statistics.has_tangents = lod0
            .submeshes
            .iter()
            .all(|submesh| submesh.geometry.has_tangents());
        statistics.has_uvs = lod0
            .submeshes
            .iter()
            .all(|submesh| submesh.geometry.uvs().any(|uv| uv != Vec2::ZERO));
    }
}

#[cfg(test)]
mod tests;
