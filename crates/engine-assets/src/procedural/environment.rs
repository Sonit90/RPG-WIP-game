//! The environment library: seventeen models, authored in code, for one combat arena.
//!
//! ```text
//!  terrain        ground_plain   arena_floor
//!  architecture   wall_segment   pillar        arch          dome
//!  rocks          rock_small     rock_medium   rock_spire
//!  vegetation     shrub          fern          pine          leaf_screen
//!  props          brazier        supply_crate  banner
//!  background     ridge          sky_dome
//! ```
//!
//! Same rules as the rest of [`crate::procedural`]: built from tapered prisms and flat
//! primitives, original work with no licence attached, and producing exactly the
//! [`ModelAsset`] the glTF importer produces — so replacing any one of them with an
//! artist's `.glb` is a change to [`environment_model`] and nothing else.
//!
//! # The material budget
//!
//! Ten textures across the whole library, and every model draws from that set:
//!
//! | Texture | Used by |
//! | --- | --- |
//! | `env_stone` | wall segment, pillar, arch |
//! | `env_flagstone` | arena floor |
//! | `env_ground` | ground plain |
//! | `env_rock` | all three rocks |
//! | `env_bark` | pine trunk |
//! | `env_foliage` | pine canopy, shrub, fern |
//! | `env_leaf` | leaf screen (the one texture with an alpha channel that means something) |
//! | `env_wood` | crate, banner pole |
//! | `env_metal` | brazier |
//! | `env_cloth` | banner |
//!
//! The sky dome and the ridges are untextured: a gradient is cheaper as a two-colour vertex
//! ramp than as an image, and a distant silhouette has no detail to carry.
//!
//! # Why the textures are duplicated between models
//!
//! `ModelAsset` owns its textures and `Renderer::create_model` uploads them, so `rock_small`
//! and `rock_medium` each carry a copy of `env_rock` and each upload one. Three 128×128
//! RGBA copies is 192 KB, measured and recorded in `phase_environment.md`; the fix — a
//! renderer-side texture cache keyed by name — is a change to the uploader that no caller
//! would notice, and it is not worth making for a fifth of a megabyte. It is listed under
//! deferred work rather than pretended away.
//!
//! # Levels of detail
//!
//! Chosen per asset from how large it is and how many of it there are, exactly as the phase
//! brief asks — not three levels each because three is the number:
//!
//! | Asset | Levels | Why |
//! | --- | --- | --- |
//! | pine, pillar, rock_medium, rock_spire | 3 | Numerous *and* tall enough to stay on screen from across the arena. |
//! | wall segment, arch, crate, brazier, banner, shrub, fern, leaf_screen, rock_small, arena floor | 2 | Either few, or small enough that the second level is the last one that matters. |
//! | ground plain, sky dome, ridge | 1 | Two triangles, a shell and a silhouette. There is nothing to remove. |

use engine_core::math::{Vec2, Vec3};
use engine_environment::EnvironmentAsset;
use engine_environment::asset as ids;

use crate::material::MaterialAsset;
use crate::mesh::SubMesh;
use crate::model::ModelAsset;
use crate::procedural::{MeshBuilder, Prism};
use crate::texture::{TextureAsset, TextureFormat};

/// Radius the arena floor disc is authored at, in metres.
///
/// Must match `engine_environment::arena::FLOOR_RADIUS`: the scene places the floor at scale
/// one, so the mesh *is* the number. Asserted in this module's tests rather than left as a
/// comment, because two constants that must agree and are not checked will not agree.
pub const FLOOR_RADIUS: f32 = 41.0;

/// Length one wall segment is authored at, in metres.
///
/// Must match `engine_environment::arena::WALL_SEGMENT_LENGTH`, for the same reason: the
/// kerb is a row of these placed end to end, and a mismatch is a row with gaps or overlaps.
pub const WALL_SEGMENT_LENGTH: f32 = 8.0;

/// Radius the sky dome is authored at, in metres.
///
/// Inside the camera's far plane and outside everything else in the scene. A dome beyond the
/// far plane is clipped away and the sky becomes the clear colour.
pub const SKY_RADIUS: f32 = 420.0;

/// The model for one environment asset.
///
/// # Panics
///
/// Never. An unknown id returns a small grey marker cube rather than nothing: a scene that
/// names an asset the library does not have is a content bug, and a visible magenta-grey box
/// standing where the object should be is how it gets noticed. [`environment_library`] is
/// the exhaustive list, and a test asserts every id in it resolves to something real.
#[must_use]
pub fn environment_model(asset: EnvironmentAsset) -> ModelAsset {
    match asset {
        a if a == ids::GROUND_PLAIN => ground_plain(),
        a if a == ids::ARENA_FLOOR => arena_floor(),
        a if a == ids::WALL_SEGMENT => wall_segment(),
        a if a == ids::PILLAR => pillar(),
        a if a == ids::ARCH => arch(),
        a if a == ids::DOME => dome(),
        a if a == ids::ROCK_SMALL => rock(RockSize::Small),
        a if a == ids::ROCK_MEDIUM => rock(RockSize::Medium),
        a if a == ids::ROCK_SPIRE => rock(RockSize::Spire),
        a if a == ids::SHRUB => shrub(),
        a if a == ids::FERN => fern(),
        a if a == ids::PINE => pine(),
        a if a == ids::LEAF_SCREEN => leaf_screen(),
        a if a == ids::BRAZIER => brazier(),
        a if a == ids::SUPPLY_CRATE => supply_crate(),
        a if a == ids::BANNER => banner(),
        a if a == ids::RIDGE => ridge(),
        a if a == ids::SKY_DOME => sky_dome(),
        other => missing(other),
    }
}

/// Every library asset, paired with its model.
///
/// What an uploader iterates: one `create_model` per entry, once, at load.
#[must_use]
pub fn environment_library() -> Vec<(EnvironmentAsset, ModelAsset)> {
    ids::LIBRARY
        .iter()
        .map(|asset| (*asset, environment_model(*asset)))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Textures
// ---------------------------------------------------------------------------------------

/// Deterministic value in `0..1` from a pair of integers.
///
/// The same hash the training dummy's textures use, so a texture generated here is the same
/// texture on every machine.
fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    (value & 0xFFFF) as f32 / 65535.0
}

/// Packs a linear-ish RGB triple into an opaque sRGB texel.
fn texel(color: [f32; 3]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

/// Mixes two colours.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Texture size every environment texture is generated at, in texels.
///
/// 128 is the same size the training dummy's textures use. These are tiling surface details
/// seen at a distance, not character faces; 256 would quadruple the memory for detail that
/// is below a pixel at gameplay range.
const TEXTURE_SIZE: u32 = 128;

/// A mortared block wall: the surface every piece of architecture shares.
fn stone_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_stone",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            // Four courses of blocks, offset by half a block on alternate courses so the
            // joints do not line up into a column — which is what makes a running bond read
            // as masonry rather than as a grid.
            let course = (v * 4.0).floor();
            let shifted = u + if (course as i32) % 2 == 0 { 0.0 } else { 0.25 };
            let block_u = (shifted * 4.0).fract().abs();
            let block_v = (v * 4.0).fract();
            let joint = if !(0.035..=0.965).contains(&block_u) || !(0.06..=0.94).contains(&block_v)
            {
                1.0
            } else {
                0.0
            };
            let grain = hash01(
                (shifted * 4.0) as u32 * 71 + course as u32,
                (v * TEXTURE_SIZE as f32) as u32,
            ) * 0.18;
            let base = mix([0.56, 0.55, 0.51], [0.42, 0.41, 0.39], grain * 3.0);
            texel(mix(base, [0.22, 0.21, 0.20], joint * 0.85))
        },
    )
}

/// Large irregular paving: the arena floor.
fn flagstone_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_flagstone",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            // Three by three slabs with a wide joint, each slab given its own shade so the
            // floor has variation at slab scale rather than only at texel scale. A floor
            // that is uniform at slab scale reads as a texture, not as a surface.
            let cell_u = (u * 3.0).floor() as u32;
            let cell_v = (v * 3.0).floor() as u32;
            let inner_u = (u * 3.0).fract();
            let inner_v = (v * 3.0).fract();
            let joint = f32::from(
                !(0.045..=0.955).contains(&inner_u) || !(0.045..=0.955).contains(&inner_v),
            );
            let shade = hash01(cell_u, cell_v);
            let speckle = hash01(
                (u * TEXTURE_SIZE as f32) as u32,
                (v * TEXTURE_SIZE as f32) as u32,
            ) * 0.12;
            let slab = mix(
                [0.60, 0.57, 0.50],
                [0.44, 0.42, 0.38],
                shade * 0.9 + speckle,
            );
            texel(mix(slab, [0.26, 0.24, 0.21], joint * 0.9))
        },
    )
}

