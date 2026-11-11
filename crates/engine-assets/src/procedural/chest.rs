//! The boss's treasure chest, authored in code: a body, a separate lid and the treasure inside.
//!
//! ```text
//!            lid    local origin = the hinge, so rotating the lid about +X opens it
//!         ,-""""-.
//!   hinge o-------'   <- back (+Z)        the lock plate faces -Z, the engine's "forward"
//!   ┌──────────┐
//!   │  body    │  1.2 m wide, 0.8 m deep, 0.46 m to the rim; the lid arches 0.27 m above it
//!   └──────────┘
//! ```
//!
//! # Three models rather than one
//!
//! The chest moves in three independent ways - the whole thing falls and lands, the lid swings
//! about the hinge, and the treasure grows out of the open box - and a [`ModelAsset`] has no
//! transform hierarchy to express that. So each moving piece is its own model, authored so the
//! transform it needs is the obvious one:
//!
//! * [`chest_body`] stands on its own origin (centre of the footprint, on the ground).
//! * [`chest_lid`] has its origin **on the hinge**, the back top edge of the body; presentation
//!   places it at [`LID_HINGE`] in the body's space and turns it about +X.
//! * [`chest_treasure`] has its origin at the centre of the box's lining; presentation places
//!   it at [`TREASURE_BASE`] and scales it up as the lid opens.
//!
//! # Part order is a contract
//!
//! The renderer draws one part per submesh, in submesh order, and presentation styles them by
//! index - the gold trim breathes while the chest waits, the coins glow once it is open. The
//! indices are the constants in [`part`]; a test pins them to the materials.

use engine_core::math::{Vec2, Vec3};

use crate::material::MaterialAsset;
use crate::mesh::SubMesh;
use crate::model::ModelAsset;
use crate::procedural::{MeshBuilder, Prism};
use crate::texture::{TextureAsset, TextureFormat};

/// How wide the chest is, along X, in metres.
pub const WIDTH: f32 = 1.2;
/// How deep the chest is, along Z, in metres.
pub const DEPTH: f32 = 0.8;
/// How tall the body is to its rim, in metres.
pub const BODY_HEIGHT: f32 = 0.46;
/// How far the closed lid's crown rises above the rim, in metres.
pub const LID_RISE: f32 = 0.27;
/// Where the lid's hinge is, in the body's space: the back top edge.
pub const LID_HINGE: Vec3 = Vec3::new(0.0, BODY_HEIGHT, DEPTH * 0.5);
/// Where the treasure's origin is, in the body's space: on top of the lining.
pub const TREASURE_BASE: Vec3 = Vec3::new(0.0, 0.38, 0.0);

/// Submesh and part indices, shared by all three models where they apply.
pub mod part {
    /// Planks. Textured, dark walnut.
    pub const WOOD: usize = 0;
    /// Iron bands, corner posts and the rim.
    pub const IRON: usize = 1;
    /// Gold trim: the lock plate, rivets, the lid's crest.
    pub const GOLD: usize = 2;
    /// The red cloth lining inside the box and under the lid.
    pub const LINING: usize = 3;
    /// The treasure model's coins.
    pub const COINS: usize = 0;
    /// The treasure model's gems.
    pub const GEMS: usize = 1;
}

/// Half the footprint's width and depth, with the lid's overhang already in.
const HALF_W: f32 = WIDTH * 0.5;
const HALF_D: f32 = DEPTH * 0.5;
/// How far the lid overhangs the body on each side, in metres.
const OVERHANG: f32 = 0.012;
/// Wall thickness of the body, in metres.
const WALL: f32 = 0.055;
/// How thick the lid's shell is, in metres.
const SHELL: f32 = 0.035;
/// How many segments the lid's arch is cut into.
const ARCH_SEGMENTS: usize = 14;
/// Texture repeat of the planks, in metres.
const WOOD_TILE: f32 = 0.5;

/// Deterministic value in `0..1`, the same hash the other generated textures use.
fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    (value & 0xFFFF) as f32 / 65535.0
}