/// Dry grass over dirt: the plain the arena stands on.
fn ground_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_ground",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            // Two octaves of hash noise, one coarse and one fine. The coarse one makes
            // patches the eye can follow across the plain; the fine one keeps the surface
            // from looking like a blurred photograph when the camera is close.
            let coarse = hash01((u * 9.0) as u32, (v * 9.0) as u32);
            let fine = hash01(
                (u * TEXTURE_SIZE as f32) as u32,
                (v * TEXTURE_SIZE as f32) as u32,
            );
            let blades = ((v * 61.0 + (u * 13.0).sin() * 3.0).sin() * 0.5 + 0.5) * 0.25;
            let t = (coarse * 0.55 + fine * 0.25 + blades * 0.2).clamp(0.0, 1.0);
            texel(mix([0.30, 0.36, 0.19], [0.42, 0.36, 0.23], t))
        },
    )
}

/// Grey-brown granite, for every boulder.
fn rock_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_rock",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let fine = hash01(
                (u * TEXTURE_SIZE as f32) as u32,
                (v * TEXTURE_SIZE as f32) as u32,
            );
            let veins = ((u * 21.0 + v * 7.0).sin() * 0.5 + 0.5).powf(3.0) * 0.4;
            let t = (fine * 0.6 + veins).clamp(0.0, 1.0);
            texel(mix([0.47, 0.46, 0.44], [0.27, 0.26, 0.25], t))
        },
    )
}

/// Vertically grooved bark.
fn bark_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_bark",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let grooves = ((u * 47.0).sin() * 0.5 + 0.5).powf(1.6);
            let noise = hash01(
                (u * TEXTURE_SIZE as f32) as u32,
                (v * TEXTURE_SIZE as f32) as u32,
            ) * 0.25;
            texel(mix(
                [0.31, 0.22, 0.15],
                [0.14, 0.10, 0.07],
                (grooves * 0.7 + noise).clamp(0.0, 1.0),
            ))
        },
    )
}

/// Clustered leaves, for every plant.
fn foliage_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_foliage",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let clumps = hash01((u * 11.0) as u32, (v * 11.0) as u32);
            let needles = ((v * 83.0 + (u * 23.0).sin() * 4.0).sin() * 0.5 + 0.5) * 0.4;
            let t = (clumps * 0.5 + needles * 0.5).clamp(0.0, 1.0);
            texel(mix([0.20, 0.38, 0.19], [0.10, 0.22, 0.12], t))
        },
    )
}

/// Broad leaves with holes between them: the one environment texture with real alpha.
///
/// The alpha channel is the silhouette. Nine leaves scattered across the tile, each an
/// ellipse fattened by a hash so no two are the same shape, and nothing between them. What
/// makes this a *cutout* rather than a blend is that alpha is only ever near 0 or near 1 —
/// an alpha test compares against one number and keeps or discards, so a texture with a soft
/// ramp in it would give a shadow whose edge moves when the cutoff is retuned.
fn leaf_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_leaf",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            const COLUMNS: f32 = 3.0;
            let (cell_u, cell_v) = ((u * COLUMNS).floor(), (v * COLUMNS).floor());
            let (local_u, local_v) = ((u * COLUMNS).fract(), (v * COLUMNS).fract());
            let seed = hash01(cell_u as u32, cell_v as u32);
            // Each leaf is an ellipse leaning one way or the other, tall and narrow.
            let lean = (seed - 0.5) * 0.8;
            let x = (local_u - 0.5) + lean * (local_v - 0.5);
            let y = local_v - 0.5;
            let radius = (x / 0.22).powi(2) + (y / 0.42).powi(2);
            if radius > 1.0 {
                // Transparent, but *green*. A cutout texture whose gaps are black gets a
                // black fringe wherever the sampler filters across a leaf's edge, because
                // bilinear filtering averages colour and alpha separately and does not know
                // which texels the alpha test was about to throw away.
                let mut gap = texel([0.18, 0.34, 0.17]);
                gap[3] = 0;
                return gap;
            }
            // A midrib and a darker edge, so the card still reads as a leaf up close where
            // the silhouette alone is not enough.
            let rib = f32::from(x.abs() < 0.02);
            let edge = radius.clamp(0.0, 1.0);
            let base = mix(
                [0.24, 0.44, 0.20],
                [0.13, 0.27, 0.13],
                edge * 0.8 + seed * 0.2,
            );
            let color = mix(base, [0.36, 0.52, 0.26], rib * 0.6);
            let mut packed = texel(color);
            packed[3] = 255;
            packed
        },
    )
}

/// Sawn planks, for the crate and the banner's pole.
fn wood_texture() -> TextureAsset {
    TextureAsset::generated(
        "env_wood",
        TEXTURE_SIZE,
        TEXTURE_SIZE,
        TextureFormat::Rgba8Srgb,
        |u, v| {
            let plank = (v * 5.0).fract();
            let seam = f32::from(!(0.06..=0.94).contains(&plank));
            let grain = ((u * 37.0 + (v * 5.0).floor() * 2.3).sin() * 0.5 + 0.5) * 0.3;
            let base = mix([0.52, 0.37, 0.22], [0.34, 0.24, 0.14], grain);
            texel(mix(base, [0.18, 0.12, 0.08], seam * 0.8))
        },
    )
}

/// Dark hammered iron, for the brazier.
fn metal_texture() -> TextureAsset {
    TextureAsset::generated("env_metal", 64, 64, TextureFormat::Rgba8Srgb, |u, v| {
        let dents = hash01((u * 16.0) as u32, (v * 16.0) as u32);
        let fine = hash01((u * 64.0) as u32, (v * 64.0) as u32) * 0.2;
        texel(mix(
            [0.24, 0.24, 0.26],
            [0.11, 0.11, 0.13],
            dents * 0.8 + fine,
        ))
    })
}

/// A woven banner: deep red with a lighter warp.
fn cloth_texture() -> TextureAsset {
    TextureAsset::generated("env_cloth", 64, 64, TextureFormat::Rgba8Srgb, |u, v| {
        let weave = (((u * 64.0).sin() + (v * 64.0).sin()) * 0.25 + 0.5).clamp(0.0, 1.0);
        // A pale chevron across the middle third, so the banner has a device on it
        // rather than being a rectangle of one colour.
        let device =
            f32::from((v - 0.5).abs() < 0.16 && (u - 0.5).abs() * 1.4 < (v - 0.5).abs() + 0.14);
        let base = mix([0.52, 0.11, 0.10], [0.36, 0.07, 0.07], weave);
        texel(mix(base, [0.82, 0.72, 0.44], device * 0.9))
    })
}

// ---------------------------------------------------------------------------------------
// Terrain
// ---------------------------------------------------------------------------------------

/// The flat plain, as a unit quad the scene scales to cover the horizon.
///
/// One quad and two triangles. A height-mapped terrain system is explicitly out of scope
/// for this phase, and the arena is flat because the combat is: an ability placed on a slope
/// would need its indicator projected onto the slope, which is a Phase 3 problem.
///
/// The UVs run 0..64 rather than 0..1, so the texture tiles sixty-four times across the
/// quad. At the scene's 640 m scale that is one ten-metre tile, which is the size a stylized
/// ground texture reads at from a third-person camera. The sampler is `WRAP`, which is what
/// makes a UV past 1 mean "tile" rather than "clamp to the edge texel".
fn ground_plain() -> ModelAsset {
    const TILES: f32 = 64.0;
    let mut builder = MeshBuilder::new();
    builder.quad(
        [
            Vec3::new(-0.5, 0.0, -0.5),
            Vec3::new(-0.5, 0.0, 0.5),
            Vec3::new(0.5, 0.0, 0.5),
            Vec3::new(0.5, 0.0, -0.5),
        ],
        Vec3::Y,
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, TILES),
            Vec2::new(TILES, TILES),
            Vec2::new(TILES, 0.0),
        ],
    );
    ModelAsset::rigid(
        "ground_plain",
        vec![builder.into_submesh("plain", 0)],
        vec![
            MaterialAsset::colored("env_ground", 1.0, 1.0, 1.0)
                .with_roughness(0.97)
                .with_base_color_texture(0),
        ],
        vec![ground_texture()],
    )
}

/// The arena's paved floor: a disc, two centimetres above the plain.
///
/// A disc rather than a square, because the boundary is a square and a round floor inside a
/// square wall is what stops the corners reading as dead space. Thirty-two segments at 41 m
/// radius puts each edge at four metres, which is below what the eye resolves as a polygon
/// from the gameplay camera.
fn arena_floor() -> ModelAsset {
    let level = |segments: u32| {
        let mut builder = MeshBuilder::new();
        builder.disc(FLOOR_RADIUS, segments, 0.0, 24.0);
        vec![builder.into_submesh("floor", 0)]
    };
    ModelAsset::rigid(
        "arena_floor",
        level(32),
        vec![
            MaterialAsset::colored("env_flagstone", 1.0, 1.0, 1.0)
                .with_roughness(0.90)
                .with_base_color_texture(0),
        ],
        vec![flagstone_texture()],
    )
    // One coarser level. The floor is always on screen, so it never reaches this from the
    // gameplay camera — it exists for the overhead and long-shot views a future spectator
    // or minimap camera will take.
    .with_lod(level(12), 150.0)
}

// ---------------------------------------------------------------------------------------
// Architecture
// ---------------------------------------------------------------------------------------

/// One length of the boundary kerb: a footing, a wall and a capstone.
///
/// Authored along X so a row of them laid end to end forms a straight wall, and yawed a
/// quarter turn by the scene for the two sides that run along Z.
fn wall_segment() -> ModelAsset {
    let half = WALL_SEGMENT_LENGTH * 0.5;
    let level = |detail: bool| {
        let mut stone = MeshBuilder::new();
        // The wall itself: slightly battered — wider at the base than at the top — which is
        // what makes a stone wall look like it is holding itself up.
        stone.prism(
            &Prism::new(
                Vec3::new(-half, 0.0, 0.0),
                Vec3::new(half, 0.0, 0.0),
                0.0,
                0.0,
            )
            .with_halves(Vec2::new(0.50, 0.46), Vec2::new(0.50, 0.34))
            .with_width_axis(Vec3::Y)
            // The prism runs along X, so its "width" axis is Y and its cross-section is
            // (height, depth). Placed with its lower cap at -half and upper at +half, the
            // shape spans the segment; it is then lifted so its base sits on the floor.
            .with_uv_span(0.0, 1.0),
        );
        if detail {
            // A footing course, wider than the wall.
            stone.prism(
                &Prism::new(
                    Vec3::new(-half, 0.0, 0.0),
                    Vec3::new(half, 0.0, 0.0),
                    0.0,
                    0.0,
                )
                .with_halves(Vec2::new(0.12, 0.58), Vec2::new(0.12, 0.58))
                .with_width_axis(Vec3::Y)
                .with_uv_span(0.0, 0.2),
            );
            // A capstone, wider again, which is what reads as a finished wall rather than
            // as a slab standing on edge.
            stone.prism(
                &Prism::new(
                    Vec3::new(-half, 1.02, 0.0),
                    Vec3::new(half, 1.02, 0.0),
                    0.0,
                    0.0,
                )
                .with_halves(Vec2::new(0.10, 0.52), Vec2::new(0.10, 0.52))
                .with_width_axis(Vec3::Y)
                .with_uv_span(0.8, 0.2),
            );
        }
        vec![stone.into_submesh("wall", 0)]
    };

    // The prisms above are centred on y = 0 in their cross-section, so the whole thing is
    // lifted by its own half-height to stand on the floor. Done by rebuilding rather than by
    // a transform, because a `ModelAsset` has no transform and every model in this project
    // stands on its own origin.
    let lift = |submeshes: Vec<SubMesh>, amount: f32| lifted(submeshes, amount);

    ModelAsset::rigid(
        "wall_segment",
        lift(level(true), 0.50),
        vec![stone_material()],
        vec![stone_texture()],
    )
    .with_lod(lift(level(false), 0.50), 45.0)
}

/// A standing stone pillar: base, shaft, collar and capital.
fn pillar() -> ModelAsset {
    let level = |detail: u8| {
        let mut stone = MeshBuilder::new();
        // The shaft, tapered. Always present: it is what a pillar is at any distance.
        stone.prism(
            &Prism::new(
                Vec3::new(0.0, 0.18, 0.0),
                Vec3::new(0.0, 4.10, 0.0),
                0.0,
                0.0,
            )
            .with_halves(Vec2::new(0.42, 0.42), Vec2::new(0.33, 0.33))
            .with_uv_span(0.05, 0.9),
        );
        if detail >= 1 {
            // A plinth and a capital. Two prisms that turn a post into a pillar.
            stone.prism(
                &Prism::new(Vec3::ZERO, Vec3::new(0.0, 0.20, 0.0), 0.58, 0.52)
                    .with_uv_span(0.0, 0.1),
            );
            stone.prism(
                &Prism::new(
                    Vec3::new(0.0, 4.06, 0.0),
                    Vec3::new(0.0, 4.42, 0.0),
                    0.36,
                    0.52,
                )
                .with_uv_span(0.85, 0.15),
            );
        }
        if detail >= 2 {
            // A collar two thirds up, and a cap slab. The details that only exist close to.
            stone.prism(
                &Prism::new(
                    Vec3::new(0.0, 2.70, 0.0),
                    Vec3::new(0.0, 2.92, 0.0),
                    0.44,
                    0.42,
                )
                .with_uv_span(0.55, 0.1),
            );
            stone.prism(
                &Prism::new(
                    Vec3::new(0.0, 4.42, 0.0),
                    Vec3::new(0.0, 4.58, 0.0),
                    0.50,
                    0.44,
                )
                .with_uv_span(0.9, 0.1),
            );
        }
        vec![stone.into_submesh("pillar", 0)]
    };

    ModelAsset::rigid(
        "pillar",
        level(2),
        vec![stone_material()],
        vec![stone_texture()],
    )
    .with_lod(level(1), 40.0)
    .with_lod(level(0), 95.0)
}

/// The gate's broken arch: two haunches and a span with a piece missing.
///
/// Placed by the scene between the two gate pillars, which is what it appears to rest on —
/// so this model is the span alone, five metres up, and nothing here reaches the ground.
/// That is also why it is decoration: the pillars underneath already block.
fn arch() -> ModelAsset {
    const GATE_HALF: f32 = 13.0;
    const SPRING: f32 = 4.35;
    let level = |detail: bool| {
        let mut stone = MeshBuilder::new();
        // Two haunches leaning in from the pillar tops towards the crown.
        for side in [-1.0_f32, 1.0] {
            stone.prism(
                &Prism::new(
                    Vec3::new(side * GATE_HALF, SPRING, 0.0),
                    Vec3::new(side * GATE_HALF * 0.42, SPRING + 1.75, 0.0),
                    0.0,
                    0.0,
                )
                .with_halves(Vec2::new(0.60, 0.46), Vec2::new(0.46, 0.40))
                .with_uv_span(0.0, 0.6),
            );
        }
        if detail {
            // The crown, deliberately short of meeting: the arch is broken, and the gap is
            // what makes the arena read as old rather than as newly built.
            for side in [-1.0_f32, 1.0] {
                stone.prism(
                    &Prism::new(
                        Vec3::new(side * GATE_HALF * 0.42, SPRING + 1.75, 0.0),
                        Vec3::new(side * GATE_HALF * 0.13, SPRING + 2.15, 0.0),
                        0.0,
                        0.0,
                    )
                    .with_halves(Vec2::new(0.44, 0.40), Vec2::new(0.30, 0.34))
                    .with_uv_span(0.6, 0.4),
                );
            }
            // A keystone hanging from one side of the gap, which is what says the gap is a
            // break rather than a design.
            stone.prism(
                &Prism::new(
                    Vec3::new(-GATE_HALF * 0.13, SPRING + 2.15, 0.0),
                    Vec3::new(-GATE_HALF * 0.02, SPRING + 1.72, 0.0),
                    0.0,
                    0.0,
                )
                .with_halves(Vec2::new(0.28, 0.34), Vec2::new(0.20, 0.30))
                .with_uv_span(0.7, 0.3),
            );
        }
        vec![stone.into_submesh("arch", 0)]
    };

    ModelAsset::rigid(
        "arch",
        level(true),
        vec![stone_material()],
        vec![stone_texture()],
    )
    .with_lod(level(false), 70.0)
}

/// The one stone material every architectural piece shares.
fn stone_material() -> MaterialAsset {
    MaterialAsset::colored("env_stone", 1.0, 1.0, 1.0)
        .with_roughness(0.88)
        .with_base_color_texture(0)
}