fn texel(color: [f32; 3]) -> [u8; 4] {
    [
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Dark walnut planks with grooves between them, a wavy grain and the odd knot.
///
/// Three planks per repeat. The groove is part of the image rather than geometry: a
/// half-centimetre recess would be below a pixel at gameplay range, and a dark line is what the
/// eye reads as "these are boards".
fn wood_texture() -> TextureAsset {
    TextureAsset::generated("chest_wood", 128, 128, TextureFormat::Rgba8Srgb, |u, v| {
        let plank_v = v * 3.0;
        let plank = plank_v.floor();
        let along = plank_v.fract();
        let groove = {
            let edge = along.min(1.0 - along);
            f32::from(edge < 0.045) * (1.0 - edge / 0.045)
        };
        // Each board has its own tone and grain phase.
        let tone = hash01(plank as u32, 7);
        let grain = ((u * 23.0 + along * 3.0 + tone * 20.0).sin() * 0.5 + 0.5) * 0.45
            + ((u * 71.0 + tone * 9.0).sin() * 0.5 + 0.5) * 0.15;
        let base = mix([0.40, 0.23, 0.12], [0.27, 0.15, 0.08], grain);
        let base = mix(base, [0.46, 0.28, 0.15], tone * 0.35);
        // A knot: a darker ring somewhere on the board.
        let (kx, ky) = (0.2 + tone * 0.6, 0.5);
        let knot = 1.0 - (((u - kx) * 3.0).hypot(along - ky) * 5.0).clamp(0.0, 1.0);
        let base = mix(base, [0.15, 0.08, 0.04], knot * 0.55);
        texel(mix(base, [0.07, 0.04, 0.02], groove * 0.85))
    })
}

/// Dark, slightly rusty iron for the straps.
fn iron_texture() -> TextureAsset {
    TextureAsset::generated("chest_iron", 64, 64, TextureFormat::Rgba8Srgb, |u, v| {
        let pits = hash01((u * 24.0) as u32, (v * 24.0) as u32);
        let fine = hash01((u * 64.0) as u32, (v * 64.0) as u32) * 0.2;
        let rust = hash01((u * 6.0) as u32 + 3, (v * 6.0) as u32 + 11);
        let base = mix([0.20, 0.20, 0.22], [0.09, 0.09, 0.11], pits * 0.8 + fine);
        texel(mix(base, [0.30, 0.17, 0.10], f32::from(rust > 0.82) * 0.35))
    })
}

fn wood_material() -> MaterialAsset {
    MaterialAsset::colored("chest_wood", 1.0, 1.0, 1.0)
        .with_roughness(0.82)
        .with_base_color_texture(0)
}

fn iron_material() -> MaterialAsset {
    MaterialAsset::colored("chest_iron", 0.85, 0.85, 0.9)
        .with_roughness(0.55)
        .with_metallic(0.8)
        .with_base_color_texture(1)
}

/// Polished gold with the faintest glow of its own, so it holds up in the shade of a crypt.
fn gold_material() -> MaterialAsset {
    MaterialAsset::colored("chest_gold", 0.96, 0.72, 0.22)
        .with_roughness(0.28)
        .with_metallic(0.95)
        .with_emissive(Vec3::new(0.10, 0.06, 0.01))
}

fn lining_material() -> MaterialAsset {
    MaterialAsset::colored("chest_lining", 0.50, 0.07, 0.10).with_roughness(0.95)
}

/// The four materials the body and the lid share, in [`part`] order.
fn shell_materials() -> Vec<MaterialAsset> {
    vec![
        wood_material(),
        iron_material(),
        gold_material(),
        lining_material(),
    ]
}

/// Appends an axis-aligned box with UVs planar in metres at [`WOOD_TILE`] to the repeat.
fn block(builder: &mut MeshBuilder, min: Vec3, max: Vec3) {
    let scale = 1.0 / WOOD_TILE;
    let uv = |a: f32, b: f32| Vec2::new(a * scale, -b * scale);
    let (x0, y0, z0) = (min.x, min.y, min.z);
    let (x1, y1, z1) = (max.x, max.y, max.z);
    // (normal, four corners wound counter-clockwise seen from outside).
    let faces: [(Vec3, [Vec3; 4]); 6] = [
        (
            Vec3::X,
            [
                Vec3::new(x1, y0, z1),
                Vec3::new(x1, y0, z0),
                Vec3::new(x1, y1, z0),
                Vec3::new(x1, y1, z1),
            ],
        ),
        (
            -Vec3::X,
            [
                Vec3::new(x0, y0, z0),
                Vec3::new(x0, y0, z1),
                Vec3::new(x0, y1, z1),
                Vec3::new(x0, y1, z0),
            ],
        ),
        (
            Vec3::Z,
            [
                Vec3::new(x0, y0, z1),
                Vec3::new(x1, y0, z1),
                Vec3::new(x1, y1, z1),
                Vec3::new(x0, y1, z1),
            ],
        ),
        (
            -Vec3::Z,
            [
                Vec3::new(x1, y0, z0),
                Vec3::new(x0, y0, z0),
                Vec3::new(x0, y1, z0),
                Vec3::new(x1, y1, z0),
            ],
        ),
        (
            Vec3::Y,
            [
                Vec3::new(x0, y1, z0),
                Vec3::new(x0, y1, z1),
                Vec3::new(x1, y1, z1),
                Vec3::new(x1, y1, z0),
            ],
        ),
        (
            -Vec3::Y,
            [
                Vec3::new(x0, y0, z0),
                Vec3::new(x1, y0, z0),
                Vec3::new(x1, y0, z1),
                Vec3::new(x0, y0, z1),
            ],
        ),
    ];
    for (normal, corners) in faces {
        let uvs = corners.map(|corner| {
            if normal.x.abs() > 0.5 {
                uv(corner.z, corner.y)
            } else if normal.z.abs() > 0.5 {
                uv(corner.x, corner.y)
            } else {
                uv(corner.x, corner.z)
            }
        });
        builder.quad(corners, normal, uvs);
    }
}

/// A box centred on `centre` with half-extents `half`.
fn cube(builder: &mut MeshBuilder, centre: Vec3, half: Vec3) {
    block(builder, centre - half, centre + half);
}

/// One point of the lid's arch profile: where it is in the YZ plane and which way it faces.
#[derive(Debug, Clone, Copy)]
struct Rib {
    /// Z, local to the hinge.
    z: f32,
    /// Y, above the rim.
    y: f32,
    /// Outward normal's Z.
    nz: f32,
    /// Outward normal's Y.
    ny: f32,
    /// Distance along the arch from the back, in metres, for texture V.
    along: f32,
}

/// The arch's profile for a half-ellipse of half-depth `a` and rise `b`, `grow` metres out.
///
/// The ellipse is centred on `z = -HALF_D` - the middle of the body below - so the lid sits
/// squarely on it. `grow` offsets each point along its normal, which is how a band lies just
/// proud of the shell and how the inner lining lies just under it.
fn arch(a: f32, b: f32, grow: f32) -> Vec<Rib> {
    let mut ribs = Vec::with_capacity(ARCH_SEGMENTS + 1);
    let mut along = 0.0;
    let mut previous: Option<(f32, f32)> = None;
    for step in 0..=ARCH_SEGMENTS {
        let t = step as f32 / ARCH_SEGMENTS as f32 * core::f32::consts::PI;
        let (sin, cos) = t.sin_cos();
        // Outward normal of an ellipse: the gradient, (cos/a, sin/b).
        let normal = Vec2::new(cos / a, sin / b);
        let normal = normal / normal.length().max(1e-6);
        let (z, y) = (
            -HALF_D + a * cos + normal.x * grow,
            b * sin + normal.y * grow,
        );
        if let Some((pz, py)) = previous {
            along += (z - pz).hypot(y - py);
        }
        previous = Some((z, y));
        ribs.push(Rib {
            z,
            y,
            nz: normal.x,
            ny: normal.y,
            along,
        });
    }
    ribs
}

/// Sweeps `ribs` across `x0..x1` as a smooth strip. `inward` flips the winding and normals,
/// for the surface seen from underneath.
fn sweep(builder: &mut MeshBuilder, ribs: &[Rib], x0: f32, x1: f32, inward: bool) {
    let sign = if inward { -1.0 } else { 1.0 };
    let tile = 1.0 / WOOD_TILE;
    let mut previous: Option<(u32, u32)> = None;
    for rib in ribs {
        let normal = Vec3::new(0.0, rib.ny * sign, rib.nz * sign);
        let left = builder.vertex(
            Vec3::new(x0, rib.y, rib.z),
            normal,
            Vec2::new(x0 * tile, -rib.along * tile),
        );
        let right = builder.vertex(
            Vec3::new(x1, rib.y, rib.z),
            normal,
            Vec2::new(x1 * tile, -rib.along * tile),
        );
        if let Some((back_left, back_right)) = previous {
            if inward {
                builder.triangle(back_left, right, back_right);
                builder.triangle(back_left, left, right);
            } else {
                builder.triangle(back_left, back_right, right);
                builder.triangle(back_left, right, left);
            }
        }
        previous = Some((left, right));
    }
}

/// Fills the half-ellipse of `ribs` in the plane `x`, facing `+X` when `facing_positive`.
fn end_cap(builder: &mut MeshBuilder, ribs: &[Rib], x: f32, facing_positive: bool) {
    let normal = if facing_positive { Vec3::X } else { -Vec3::X };
    let tile = 1.0 / WOOD_TILE;
    let uv = |z: f32, y: f32| Vec2::new(z * tile, -y * tile);
    let centre = builder.vertex(Vec3::new(x, 0.0, -HALF_D), normal, uv(-HALF_D, 0.0));
    let mut previous: Option<u32> = None;
    for rib in ribs {
        let index = builder.vertex(Vec3::new(x, rib.y, rib.z), normal, uv(rib.z, rib.y));
        if let Some(back) = previous {
            // The ring runs back to front, which seen from +X is left to right over the
            // top: clockwise. The +X face winds the other way round.
            if facing_positive {
                builder.triangle(centre, index, back);
            } else {
                builder.triangle(centre, back, index);
            }
        }
        previous = Some(index);
    }
}

/// The body: plinth, four walls, floor, iron straps and posts, the gold lock plate.
///
/// Open at the top with a red lining at `TREASURE_BASE.y`, so a raised lid shows a filled box.
#[must_use]
pub fn chest_body() -> ModelAsset {
    let mut wood = MeshBuilder::new();
    let mut iron = MeshBuilder::new();
    let mut gold = MeshBuilder::new();
    let mut lining = MeshBuilder::new();

    let h = BODY_HEIGHT;
    // The plinth, a little wider than the walls so the box stands on a base.
    block(
        &mut wood,
        Vec3::new(-HALF_W - 0.008, 0.0, -HALF_D - 0.008),
        Vec3::new(HALF_W + 0.008, 0.07, HALF_D + 0.008),
    );
    // Four walls.
    block(
        &mut wood,
        Vec3::new(-HALF_W, 0.07, -HALF_D),
        Vec3::new(HALF_W, h, -HALF_D + WALL),
    );
    block(
        &mut wood,
        Vec3::new(-HALF_W, 0.07, HALF_D - WALL),
        Vec3::new(HALF_W, h, HALF_D),
    );
    block(
        &mut wood,
        Vec3::new(-HALF_W, 0.07, -HALF_D + WALL),
        Vec3::new(-HALF_W + WALL, h, HALF_D - WALL),
    );
    block(
        &mut wood,
        Vec3::new(HALF_W - WALL, 0.07, -HALF_D + WALL),
        Vec3::new(HALF_W, h, HALF_D - WALL),
    );
    // The cushion inside: a solid block, its top is where the treasure sits.
    lining_block(
        &mut lining,
        Vec3::new(-HALF_W + WALL, 0.07, -HALF_D + WALL),
        Vec3::new(HALF_W - WALL, TREASURE_BASE.y, HALF_D - WALL),
    );

    // Iron: a rim frame, straps and corner posts, all a centimetre or so proud of the wood.
    let proud = 0.012;
    let rim_low = h - 0.04;
    block(
        &mut iron,
        Vec3::new(-HALF_W - proud, rim_low, -HALF_D - proud),
        Vec3::new(HALF_W + proud, h + 0.006, -HALF_D + WALL * 0.6),
    );
    block(
        &mut iron,
        Vec3::new(-HALF_W - proud, rim_low, HALF_D - WALL * 0.6),
        Vec3::new(HALF_W + proud, h + 0.006, HALF_D + proud),
    );
    block(
        &mut iron,
        Vec3::new(-HALF_W - proud, rim_low, -HALF_D + WALL * 0.6),
        Vec3::new(-HALF_W + WALL * 0.6, h + 0.006, HALF_D - WALL * 0.6),
    );
    block(
        &mut iron,
        Vec3::new(HALF_W - WALL * 0.6, rim_low, -HALF_D + WALL * 0.6),
        Vec3::new(HALF_W + proud, h + 0.006, HALF_D - WALL * 0.6),
    );
    for x in [-0.38_f32, 0.38] {
        for side in [-1.0_f32, 1.0] {
            let z = side * (HALF_D + proud * 0.5);
            block(
                &mut iron,
                Vec3::new(x - 0.045, 0.0, z.min(side * (HALF_D - 0.004)) - 0.0),
                Vec3::new(x + 0.045, h, z.max(side * (HALF_D - 0.004))),
            );
            // Rivets on the strap: gold heads at the plinth and under the rim.
            for y in [0.13_f32, 0.30] {
                cube(
                    &mut gold,
                    Vec3::new(x, y, side * (HALF_D + proud + 0.004)),
                    Vec3::new(0.016, 0.016, 0.006),
                );
            }
        }
    }
    for side in [-1.0_f32, 1.0] {
        let x = side * (HALF_W + proud * 0.5);
        for z in [-0.2_f32, 0.2] {
            block(
                &mut iron,
                Vec3::new(x.min(side * (HALF_W - 0.004)), 0.0, z - 0.04),
                Vec3::new(x.max(side * (HALF_W - 0.004)), h, z + 0.04),
            );
        }
    }
    // Corner posts, with a gold cap on each.
    for sx in [-1.0_f32, 1.0] {
        for sz in [-1.0_f32, 1.0] {
            let centre = Vec3::new(sx * (HALF_W - 0.012), h * 0.5, sz * (HALF_D - 0.012));
            cube(&mut iron, centre, Vec3::new(0.036, h * 0.5, 0.036));
            cube(
                &mut gold,
                Vec3::new(centre.x, h + 0.014, centre.z),
                Vec3::new(0.042, 0.016, 0.042),
            );
            cube(
                &mut gold,
                Vec3::new(centre.x, 0.026, centre.z),
                Vec3::new(0.046, 0.02, 0.046),
            );
        }
    }

    // Gold: a trim line on top of the plinth, along the front, and the lock plate with its
    // keyhole on the face the lid's hasp comes down to.
    block(
        &mut gold,
        Vec3::new(-HALF_W, 0.07, -HALF_D - 0.012),
        Vec3::new(HALF_W, 0.088, -HALF_D),
    );
    block(
        &mut gold,
        Vec3::new(-0.10, 0.25, -HALF_D - 0.02),
        Vec3::new(0.10, 0.40, -HALF_D),
    );
    block(
        &mut gold,
        Vec3::new(-0.115, 0.235, -HALF_D - 0.012),
        Vec3::new(0.115, 0.25, -HALF_D),
    );
    // The keyhole, in iron: a round-ish head over a slot.
    cube(
        &mut iron,
        Vec3::new(0.0, 0.335, -HALF_D - 0.022),
        Vec3::new(0.018, 0.018, 0.004),
    );
    block(
        &mut iron,
        Vec3::new(-0.008, 0.28, -HALF_D - 0.022),
        Vec3::new(0.008, 0.335, -HALF_D),
    );

    let submeshes = vec![
        wood.into_submesh("body_wood", part::WOOD),
        iron.into_submesh("body_iron", part::IRON),
        gold.into_submesh("body_gold", part::GOLD),
        lining.into_submesh("body_lining", part::LINING),
    ];
    ModelAsset::rigid(
        "chest_body",
        submeshes,
        shell_materials(),
        vec![wood_texture(), iron_texture()],
    )
}

/// A box in the lining material. Same geometry as [`block`]; a separate name because it goes
/// into a different builder and reads as such at the call site.
fn lining_block(builder: &mut MeshBuilder, min: Vec3, max: Vec3) {
    block(builder, min, max);
}

/// The lid: a barrel vault with iron straps, a gold hasp and crest, and a red underside.
///
/// Its origin is the hinge. At rest it spans `z = -DEPTH..0` and rises `LID_RISE` above
/// `y = 0`; rotating it about +X by an angle lifts the front edge.
#[must_use]
pub fn chest_lid() -> ModelAsset {
    let mut wood = MeshBuilder::new();
    let mut iron = MeshBuilder::new();
    let mut gold = MeshBuilder::new();
    let mut lining = MeshBuilder::new();

    let a = HALF_D + OVERHANG;
    let b = LID_RISE;
    let half_w = HALF_W + OVERHANG;
    let outer = arch(a, b, 0.0);
    let inner = arch(a - SHELL, b - SHELL * 0.8, 0.0);
    let inner_half_w = half_w - SHELL;

    // Outer shell and its two end caps.
    sweep(&mut wood, &outer, -half_w, half_w, false);
    end_cap(&mut wood, &outer, half_w, true);
    end_cap(&mut wood, &outer, -half_w, false);
    // Inner shell and the faces of the end caps seen from inside.
    sweep(&mut lining, &inner, -inner_half_w, inner_half_w, true);
    end_cap(&mut lining, &inner, inner_half_w, false);
    end_cap(&mut lining, &inner, -inner_half_w, true);

    // The lip: the shell's bottom edge, front and back, so there is no gap between the outer
    // and inner surfaces where the lid meets the rim.
    let lip = |builder: &mut MeshBuilder, outer_z: f32, inner_z: f32| {
        builder.quad(
            [
                Vec3::new(-half_w, 0.0, inner_z),
                Vec3::new(half_w, 0.0, inner_z),
                Vec3::new(half_w, 0.0, outer_z),
                Vec3::new(-half_w, 0.0, outer_z),
            ],
            -Vec3::Y,
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(2.4, 0.0),
                Vec2::new(2.4, 0.1),
                Vec2::new(0.0, 0.1),
            ],
        );
    };
    let back = &outer[0];
    let front = &outer[ARCH_SEGMENTS];
    lip(&mut wood, back.z, inner[0].z);
    wood.quad(
        [
            Vec3::new(-half_w, 0.0, front.z),
            Vec3::new(half_w, 0.0, front.z),
            Vec3::new(half_w, 0.0, inner[ARCH_SEGMENTS].z),
            Vec3::new(-half_w, 0.0, inner[ARCH_SEGMENTS].z),
        ],
        -Vec3::Y,
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.4, 0.0),
            Vec2::new(2.4, 0.1),
            Vec2::new(0.0, 0.1),
        ],
    );
    // Under each end cap, the strip of shell between the outer and inner faces.
    for (x0, x1) in [(-half_w, -inner_half_w), (inner_half_w, half_w)] {
        wood.quad(
            [
                Vec3::new(x0, 0.0, front.z),
                Vec3::new(x1, 0.0, front.z),
                Vec3::new(x1, 0.0, back.z),
                Vec3::new(x0, 0.0, back.z),
            ],
            -Vec3::Y,
            [
                Vec2::new(0.0, 0.0),
                Vec2::new(0.1, 0.0),
                Vec2::new(0.1, 1.6),
                Vec2::new(0.0, 1.6),
            ],
        );
    }

    // Straps across the vault, standing off the shell by a centimetre.
    let band = arch(a, b, 0.012);
    for x in [-0.38_f32, 0.38, -0.60, 0.60] {
        let width = if x.abs() > 0.5 { 0.03 } else { 0.045 };
        sweep(&mut iron, &band, x - width, x + width, false);
    }
    // The hasp: a gold tongue down the front of the vault to the lock, and a crest at the
    // crown. Cut from the arch between the crown and the front edge.
    let hasp_ribs: Vec<Rib> = arch(a, b, 0.018)
        .into_iter()
        .filter(|rib| rib.z < -HALF_D - 0.05)
        .collect();
    sweep(&mut gold, &hasp_ribs, -0.075, 0.075, false);
    let crown = arch(a, b, 0.024);
    let (crest_lo, crest_hi) = (ARCH_SEGMENTS / 2 - 1, ARCH_SEGMENTS / 2 + 1);
    sweep(&mut gold, &crown[crest_lo..=crest_hi], -0.12, 0.12, false);
    // A gold lip along the front edge of the lid.
    block(
        &mut gold,
        Vec3::new(-half_w, 0.0, front.z - 0.012),
        Vec3::new(half_w, 0.035, front.z + 0.016),
    );
    // Rivets along the straps.
    for x in [-0.38_f32, 0.38] {
        for step in [3_usize, 6, 9, 11] {
            let rib = band[step];
            cube(
                &mut gold,
                Vec3::new(x, rib.y + rib.ny * 0.008, rib.z + rib.nz * 0.008),
                Vec3::new(0.015, 0.015, 0.015),
            );
        }
    }

    let submeshes = vec![
        wood.into_submesh("lid_wood", part::WOOD),
        iron.into_submesh("lid_iron", part::IRON),
        gold.into_submesh("lid_gold", part::GOLD),
        lining.into_submesh("lid_lining", part::LINING),
    ];
    ModelAsset::rigid(
        "chest_lid",
        submeshes,
        shell_materials(),
        vec![wood_texture(), iron_texture()],
    )
}