/// A smooth stone dome: a round hill on a unit footprint, gentle enough to walk up.
///
/// Authored at radius one and scaled by the placement, because a dome is the one library
/// asset whose whole purpose is to be placed at a size the scene chooses: the line-of-sight
/// course wants a seven-metre hill, and a future scene may want a two-metre boulder-cap out
/// of the same mesh. The scale is uniform, so the profile's shape is the same at any size.
///
/// # Why the curve is not a hemisphere
///
/// A hemisphere is vertical where it meets the ground, so the only way onto one is to be
/// placed there. This one is [`engine_environment::asset::dome_profile`] — a raised cosine,
/// flat at the crest, flat at the rim, and 35 degrees at its steepest — which is a hill a
/// character can walk up and a shape the collision world can follow with tilted boxes
/// instead of a staircase. The mesh and the level read that one function, so what the eye
/// sees, what the feet stand on and what stops a ray are the same hill.
///
/// # Why a swept shell and not a stack of prisms
///
/// Every other rounded piece here is faceted on purpose — a boulder wants facets. A dome
/// does not: what it is *for* is a surface that curves away from the caster, so a target
/// halfway up it is visible and a target the same distance down the far side is not. Facets
/// large enough to see would make that boundary a staircase, and the staircase would be the
/// thing being tested rather than the curve.
fn dome() -> ModelAsset {
    let level = |segments: u32, rings: u32| {
        let mut builder = MeshBuilder::new();
        let point = |angle: f32, t: f32| {
            Vec3::new(
                angle.cos() * t,
                engine_environment::asset::dome_profile(t),
                angle.sin() * t,
            )
        };
        // The surface normal from the profile's own slope: `dy/dt` against the radial
        // direction, which is exact for a surface of revolution and costs one sine.
        let normal = |angle: f32, t: f32| {
            let slope = engine_environment::asset::dome_slope(t);
            Vec3::new(-slope * angle.cos(), 1.0, -slope * angle.sin()).normalize()
        };
        // Planar UVs from the footprint, two tiles across the diameter. A spherical
        // projection would pinch every texel at the crest, which on a stone texture reads as
        // a bruise.
        let uv = |position: Vec3| Vec2::new(position.x + 1.0, position.z + 1.0);
        for index in 0..rings {
            let inner = index as f32 / rings as f32;
            let outer = (index + 1) as f32 / rings as f32;
            for step in 0..segments {
                let a0 = step as f32 / segments as f32 * std::f32::consts::TAU;
                let a1 = (step + 1) as f32 / segments as f32 * std::f32::consts::TAU;
                let corners = [
                    point(a0, outer),
                    point(a0, inner),
                    point(a1, inner),
                    point(a1, outer),
                ];
                // One normal per quad is what the builder takes; the middle of the patch is
                // where it is least wrong.
                let facing = normal((a0 + a1) * 0.5, (inner + outer) * 0.5);
                let uvs = [
                    uv(corners[0]),
                    uv(corners[1]),
                    uv(corners[2]),
                    uv(corners[3]),
                ];
                builder.quad(corners, facing, uvs);
            }
        }
        vec![builder.into_submesh("dome", 0)]
    };

    ModelAsset::rigid(
        "dome",
        level(12, 3),
        vec![stone_material()],
        vec![stone_texture()],
    )
    // One coarser level. A dome is large and there are three of them, so the level that
    // matters is the near one; this is what the far end of the course draws.
    .with_lod(level(8, 2), 60.0)
}

// ---------------------------------------------------------------------------------------
// Rocks
// ---------------------------------------------------------------------------------------

/// Which boulder to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RockSize {
    /// Knee-high, two lobes. The most numerous scatter piece.
    Small,
    /// Waist-high, three lobes.
    Medium,
    /// A leaning spire twice a character's height.
    Spire,
}

impl RockSize {
    fn name(self) -> &'static str {
        match self {
            Self::Small => "rock_small",
            Self::Medium => "rock_medium",
            Self::Spire => "rock_spire",
        }
    }

    /// Lobes at the finest level, and the distances the coarser levels take over at.
    fn levels(self) -> (usize, &'static [f32]) {
        match self {
            Self::Small => (2, &[38.0]),
            Self::Medium => (3, &[34.0, 80.0]),
            Self::Spire => (4, &[45.0, 105.0]),
        }
    }

    /// Height, footprint half-width and lean of the whole boulder.
    fn shape(self) -> (f32, f32, f32) {
        match self {
            Self::Small => (0.62, 0.66, 0.10),
            Self::Medium => (1.15, 0.92, 0.16),
            Self::Spire => (3.60, 0.78, 0.55),
        }
    }
}

/// A faceted boulder: a stack of offset, tapering prisms.
///
/// A boulder is the one shape a prism is *not* obviously the primitive for, and it is built
/// from prisms anyway — because a stack of two or three offset, unevenly tapered prisms
/// gives a faceted mass with a readable silhouette at twenty-four triangles a lobe, and the
/// stylized target wants facets rather than a smooth ellipsoid. A UV sphere at the same
/// triangle count is a lumpy ball; at a silhouette good enough to compete it is four times
/// the cost, for geometry there are a hundred of.
fn rock(size: RockSize) -> ModelAsset {
    let (height, half_width, lean) = size.shape();
    let (lobes, thresholds) = size.levels();

    let level = |count: usize| {
        let mut rock = MeshBuilder::new();
        let count = count.max(1);
        for lobe in 0..count {
            let low = lobe as f32 / count as f32;
            let high = (lobe + 1) as f32 / count as f32;
            // Each lobe is narrower than the one below it, and offset sideways by a
            // deterministic amount, so the stack leans and is not a cone.
            let jitter_x = (hash01(lobe as u32, 17) - 0.5) * half_width * 0.55;
            let jitter_z = (hash01(lobe as u32, 29) - 0.5) * half_width * 0.55;
            let taper_low = 1.0 - low * 0.55;
            let taper_high = 1.0 - high * 0.62;
            rock.prism(
                &Prism::new(
                    Vec3::new(
                        lean * low * half_width + jitter_x * low,
                        height * low,
                        jitter_z * low,
                    ),
                    Vec3::new(lean * high * half_width + jitter_x, height * high, jitter_z),
                    0.0,
                    0.0,
                )
                .with_halves(
                    Vec2::new(
                        half_width * taper_low,
                        half_width * taper_low * (0.78 + hash01(lobe as u32, 37) * 0.35),
                    ),
                    Vec2::new(
                        half_width * taper_high,
                        half_width * taper_high * (0.74 + hash01(lobe as u32, 43) * 0.4),
                    ),
                )
                .with_uv_span(low, high - low),
            );
        }
        vec![rock.into_submesh("rock", 0)]
    };

    // Every lobe past the first is offset and tilted, so the prism's cross-section dips
    // below the axis it is built on and the boulder ends up part-buried. Measured once from
    // the finest level and applied to every level, so a level change is not also a hop.
    let sink = lowest_point(&level(lobes));
    let level = |count: usize| lifted(level(count), -sink);

    let mut model = ModelAsset::rigid(
        size.name(),
        level(lobes),
        vec![
            MaterialAsset::colored("env_rock", 1.0, 1.0, 1.0)
                .with_roughness(0.93)
                .with_base_color_texture(0),
        ],
        vec![rock_texture()],
    );
    for (step, threshold) in thresholds.iter().enumerate() {
        let remaining = lobes.saturating_sub(step + 1).max(1);
        model = model.with_lod(level(remaining), *threshold);
    }
    model
}

// ---------------------------------------------------------------------------------------
// Vegetation
// ---------------------------------------------------------------------------------------

/// The one foliage material every plant shares.
fn foliage_material() -> MaterialAsset {
    MaterialAsset::colored("env_foliage", 1.0, 1.0, 1.0)
        .with_roughness(0.95)
        .with_base_color_texture(0)
}

/// A low round bush: three overlapping tapered masses.
fn shrub() -> ModelAsset {
    let level = |count: usize| {
        let mut leaves = MeshBuilder::new();
        for index in 0..count.max(1) {
            let angle = index as f32 / count.max(1) as f32 * std::f32::consts::TAU;
            let offset = 0.22 * f32::from(count > 1);
            leaves.prism(
                &Prism::new(
                    Vec3::new(angle.cos() * offset, 0.0, angle.sin() * offset),
                    Vec3::new(angle.cos() * offset * 0.4, 0.72, angle.sin() * offset * 0.4),
                    0.0,
                    0.0,
                )
                .with_halves(Vec2::new(0.34, 0.30), Vec2::new(0.20, 0.18))
                .with_uv_span(0.0, 1.0),
            );
        }
        vec![leaves.into_submesh("shrub", 0)]
    };
    // The masses lean outwards, so their cross-sections dip below the ground they start on.
    // Corrected once from the finest level, as the boulders are.
    let sink = lowest_point(&level(3));
    let level = |count: usize| lifted(level(count), -sink);
    ModelAsset::rigid(
        "shrub",
        level(3),
        vec![foliage_material()],
        vec![foliage_texture()],
    )
    .with_lod(level(1), 30.0)
}

/// A fan of blades, splayed outwards from one point.
fn fern() -> ModelAsset {
    let level = |count: usize| {
        let mut leaves = MeshBuilder::new();
        let count = count.max(1);
        for index in 0..count {
            let angle = index as f32 / count as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            // Each blade leans out and up, tapering to a point.
            leaves.prism(
                &Prism::new(
                    Vec3::new(0.0, 0.04, 0.0),
                    Vec3::new(cos * 0.46, 0.52, sin * 0.46),
                    0.0,
                    0.0,
                )
                .with_halves(Vec2::new(0.13, 0.035), Vec2::new(0.03, 0.012))
                .with_width_axis(Vec3::new(-sin, 0.0, cos))
                .with_uv_span(0.0, 1.0),
            );
        }
        vec![leaves.into_submesh("fern", 0)]
    };
    let sink = lowest_point(&level(5));
    let level = |count: usize| lifted(level(count), -sink);
    ModelAsset::rigid(
        "fern",
        level(5),
        vec![foliage_material()],
        vec![foliage_texture()],
    )
    // Ground cover is the first thing that should stop costing anything: two blades at
    // twenty-five metres is still a green smudge in the right place, which is all it was
    // ever contributing.
    .with_lod(level(2), 25.0)
}