/// The treasure: a heap of coins and a few gems, authored on its own origin.
///
/// Hidden while the chest is closed and scaled up from nothing as the lid opens, so it appears
/// to rise out of the box. Both materials are emissive; presentation adds to that as the glow
/// builds and fades it to a lingering shimmer.
#[must_use]
pub fn chest_treasure() -> ModelAsset {
    let mut coins = MeshBuilder::new();
    let mut gems = MeshBuilder::new();

    // The mound, in three tapering layers.
    for (low, high, lower, upper) in [
        (
            0.0_f32,
            0.06_f32,
            Vec2::new(0.48, 0.28),
            Vec2::new(0.44, 0.25),
        ),
        (0.06, 0.12, Vec2::new(0.40, 0.24), Vec2::new(0.30, 0.19)),
        (0.12, 0.17, Vec2::new(0.26, 0.16), Vec2::new(0.12, 0.09)),
    ] {
        coins.prism(
            &Prism::new(
                Vec3::new(0.0, low, 0.0),
                Vec3::new(0.0, high, 0.0),
                1.0,
                1.0,
            )
            .with_halves(lower, upper),
        );
    }
    // Loose coins: thin tilted slabs scattered over the heap.
    for index in 0..22_u32 {
        let r0 = hash01(index, 1);
        let r1 = hash01(index, 2);
        let r2 = hash01(index, 3);
        let r3 = hash01(index, 4);
        let x = (r0 - 0.5) * 0.78;
        let z = (r1 - 0.5) * 0.44;
        // Higher in the middle of the heap, lower at its edges.
        let falloff = 1.0 - ((x / 0.42).powi(2) + (z / 0.26).powi(2)).min(1.0);
        let y = 0.05 + 0.13 * falloff;
        let tilt = Vec3::new((r2 - 0.5) * 0.07, 0.018, (r3 - 0.5) * 0.07);
        coins.prism(&Prism::new(
            Vec3::new(x, y, z),
            Vec3::new(x, y, z) + tilt,
            0.036,
            0.036,
        ));
    }
    // Gems: three tapered crystals poking out of the top.
    for (x, z, lean, height) in [
        (-0.14_f32, 0.02_f32, -0.04_f32, 0.15_f32),
        (0.12, -0.05, 0.05, 0.12),
        (0.02, 0.08, 0.0, 0.10),
    ] {
        gems.prism(&Prism::new(
            Vec3::new(x, 0.12, z),
            Vec3::new(x + lean, 0.12 + height, z),
            0.035,
            0.006,
        ));
    }

    let submeshes = vec![
        coins.into_submesh("treasure_coins", part::COINS),
        gems.into_submesh("treasure_gems", part::GEMS),
    ];
    ModelAsset::rigid(
        "chest_treasure",
        submeshes,
        vec![
            MaterialAsset::colored("chest_coins", 0.98, 0.76, 0.24)
                .with_roughness(0.25)
                .with_metallic(0.95)
                .with_emissive(Vec3::new(0.75, 0.48, 0.07)),
            MaterialAsset::colored("chest_gems", 0.55, 0.85, 1.0)
                .with_roughness(0.1)
                .with_metallic(0.2)
                .with_emissive(Vec3::new(0.25, 0.55, 0.85)),
        ],
        vec![],
    )
}