/// Crossed cards of leaves: the library's one alpha-tested model.
///
/// Three quads through the same vertical axis at sixty degrees to each other, each emitted
/// twice with opposite winding so a card is a card from both sides rather than vanishing
/// when the camera walks around it. Six quads, twelve triangles — the cheapest geometry in
/// the library, and deliberately so: the whole point is that the shape comes from the
/// texture, so the shape costs nothing to draw and nothing to cast a shadow with.
///
/// # Why the alpha cutoff is 0.5
///
/// [`leaf_texture`] is binary — 0 or 255 and nothing between — so anything strictly inside
/// that range separates the leaves from the gaps identically. 0.5 is the glTF default and
/// the middle of the range, which means the model survives being re-authored as a real
/// asset with a soft-edged texture without its silhouette jumping.
fn leaf_screen() -> ModelAsset {
    const HEIGHT: f32 = 1.7;
    const HALF_WIDTH: f32 = 0.62;
    let level = |cards: usize| {
        let mut builder = MeshBuilder::new();
        let cards = cards.max(1);
        for index in 0..cards {
            let angle = index as f32 / cards as f32 * std::f32::consts::PI;
            let (sin, cos) = angle.sin_cos();
            let across = Vec3::new(cos * HALF_WIDTH, 0.0, sin * HALF_WIDTH);
            let corners = [
                Vec3::new(-across.x, 0.0, -across.z),
                Vec3::new(-across.x, HEIGHT, -across.z),
                Vec3::new(across.x, HEIGHT, across.z),
                Vec3::new(across.x, 0.0, across.z),
            ];
            let uvs = [
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
            ];
            let normal = Vec3::new(-sin, 0.0, cos);
            builder.quad(corners, normal, uvs);
            // The same card facing the other way. Reversing the corner order reverses the
            // winding, and the normal follows it, so the back of a leaf is lit as the back
            // of a leaf rather than as a black hole.
            let flipped = [corners[3], corners[2], corners[1], corners[0]];
            let flipped_uvs = [uvs[3], uvs[2], uvs[1], uvs[0]];
            builder.quad(flipped, -normal, flipped_uvs);
        }
        vec![builder.into_submesh("leaf_screen", 0)]
    };
    ModelAsset::rigid(
        "leaf_screen",
        level(3),
        vec![
            MaterialAsset::colored("env_leaf", 1.0, 1.0, 1.0)
                .with_roughness(0.9)
                .with_base_color_texture(0)
                .with_alpha_cutoff(0.5),
        ],
        vec![leaf_texture()],
    )
    // One card past thirty metres. Three cards and one card have the same silhouette from
    // the side; what the other two buy is the screen still reading as a volume when you walk
    // around it, and at thirty metres nobody walks around it fast enough to notice.
    .with_lod(level(1), 30.0)
}

/// A conifer: a bark trunk and three stacked canopy tiers.
///
/// Two materials, and the only plant that has two: a trunk shaded like foliage reads as a
/// green pole, and the treeline's whole job is to give the horizon a silhouette.
fn pine() -> ModelAsset {
    let level = |tiers: usize| {
        let mut bark = MeshBuilder::new();
        let mut needles = MeshBuilder::new();
        let height = 6.4;
        bark.prism(
            &Prism::new(Vec3::ZERO, Vec3::new(0.0, height * 0.55, 0.0), 0.22, 0.13)
                .with_uv_span(0.0, 1.0),
        );
        let tiers = tiers.max(1);
        for tier in 0..tiers {
            let low = 0.28 + tier as f32 / tiers as f32 * 0.58;
            let high = 0.28 + (tier + 1) as f32 / tiers as f32 * 0.66;
            let width = 1.35 * (1.0 - tier as f32 / (tiers as f32 + 0.6));
            needles.prism(
                &Prism::new(
                    Vec3::new(0.0, height * low, 0.0),
                    Vec3::new(0.0, height * high, 0.0),
                    width,
                    width * 0.22,
                )
                .with_uv_span(low, high - low),
            );
        }
        vec![
            bark.into_submesh("trunk", 0),
            needles.into_submesh("canopy", 1),
        ]
    };
    ModelAsset::rigid(
        "pine",
        level(3),
        vec![
            MaterialAsset::colored("env_bark", 1.0, 1.0, 1.0)
                .with_roughness(0.95)
                .with_base_color_texture(0),
            MaterialAsset::colored("env_foliage", 1.0, 1.0, 1.0)
                .with_roughness(0.96)
                .with_base_color_texture(1),
        ],
        vec![bark_texture(), foliage_texture()],
    )
    .with_lod(level(2), 55.0)
    .with_lod(level(1), 110.0)
}

// ---------------------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------------------

/// A lit brazier: three legs, an iron bowl and a bed of coals.
///
/// The arena's only emissive surface. It is here for readability as much as for decoration:
/// four warm points at the corners of a cool arena are what give the eye a scale reference
/// and stop the far half of the space reading as flat.
fn brazier() -> ModelAsset {
    let level = |detail: bool| {
        let mut iron = MeshBuilder::new();
        let mut coals = MeshBuilder::new();
        // The bowl: a wide shallow tapered prism.
        iron.prism(
            &Prism::new(
                Vec3::new(0.0, 0.78, 0.0),
                Vec3::new(0.0, 1.24, 0.0),
                0.30,
                0.52,
            )
            .with_uv_span(0.0, 1.0),
        );
        if detail {
            for index in 0..3_u32 {
                let angle = index as f32 / 3.0 * std::f32::consts::TAU;
                let (sin, cos) = angle.sin_cos();
                iron.prism(
                    &Prism::new(
                        Vec3::new(cos * 0.34, 0.0, sin * 0.34),
                        Vec3::new(cos * 0.11, 0.80, sin * 0.11),
                        0.07,
                        0.05,
                    )
                    .with_uv_span(0.0, 0.4),
                );
            }
        }
        // The coals, sitting just below the rim.
        coals.prism(
            &Prism::new(
                Vec3::new(0.0, 1.10, 0.0),
                Vec3::new(0.0, 1.30, 0.0),
                0.42,
                0.30,
            )
            .with_uv_span(0.0, 1.0),
        );
        vec![iron.into_submesh("iron", 0), coals.into_submesh("coals", 1)]
    };
    ModelAsset::rigid(
        "brazier",
        level(true),
        vec![
            MaterialAsset::colored("env_metal", 1.0, 1.0, 1.0)
                .with_roughness(0.55)
                .with_metallic(0.75)
                .with_base_color_texture(0),
            // Emissive, and untextured. A glow is a constant added after lighting, so it
            // needs no map, and giving it one would make the coals darker where the map is
            // dark without making them any less lit.
            MaterialAsset::colored("env_coals", 0.55, 0.16, 0.05)
                .with_roughness(0.9)
                .with_emissive(Vec3::new(1.05, 0.42, 0.10)),
        ],
        vec![metal_texture()],
    )
    .with_lod(level(false), 42.0)
}

/// A banded wooden crate.
fn supply_crate() -> ModelAsset {
    let level = |detail: bool| {
        let mut wood = MeshBuilder::new();
        wood.prism(
            &Prism::new(Vec3::ZERO, Vec3::new(0.0, 0.90, 0.0), 0.45, 0.44).with_uv_span(0.0, 1.0),
        );
        if detail {
            // Two bands, slightly proud of the box. Proud rather than inset because two
            // coplanar faces are a depth tie and a tie flickers.
            for height in [0.20_f32, 0.68] {
                wood.prism(
                    &Prism::new(
                        Vec3::new(0.0, height, 0.0),
                        Vec3::new(0.0, height + 0.09, 0.0),
                        0.465,
                        0.462,
                    )
                    .with_uv_span(0.45, 0.1),
                );
            }
        }
        vec![wood.into_submesh("crate", 0)]
    };
    ModelAsset::rigid(
        "supply_crate",
        level(true),
        vec![
            MaterialAsset::colored("env_wood", 1.0, 1.0, 1.0)
                .with_roughness(0.92)
                .with_base_color_texture(0),
        ],
        vec![wood_texture()],
    )
    .with_lod(level(false), 32.0)
}

/// A banner on a crossbar: a pole, an arm and a hanging cloth.
fn banner() -> ModelAsset {
    let level = |detail: bool| {
        let mut wood = MeshBuilder::new();
        let mut cloth = MeshBuilder::new();
        // The pole is in every level. A level that dropped it would be a level with an
        // empty submesh, which validation rejects — correctly, since an empty submesh is a
        // draw of nothing — and it would also make the coarse banner a flag hanging in
        // mid-air.
        wood.prism(
            &Prism::new(Vec3::ZERO, Vec3::new(0.0, 3.55, 0.0), 0.075, 0.055).with_uv_span(0.0, 1.0),
        );
        if detail {
            wood.prism(
                &Prism::new(
                    Vec3::new(-0.52, 3.40, 0.0),
                    Vec3::new(0.52, 3.40, 0.0),
                    0.05,
                    0.05,
                )
                .with_width_axis(Vec3::Y),
            );
        }
        // The cloth, hanging from the crossbar and tapering to a point.
        cloth.prism(
            &Prism::new(
                Vec3::new(0.0, 3.36, 0.0),
                Vec3::new(0.0, 1.30, 0.0),
                0.0,
                0.0,
            )
            .with_halves(Vec2::new(0.50, 0.025), Vec2::new(0.16, 0.02))
            .with_uv_span(0.0, 1.0),
        );
        vec![wood.into_submesh("pole", 0), cloth.into_submesh("cloth", 1)]
    };
    ModelAsset::rigid(
        "banner",
        level(true),
        vec![
            MaterialAsset::colored("env_wood", 1.0, 1.0, 1.0)
                .with_roughness(0.92)
                .with_base_color_texture(0),
            MaterialAsset::colored("env_cloth", 1.0, 1.0, 1.0)
                .with_roughness(0.98)
                .with_base_color_texture(1),
        ],
        vec![wood_texture(), cloth_texture()],
    )
    .with_lod(level(false), 60.0)
}

// ---------------------------------------------------------------------------------------
// Background
// ---------------------------------------------------------------------------------------

/// A distant mountain silhouette: five peaks in a row, two hundred metres wide.
///
/// One level of detail. It is only ever seen from two hundred metres away — the level it
/// would drop to *is* the level it has — and it is untextured for the same reason: a mountain
/// at that range is a shape and a colour, and a rock texture on it would alias.
///
/// The material carries a small emissive term standing in for atmospheric haze, which is
/// what stops the ridges reading as enormous nearby rocks. Real depth fog is a shader change
/// and is deferred; this buys most of the effect for a constant.
fn ridge() -> ModelAsset {
    let mut stone = MeshBuilder::new();
    let peaks: [(f32, f32, f32); 5] = [
        (-86.0, 34.0, 42.0),
        (-38.0, 55.0, 34.0),
        (6.0, 72.0, 40.0),
        (52.0, 48.0, 36.0),
        (94.0, 30.0, 30.0),
    ];
    for (index, (x, height, half_width)) in peaks.into_iter().enumerate() {
        stone.prism(
            &Prism::new(
                Vec3::new(x, -6.0, 0.0),
                Vec3::new(x + (hash01(index as u32, 5) - 0.5) * 14.0, height, 0.0),
                0.0,
                0.0,
            )
            .with_halves(
                Vec2::new(half_width, half_width * 0.55),
                Vec2::new(half_width * 0.10, half_width * 0.08),
            ),
        );
    }
    ModelAsset::rigid(
        "ridge",
        vec![stone.into_submesh("ridge", 0)],
        vec![
            MaterialAsset::colored("env_ridge", 0.30, 0.36, 0.47)
                .with_roughness(1.0)
                .with_emissive(Vec3::new(0.16, 0.20, 0.28)),
        ],
        vec![],
    )
}

/// The sky: an inverted dome with a vertical colour ramp.
///
/// # Why a dome and not a cube map
///
/// A skybox is a second pipeline state, a depth state that writes nothing, a cube texture and
/// a shader that reconstructs a view ray. A dome is geometry, and geometry is what this
/// renderer already draws well: it goes through the existing opaque mesh pipeline, it is one
/// batch, it costs one draw, and it needs no new state at all. At 288 triangles it is
/// cheaper than a single pine tree.
///
/// # Why the normals all point straight up
///
/// The dome's colour has to be the ramp, not the ramp times a lighting term that varies over
/// the shell — a sky lit like a solid object has a bright side, which is exactly what a sky
/// does not have. Every vertex carries `+Y`, so `dot(N, L)` is one constant for the whole
/// dome and the gradient the eye sees is the vertex ramp alone. The ramp is baked into the
/// texture's V axis and the dome's UVs map V to height.
///
/// The skirt below the horizon exists because the plain is a finite quad: from the middle of
/// the arena the ground ends at 320 metres, and without a skirt the horizon would be a band
/// of clear colour between the two.
fn sky_dome() -> ModelAsset {
    const SEGMENTS: u32 = 24;
    const RINGS: u32 = 6;
    /// How far below the horizon the shell continues, as a fraction of the radius.
    const SKIRT: f32 = 0.18;

    let mut builder = MeshBuilder::new();
    // Ring 0 is the skirt's lower edge, ring RINGS is the zenith.
    let ring_height = |ring: u32| -> f32 {
        if ring == 0 {
            return -SKIRT;
        }
        // Cosine-spaced above the horizon, so the rings bunch near the zenith where the
        // gradient changes fastest and the silhouette of the dome is smoothest.
        let t = (ring - 1) as f32 / (RINGS - 1) as f32;
        (t * std::f32::consts::FRAC_PI_2).sin()
    };

    for ring in 0..RINGS {
        let low = ring_height(ring);
        let high = ring_height(ring + 1);
        let low_radius = (1.0 - low.max(0.0) * low.max(0.0)).max(0.0).sqrt();
        let high_radius = (1.0 - high.max(0.0) * high.max(0.0)).max(0.0).sqrt();
        for step in 0..SEGMENTS {
            let a0 = step as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let a1 = (step + 1) as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let point = |angle: f32, height: f32, radius: f32| {
                Vec3::new(
                    angle.cos() * radius * SKY_RADIUS,
                    height * SKY_RADIUS,
                    angle.sin() * radius * SKY_RADIUS,
                )
            };
            // Wound so the *inside* of the shell is the front face: the camera is inside it,
            // and a dome wound outwards is a dome that is entirely back-face culled.
            let uv =
                |height: f32| Vec2::new(0.5, ((height + SKIRT) / (1.0 + SKIRT)).clamp(0.0, 1.0));
            builder.quad(
                [
                    point(a0, low, low_radius),
                    point(a1, low, low_radius),
                    point(a1, high, high_radius),
                    point(a0, high, high_radius),
                ],
                Vec3::Y,
                [uv(low), uv(low), uv(high), uv(high)],
            );
        }
    }

    ModelAsset::rigid(
        "sky_dome",
        vec![builder.into_submesh("sky", 0)],
        vec![
            MaterialAsset::colored("env_sky", 1.0, 1.0, 1.0)
                .with_roughness(1.0)
                .with_base_color_texture(0),
        ],
        vec![sky_texture()],
    )
}

/// The sky's vertical ramp: horizon at the bottom, zenith at the top.
///
/// Eight texels wide, because the ramp only varies vertically and a wider image would be
/// eight identical columns. The colours match
/// `EnvironmentLighting::arena_afternoon`'s `sky_horizon` and `sky_zenith`, brightened by the
/// reciprocal of the constant lighting term the dome's `+Y` normals produce — the sky should
/// come out at the colour it was authored, not at 85% of it.
fn sky_texture() -> TextureAsset {
    /// Divides out the fixed `diffuse + ambient` the dome's constant normal receives under
    /// the arena's own light. Measured rather than guessed: see the test below.
    const LIGHTING_COMPENSATION: f32 = 1.18;
    TextureAsset::generated("env_sky", 8, 128, TextureFormat::Rgba8Srgb, |_u, v| {
        // A little of the horizon's warmth carried up past halfway, so the transition is a
        // gradient rather than a seam.
        let t = v.powf(0.72);
        let color = mix([0.74, 0.66, 0.56], [0.16, 0.31, 0.60], t);
        texel([
            color[0] * LIGHTING_COMPENSATION,
            color[1] * LIGHTING_COMPENSATION,
            color[2] * LIGHTING_COMPENSATION,
        ])
    })
}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

/// The lowest vertex of a level, in metres.
///
/// What a model has to be raised by to stand on its own origin. Zero for geometry authored
/// upright; negative for anything whose prisms are tilted, because a tapered prism's
/// cross-section dips below the axis it was built on.
fn lowest_point(submeshes: &[SubMesh]) -> f32 {
    submeshes
        .iter()
        .flat_map(|submesh| submesh.geometry.positions())
        .map(|position| position.y)
        .fold(f32::MAX, f32::min)
}