/// Every submesh of a model's finest level, for tests and for tools.
#[must_use]
pub fn submeshes_of(model: &ModelAsset) -> &[SubMesh] {
    model.lod0().map_or(&[], |lod| lod.submeshes.as_slice())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::validate::{ValidationLimits, validate_model};

    fn models() -> [ModelAsset; 3] {
        [chest_body(), chest_lid(), chest_treasure()]
    }

    #[test]
    fn every_chest_model_validates_and_is_rigid() {
        for model in models() {
            validate_model(&model, &ValidationLimits::default())
                .unwrap_or_else(|error| panic!("{} must validate: {error:?}", model.name));
            assert!(!model.is_skinned(), "{} has bones", model.name);
        }
    }

    #[test]
    fn the_parts_come_out_in_the_order_presentation_styles_them() {
        for model in [chest_body(), chest_lid()] {
            let names: Vec<_> = model.materials.iter().map(|m| m.name.as_str()).collect();
            assert_eq!(
                names,
                ["chest_wood", "chest_iron", "chest_gold", "chest_lining"]
            );
            let submeshes = submeshes_of(&model);
            assert_eq!(submeshes.len(), 4);
            for (index, submesh) in submeshes.iter().enumerate() {
                assert_eq!(submesh.material, index, "{}", model.name);
                assert!(
                    submesh.triangle_count() > 0,
                    "{}[{index}] is empty",
                    model.name
                );
            }
        }
        let treasure = chest_treasure();
        assert_eq!(submeshes_of(&treasure)[part::COINS].material, part::COINS);
        assert_eq!(submeshes_of(&treasure)[part::GEMS].material, part::GEMS);
    }

    fn extent(model: &ModelAsset) -> (Vec3, Vec3) {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for position in submeshes_of(model)
            .iter()
            .flat_map(|submesh| submesh.geometry.positions())
        {
            min = min.min(position);
            max = max.max(position);
        }
        (min, max)
    }

    #[test]
    fn the_closed_chest_is_about_one_point_two_by_point_eight_by_three_quarters() {
        let (body_min, body_max) = extent(&chest_body());
        let (lid_min, lid_max) = extent(&chest_lid());
        let top = LID_HINGE.y + lid_max.y;
        let width = body_max.x.max(lid_max.x) - body_min.x.min(lid_min.x);
        let depth = (body_max.z - body_min.z).max(lid_max.z - lid_min.z);
        assert!((1.2..1.27).contains(&width), "width {width}");
        assert!((0.8..0.9).contains(&depth), "depth {depth}");
        assert!((0.72..0.80).contains(&top), "height {top}");
        assert!(
            body_min.y.abs() < 1e-4,
            "the body does not stand on its origin"
        );
    }

    #[test]
    fn the_lid_hangs_from_its_hinge() {
        // Origin on the hinge: the lid's back edge is at z ~ 0 and it runs forward, -Z, to
        // the front of the chest, so a rotation about +X lifts the front.
        let (min, max) = extent(&chest_lid());
        assert!(
            max.z.abs() < 0.05,
            "the back of the lid is {} from the hinge",
            max.z
        );
        assert!(
            min.z < -0.78 && min.z > -0.9,
            "the front of the lid is at {}",
            min.z
        );
        assert!(min.y >= -1e-4 && max.y > 0.25);
    }

    #[test]
    fn every_triangle_is_wound_the_way_its_normal_faces() {
        // A reversed strip draws the inside of the lid and lights it from the wrong side,
        // which looks nearly right until the lid is open.
        for model in models() {
            for submesh in submeshes_of(&model) {
                let vertices: Vec<_> = submesh.geometry.positions().collect();
                let normals: Vec<_> = submesh.geometry.normals().collect();
                for triangle in submesh.indices.chunks_exact(3) {
                    let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|i| i as usize);
                    let facing = (vertices[b] - vertices[a]).cross(vertices[c] - vertices[b]);
                    if facing.length_squared() < 1e-12 {
                        continue;
                    }
                    let average = normals[a] + normals[b] + normals[c];
                    assert!(
                        facing.dot(average) > 0.0,
                        "{} / {} has a triangle wound against its normal: {a} {b} {c}",
                        model.name,
                        submesh.name
                    );
                }
            }
        }
    }

    #[test]
    fn the_chest_is_cheap_enough_to_be_a_prop() {
        let triangles: usize = models().iter().map(ModelAsset::triangle_count).sum();
        assert!(triangles < 2500, "{triangles} triangles");
    }
}