/// Raises every vertex of a level by `amount` metres.
///
/// A `ModelAsset` has no transform: every model in this project stands on its own origin,
/// which is what lets extraction place a model at gameplay's feet position with no lift. A
/// primitive that is easiest to author centred on zero — a wall segment's battered
/// cross-section is — is therefore moved once, here, rather than by a transform that would
/// have to be remembered at every placement.
fn lifted(submeshes: Vec<SubMesh>, amount: f32) -> Vec<SubMesh> {
    submeshes
        .into_iter()
        .map(|submesh| {
            let crate::mesh::Geometry::Static(mut vertices) = submesh.geometry else {
                // No environment asset is skinned, and nothing here can make one.
                return submesh;
            };
            for vertex in &mut vertices {
                vertex.position.y += amount;
            }
            SubMesh::new(
                &submesh.name,
                crate::mesh::Geometry::Static(vertices),
                submesh.indices,
                submesh.material,
            )
        })
        .collect()
}

/// What an unknown asset id draws: a grey marker cube at eye height.
///
/// Loud rather than silent. A scene naming an asset the library does not have is a content
/// bug, and a box standing where the object should be is how it is found — an empty space is
/// indistinguishable from an object that was never meant to be there.
fn missing(asset: EnvironmentAsset) -> ModelAsset {
    log::error!("environment asset {asset} is not in the library; drawing a marker");
    let mut builder = MeshBuilder::new();
    builder.prism(&Prism::new(Vec3::ZERO, Vec3::new(0.0, 1.6, 0.0), 0.4, 0.4));
    ModelAsset::rigid(
        "missing",
        vec![builder.into_submesh("marker", 0)],
        vec![
            MaterialAsset::colored("env_missing", 0.85, 0.10, 0.75)
                .with_roughness(0.5)
                .with_emissive(Vec3::new(0.4, 0.02, 0.35)),
        ],
        vec![],
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::model::ModelLod;
    use crate::validate::{ValidationLimits, validate_model};

    #[test]
    fn every_library_asset_builds_validates_and_is_rigid() {
        for (asset, model) in environment_library() {
            let Ok(report) = validate_model(&model, &ValidationLimits::default()) else {
                panic!("{asset} must validate");
            };
            assert!(!model.is_skinned(), "{asset} must not need a skeleton");
            assert_eq!(report.bones, 0, "{asset} has bones");
            assert_eq!(report.clips, 0, "{asset} has animation");
            assert!(report.triangles > 0, "{asset} has no geometry");
            assert!(
                model.name == asset.name(),
                "{asset} is named {}",
                model.name
            );
        }
    }

    #[test]
    fn the_whole_library_stays_inside_a_stylized_triangle_budget() {
        // Not a universal truth — a budget nobody can state a number for is a budget nobody
        // enforces. These are the numbers the arena was composed against, and the phase's
        // measurements are taken with them.
        let mut total = 0;
        for (asset, model) in environment_library() {
            let triangles = model.triangle_count();
            total += triangles;
            let limit = match asset {
                // The shell. One draw, never instanced, never repeated.
                a if a == ids::SKY_DOME => 320,
                // Six of them, each two hundred metres wide.
                a if a == ids::RIDGE => 80,
                // Everything else is scatter, and there are dozens of each.
                _ => 80,
            };
            assert!(
                triangles <= limit,
                "{asset} costs {triangles} triangles against a budget of {limit}"
            );
        }
        assert!(total < 1_200, "the whole library is {total} triangles");
    }

    #[test]
    fn every_model_stands_on_its_own_origin() {
        // The invariant the whole placement path depends on: a scene puts an object at a
        // ground position, and the model has to sit on that rather than through it or above
        // it. Three assets are deliberately not placed that way: the arch is a span five
        // metres overhead, the sky is a shell around everything, and a ridge's feet are
        // buried because a mountain meeting the ground at a hard edge reads as a cardboard
        // cut-out standing on a plain.
        for (asset, model) in environment_library() {
            if asset == ids::ARCH || asset == ids::SKY_DOME || asset == ids::RIDGE {
                continue;
            }
            let Some(lod) = model.lod0() else {
                panic!("{asset} has no LOD0");
            };
            let lowest = lod
                .submeshes
                .iter()
                .flat_map(|submesh| submesh.geometry.positions())
                .map(|position| position.y)
                .fold(f32::MAX, f32::min);
            assert!(
                lowest.abs() < 0.06,
                "{asset} has its lowest vertex at y = {lowest}"
            );
        }
    }

    #[test]
    fn every_level_of_detail_is_cheaper_than_the_one_before_it() {
        for (asset, model) in environment_library() {
            let counts: Vec<usize> = model.lods.iter().map(ModelLod::triangle_count).collect();
            for pair in counts.windows(2) {
                assert!(
                    pair[0] > pair[1],
                    "{asset}: a coarser level costs {} against {}",
                    pair[1],
                    pair[0]
                );
            }
            // And every level draws every material, so a level change is not a material
            // change — which is what would make a LOD swap also a batch change.
            let materials = |lod: &ModelLod| {
                let mut list: Vec<usize> = lod
                    .submeshes
                    .iter()
                    .map(|submesh| submesh.material)
                    .collect();
                list.sort_unstable();
                list
            };
            let Some(first) = model.lods.first() else {
                panic!("{asset} has no levels");
            };
            for lod in &model.lods {
                assert_eq!(
                    materials(lod),
                    materials(first),
                    "{asset} changes materials"
                );
            }
        }
    }

    #[test]
    fn the_assets_that_are_numerous_carry_the_most_levels() {
        // The brief's rule: choose LOD count from size, screen-space importance and how many
        // there are. Not three levels each because three is the number.
        let levels = |asset: EnvironmentAsset| environment_model(asset).lods.len();
        for asset in [ids::PINE, ids::PILLAR, ids::ROCK_MEDIUM, ids::ROCK_SPIRE] {
            assert_eq!(levels(asset), 3, "{asset}");
        }
        for asset in [ids::GROUND_PLAIN, ids::SKY_DOME, ids::RIDGE] {
            assert_eq!(levels(asset), 1, "{asset} has nothing to remove");
        }
        for asset in [ids::FERN, ids::SHRUB, ids::SUPPLY_CRATE, ids::BANNER] {
            assert_eq!(levels(asset), 2, "{asset}");
        }
    }

    #[test]
    fn the_material_set_is_small_and_shared_rather_than_one_per_asset() {
        // The brief: do not create dozens of nearly identical materials.
        let mut names: Vec<String> = Vec::new();
        for (_, model) in environment_library() {
            for material in &model.materials {
                names.push(material.name.clone());
            }
        }
        let total = names.len();
        names.sort();
        names.dedup();
        assert!(
            // Thirteen, because the leaf screen cannot share `env_foliage`: an alpha cutoff
            // is a property of the material, and giving the solid plants one would start
            // running a discard pixel shader over every shrub in the scene to no effect.
            names.len() <= 13,
            "{} distinct materials across the library: {names:?}",
            names.len()
        );
        assert!(
            total > names.len(),
            "no material is shared between assets, which is the thing to avoid"
        );
    }

    #[test]
    fn every_texture_reference_points_at_a_texture_the_model_carries() {
        // Validation checks this too; asserting it here means a broken model fails in this
        // crate's own test rather than at upload.
        for (asset, model) in environment_library() {
            for material in &model.materials {
                for (slot, index) in material.texture_indices() {
                    assert!(
                        index < model.textures.len(),
                        "{asset}'s {} names {slot} texture {index} of {}",
                        material.name,
                        model.textures.len()
                    );
                }
            }
        }
    }

    #[test]
    fn the_whole_library_fits_in_a_megabyte_of_texture_memory() {
        // The duplication the module docs admit to, measured rather than hand-waved.
        let bytes: usize = environment_library()
            .iter()
            .map(|(_, model)| model.texture_bytes())
            .sum();
        // 1.25 MB, not 1 MB. The leaf screen's alpha texture is the tenth 128x128 RGBA
        // image in the library and pushed the total past a megabyte by 36 KB; the number
        // this test exists to catch is a 2048x2048 texture appearing, not the sixty-fourth
        // kilobyte.
        assert!(bytes < 1_310_720, "{bytes} bytes of environment textures");
        assert!(bytes > 100_000, "the textures did not get generated");
    }

    #[test]
    fn the_floor_and_the_wall_are_the_size_the_layout_believes_they_are() {
        // Two constants in two crates that must agree. Unchecked, they will not.
        assert_eq!(FLOOR_RADIUS, engine_environment::arena::FLOOR_RADIUS);
        assert_eq!(
            WALL_SEGMENT_LENGTH,
            engine_environment::arena::WALL_SEGMENT_LENGTH
        );

        let floor = environment_model(ids::ARENA_FLOOR);
        let Some(lod) = floor.lod0() else {
            panic!("the floor has geometry");
        };
        let reach = lod
            .submeshes
            .iter()
            .flat_map(|submesh| submesh.geometry.positions())
            .map(|position| position.x.hypot(position.z))
            .fold(0.0_f32, f32::max);
        assert!(
            (reach - FLOOR_RADIUS).abs() < 0.01,
            "the floor disc reaches {reach} m"
        );

        let wall = environment_model(ids::WALL_SEGMENT);
        let Some(lod) = wall.lod0() else {
            panic!("the wall has geometry");
        };
        let span = lod
            .submeshes
            .iter()
            .flat_map(|submesh| submesh.geometry.positions())
            .map(|position| position.x)
            .fold(0.0_f32, f32::max)
            * 2.0;
        assert!(
            (span - WALL_SEGMENT_LENGTH).abs() < 0.01,
            "one segment spans {span} m"
        );
    }

    #[test]
    fn the_floor_disc_reaches_past_the_wall_and_leaves_the_corners_as_ground() {
        // A round floor inside a square boundary, deliberately. Along each axis the paving
        // runs a metre past the wall, so there is no seam where the player can stand; on the
        // diagonals it stops well short, and the corners are the ground plain. That is the
        // composition: a paved arena worn into a plain, rather than a paved square whose
        // corners are dead space nobody fights in.
        let half = engine_environment::arena::ARENA_HALF_EXTENT;
        assert!(
            FLOOR_RADIUS > half,
            "the paving stops {} m short of the wall",
            half - FLOOR_RADIUS
        );
        let corner = half * std::f32::consts::SQRT_2;
        assert!(FLOOR_RADIUS < corner, "the corners are meant to be ground");
        // And it comfortably contains the whole combat space with room for an area ability
        // placed at its edge.
        const {
            assert!(FLOOR_RADIUS > engine_environment::arena::CLEAR_RADIUS + 8.0);
        }
    }

    #[test]
    fn the_sky_dome_is_inside_the_far_plane_and_outside_everything_else() {
        let far = engine_render_data_far_plane();
        assert!(
            SKY_RADIUS < far,
            "a dome at {SKY_RADIUS} m is clipped by a far plane at {far} m"
        );
        // The furthest thing in the arena is a ridge at about 300 m plus its own 130 m of
        // width. The dome has to be outside that or a mountain pokes through the sky.
        const {
            assert!(SKY_RADIUS > 400.0);
        }
    }

    /// The renderer's default far plane, restated here because this crate cannot depend on
    /// the crate that owns it. Asserted against the real one in `engine-app`'s tests.
    fn engine_render_data_far_plane() -> f32 {
        1_000.0
    }

    #[test]
    fn the_sky_dome_is_wound_inwards_so_the_camera_inside_it_sees_it() {
        let model = environment_model(ids::SKY_DOME);
        let Some(lod) = model.lod0() else {
            panic!("the sky has geometry");
        };
        let Some(submesh) = lod.submeshes.first() else {
            panic!("the sky has a submesh");
        };
        let crate::mesh::Geometry::Static(vertices) = &submesh.geometry else {
            panic!("the sky is rigid");
        };

        // Every triangle's geometric normal must point towards the origin, because that is
        // where the camera is. A dome wound the other way is entirely back-face culled and
        // the symptom is a sky that is simply not there.
        let mut checked = 0;
        for triangle in submesh.indices.chunks_exact(3) {
            let a = vertices[triangle[0] as usize].position;
            let b = vertices[triangle[1] as usize].position;
            let c = vertices[triangle[2] as usize].position;
            let normal = (b - a).cross(c - b);
            if normal.length_squared() < 1e-6 {
                continue;
            }
            let centroid = (a + b + c) * (1.0 / 3.0);
            assert!(
                normal.dot(centroid) < 0.0,
                "a sky triangle at {centroid:?} faces outwards"
            );
            checked += 1;
        }
        assert!(checked > 200, "only {checked} sky triangles were testable");
    }

    #[test]
    fn the_dome_is_wound_outwards_and_its_normals_agree_with_its_winding() {
        let model = environment_model(ids::DOME);
        let Some(lod) = model.lod0() else {
            panic!("the dome has geometry");
        };
        let Some(submesh) = lod.submeshes.first() else {
            panic!("the dome has a submesh");
        };
        let crate::mesh::Geometry::Static(vertices) = &submesh.geometry else {
            panic!("the dome is rigid");
        };

        // The opposite claim to the sky's: the camera is outside this one, and every face of
        // a hill faces upwards. A triangle wound the other way is back-face culled, and the
        // symptom is a hole in the ground where the dome should be.
        let mut checked = 0;
        for triangle in submesh.indices.chunks_exact(3) {
            let a = vertices[triangle[0] as usize].position;
            let b = vertices[triangle[1] as usize].position;
            let c = vertices[triangle[2] as usize].position;
            let geometric = (b - a).cross(c - b);
            if geometric.length_squared() < 1e-9 {
                continue;
            }
            let centroid = (a + b + c) * (1.0 / 3.0);
            assert!(
                geometric.dot(Vec3::Y) > 0.0,
                "a dome triangle at {centroid:?} faces downwards"
            );
            // And the shading normal has to agree with the winding, or the lit side is the
            // side the eye cannot see.
            let shading = vertices[triangle[0] as usize].normal;
            assert!(
                shading.dot(geometric.normalize()) > 0.0,
                "a dome triangle at {centroid:?} is lit on its back face"
            );
            checked += 1;
        }
        // Sixty of the seventy-two: the innermost ring's quads have both of their inner
        // corners at the crest, so one triangle of each pair has no area and no winding.
        assert!(checked >= 60, "only {checked} dome triangles were testable");
    }

    #[test]
    fn every_sky_vertex_carries_the_same_normal_so_the_sky_has_no_lit_side() {
        let model = environment_model(ids::SKY_DOME);
        let Some(lod) = model.lod0() else {
            panic!("the sky has geometry");
        };
        for submesh in &lod.submeshes {
            let crate::mesh::Geometry::Static(vertices) = &submesh.geometry else {
                panic!("the sky is rigid");
            };
            for vertex in vertices {
                assert_eq!(vertex.normal, Vec3::Y, "the sky would have a bright side");
            }
        }
    }

    #[test]
    fn the_skys_compensation_lands_it_near_the_colour_it_was_authored_at() {
        // The number in `sky_texture` is a claim about the shader, so it is checked against
        // the shader's own arithmetic rather than left as a comment.
        let lighting = engine_environment::EnvironmentLighting::arena_afternoon();
        let direction = lighting.sun_direction;
        let length =
            (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z)
                .sqrt();
        // dot(+Y, -normalize(sun_direction))
        let lambert = (-direction.y / length).max(0.0);

        // The shader's green channel, for a texel of 1.0: diffuse + ambient.
        let received = lighting.sun_color.y * lighting.sun_intensity * lambert
            + lighting.ambient_color.y * lighting.ambient_intensity;
        let compensated = received * 1.18;
        assert!(
            (compensated - 1.0).abs() < 0.12,
            "the sky comes out at {compensated} of its authored colour"
        );
    }

    #[test]
    fn an_unknown_asset_draws_a_marker_rather_than_nothing() {
        let model = environment_model(EnvironmentAsset::new("obelisk_of_typos"));
        assert_eq!(model.name, "missing");
        assert!(
            model.triangle_count() > 0,
            "an invisible marker is no marker"
        );
        let Some(material) = model.materials.first() else {
            panic!("the marker has a material");
        };
        assert!(material.emissive.x > 0.0, "the marker must be unmissable");
    }

    #[test]
    fn every_asset_is_the_same_model_every_time_it_is_built() {
        // Determinism through the whole pipeline: same textures, same vertices, same order.
        for asset in ids::LIBRARY {
            assert_eq!(
                environment_model(asset),
                environment_model(asset),
                "{asset} is not reproducible"
            );
        }
    }

    #[test]
    fn the_pine_is_the_only_plant_with_two_materials() {
        // A trunk shaded like foliage reads as a green pole, and the treeline's whole job is
        // the silhouette. Everything else gets one material, because a second one is a
        // second draw per instance.
        assert_eq!(environment_model(ids::PINE).materials.len(), 2);
        assert_eq!(environment_model(ids::SHRUB).materials.len(), 1);
        assert_eq!(environment_model(ids::FERN).materials.len(), 1);
    }

    #[test]
    fn no_asset_costs_more_than_two_draws_per_instance() {
        // A submesh is a draw. A hundred instances of a three-submesh prop is three hundred
        // entries in the render list before batching, and batching can only collapse
        // entries that share a mesh *and* a material.
        for (asset, model) in environment_library() {
            let Some(lod) = model.lod0() else {
                panic!("{asset} has no LOD0");
            };
            assert!(
                lod.submeshes.len() <= 2,
                "{asset} costs {} draws per instance",
                lod.submeshes.len()
            );
        }
    }
}
