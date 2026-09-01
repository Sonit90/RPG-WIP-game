//! The starter library: blockout geometry a level can be built out of on the day the project
//! has no art.
//!
//! ```text
//!  rocks       rock_small  rock_medium  boulder_large  rock_spire
//!  cliffs      cliff_straight  cliff_corner  cliff_end  cliff_slope
//!              cliff_overhang  cliff_arch
//!  ground      ground_tile  platform_square  ramp_square
//!  props       stone_cube  pillar  arch  crate_wooden
//!  vegetation  bush_round  tree_simple
//!  caves       cave_entrance  cave_tunnel_straight  cave_tunnel_curve  cave_room_small
//!  water       water_plane
//! ```
//!
//! Four cave pieces, not seven: an entrance, a straight, a turn and a room are what it takes
//! to build a cave that can be walked into, through and out of, and that is what the modular
//! set has to prove before a junction and a second room size are worth authoring.
//!
//! # What these are, and what they are not
//!
//! They are *blockout* pieces: boxes, wedges and prisms with generated textures on them, at
//! real sizes, so a level can be laid out, walked through and playtested before an artist has
//! delivered anything. They are not art, and nothing here is trying to be.
//!
//! They are also not a special kind of asset. Each one is written to disk as a `.glb` by the
//! asset generator and read back by the ordinary importer, so what the editor lists and what
//! the game loads is a file, exactly as it will be for a sculpted rock. Replacing one is
//! dropping a file with the same name over it — see [`crate::gltf_export`].
//!
//! # Why the cave and cliff pieces are hollow
//!
//! The requirement that decides the shape of this library is that a player must be able to
//! walk *underneath* an overhang and *through* a tunnel. A heightmap cannot express either,
//! and neither can a solid box. So the overhang is a column with a slab cantilevered off the
//! top of it, and every tunnel piece is a floor, two walls and a roof around an empty middle:
//! the cavity is real geometry, and the collision built for it in the level is a description
//! of that cavity rather than of the block it was cut from.
//!
//! # Scale
//!
//! One grid unit is one metre, and the modular pieces are authored on a **4 m** module so
//! they tile against each other at the editor's 4 m grid setting. A character is 1.8 m; a
//! tunnel is 3 m of headroom, which reads as tight but passable.

use engine_core::math::{Vec2, Vec3};

use crate::material::MaterialAsset;
use crate::mesh::SubMesh;
use crate::model::ModelAsset;
use crate::procedural::{MeshBuilder, Prism};
use crate::texture::{TextureAsset, TextureFormat};

/// The module the cliff, ground and cave pieces are authored on, in metres.
pub const MODULE: f32 = 4.0;

/// How much headroom a cave tunnel has, in metres.
///
/// A character is 1.8 m and the controller needs room to step; 3 m reads as a tunnel rather
/// than as a corridor and still lets the camera sit behind the player's head.
pub const TUNNEL_HEIGHT: f32 = 3.0;

/// How wide the cavity of a tunnel piece is, in metres.
pub const TUNNEL_WIDTH: f32 = 3.0;

/// How thick the wall, floor and roof of a cave piece are.
pub const SHELL: f32 = 0.5;

/// One entry in the starter library.
#[derive(Debug, Clone, Copy)]
pub struct StarterAsset {
    /// The file's stem: `"rock_small"`. Also the model's name.
    pub name: &'static str,
    /// Where it is written, under `assets/generated/`: `"environment/rocks"`.
    pub folder: &'static str,
    /// Builds it.
    pub build: fn() -> ModelAsset,
}

impl StarterAsset {
    /// The path this asset is written to, relative to the assets root.
    #[must_use]
    pub fn relative_path(&self) -> String {
        format!("generated/{}/{}.glb", self.folder, self.name)
    }
}

/// Every starter model, in the order the generator writes them.
pub const STARTER_LIBRARY: [StarterAsset; 24] = [
    StarterAsset { name: "rock_small", folder: "environment/rocks", build: rock_small },
    StarterAsset { name: "rock_medium", folder: "environment/rocks", build: rock_medium },
    StarterAsset { name: "boulder_large", folder: "environment/rocks", build: boulder_large },
    StarterAsset { name: "rock_spire", folder: "environment/rocks", build: rock_spire },
    StarterAsset { name: "cliff_straight", folder: "environment/cliffs", build: cliff_straight },
    StarterAsset { name: "cliff_corner", folder: "environment/cliffs", build: cliff_corner },
    StarterAsset { name: "cliff_end", folder: "environment/cliffs", build: cliff_end },
    StarterAsset { name: "cliff_slope", folder: "environment/cliffs", build: cliff_slope },
    StarterAsset { name: "cliff_overhang", folder: "environment/cliffs", build: cliff_overhang },
    StarterAsset { name: "cliff_arch", folder: "environment/cliffs", build: cliff_arch },
    StarterAsset { name: "ground_tile", folder: "environment/ground", build: ground_tile },
    StarterAsset { name: "platform_square", folder: "environment/ground", build: platform_square },
    StarterAsset { name: "ramp_square", folder: "environment/ground", build: ramp_square },
    StarterAsset { name: "stone_cube", folder: "environment/props", build: stone_cube },
    StarterAsset { name: "pillar", folder: "environment/props", build: pillar },
    StarterAsset { name: "arch", folder: "environment/props", build: arch },
    StarterAsset { name: "crate_wooden", folder: "environment/props", build: crate_wooden },
    StarterAsset { name: "bush_round", folder: "environment/vegetation", build: bush_round },
    StarterAsset { name: "tree_simple", folder: "environment/vegetation", build: tree_simple },
    StarterAsset { name: "cave_entrance", folder: "environment/caves", build: cave_entrance },
    StarterAsset {
        name: "cave_tunnel_straight",
        folder: "environment/caves",
        build: cave_tunnel_straight,
    },
    StarterAsset { name: "cave_tunnel_curve", folder: "environment/caves", build: cave_tunnel_curve },
    StarterAsset { name: "cave_room_small", folder: "environment/caves", build: cave_room_small },
    StarterAsset { name: "water_plane", folder: "environment/water", build: water_plane },
];

/// The starter model called `name`, if the library has one.
#[must_use]
pub fn starter_model(name: &str) -> Option<ModelAsset> {
    STARTER_LIBRARY
        .iter()
        .find(|asset| asset.name == name)
        .map(|asset| (asset.build)())
}

// ---------------------------------------------------------------------------------------
// Materials and textures
// ---------------------------------------------------------------------------------------

/// One entry in the starter *material* library.
#[derive(Debug, Clone, Copy)]
pub struct StarterMaterial {
    /// What it is called: `"rock"`. Also the texture file's stem.
    pub name: &'static str,
    /// Average colour, before the noise is laid over it.
    pub base: [f32; 3],
    /// The colour the noise moves towards.
    pub accent: [f32; 3],
    /// How rough the surface is.
    pub roughness: f32,
    /// How metallic it is. Everything here is dielectric.
    pub metallic: f32,
    /// How coarse the noise is, in texels per cell.
    pub grain: u32,
    /// Opacity. Below one makes a transparent surface — water, and nothing else.
    pub opacity: f32,
}

/// Every starter material.
///
/// The seven the brief asks for, and they are the *only* materials the starter models use:
/// a blockout library with a bespoke material per asset would be a library whose look cannot
/// be changed in one place.
pub const STARTER_MATERIALS: [StarterMaterial; 7] = [
    StarterMaterial {
        name: "grass",
        base: [0.24, 0.38, 0.18],
        accent: [0.36, 0.52, 0.24],
        roughness: 0.92,
        metallic: 0.0,
        grain: 2,
        opacity: 1.0,
    },
    StarterMaterial {
        name: "dirt",
        base: [0.32, 0.24, 0.16],
        accent: [0.44, 0.34, 0.22],
        roughness: 0.95,
        metallic: 0.0,
        grain: 3,
        opacity: 1.0,
    },
    StarterMaterial {
        name: "rock",
        base: [0.42, 0.42, 0.44],
        accent: [0.55, 0.54, 0.52],
        roughness: 0.85,
        metallic: 0.0,
        grain: 4,
        opacity: 1.0,
    },
    StarterMaterial {
        name: "cliff",
        base: [0.34, 0.33, 0.36],
        accent: [0.50, 0.47, 0.45],
        roughness: 0.90,
        metallic: 0.0,
        grain: 8,
        opacity: 1.0,
    },
    StarterMaterial {
        name: "sand",
        base: [0.66, 0.58, 0.40],
        accent: [0.78, 0.70, 0.50],
        roughness: 0.95,
        metallic: 0.0,
        grain: 2,
        opacity: 1.0,
    },
    StarterMaterial {
        name: "water",
        base: [0.12, 0.28, 0.38],
        accent: [0.20, 0.44, 0.54],
        roughness: 0.12,
        metallic: 0.0,
        grain: 16,
        opacity: 0.55,
    },
    // The one material that is deliberately ugly: a checker an author can tell apart from
    // every other surface at a glance, for looking at UVs, scale and lighting.
    StarterMaterial {
        name: "simple_test",
        base: [0.85, 0.16, 0.62],
        accent: [0.10, 0.10, 0.12],
        roughness: 0.60,
        metallic: 0.0,
        grain: 0,
        opacity: 1.0,
    },
];

/// The side of every generated texture, in texels.
///
/// 128 is what the existing environment library uses, and a blockout texture has no detail
/// that a larger one would carry.
pub const TEXTURE_SIZE: u32 = 128;

/// The starter material called `name`, if there is one.
#[must_use]
pub fn starter_material(name: &str) -> Option<StarterMaterial> {
    STARTER_MATERIALS
        .iter()
        .copied()
        .find(|material| material.name == name)
}

impl StarterMaterial {
    /// The generated image for this material.
    ///
    /// Deterministic, from the same integer hash the rest of the project's procedural content
    /// uses: the same texture on every machine and every run, which is what makes a screenshot
    /// comparable with last week's.
    ///
    /// # Panics
    ///
    /// Never. The dimensions and the texel count are computed together.
    #[must_use]
    pub fn texture(&self) -> TextureAsset {
        let mut texels = Vec::with_capacity((TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize);
        for y in 0..TEXTURE_SIZE {
            for x in 0..TEXTURE_SIZE {
                let t = if self.grain == 0 {
                    // The test material: an eight-by-eight checker rather than noise.
                    let cell = (x / 16 + y / 16) % 2;
                    cell as f32
                } else {
                    let cell = self.grain.max(1);
                    let value = hash01(x / cell, y / cell);
                    // A second, finer octave, so a flat blockout surface still has something
                    // for the eye to judge distance by.
                    let fine = hash01(x, y);
                    (value * 0.75 + fine * 0.25).clamp(0.0, 1.0)
                };
                let color = mix(self.base, self.accent, t);
                texels.push(channel(color[0]));
                texels.push(channel(color[1]));
                texels.push(channel(color[2]));
                texels.push(channel(self.opacity));
            }
        }
        TextureAsset::new(
            self.name,
            TEXTURE_SIZE,
            TEXTURE_SIZE,
            TextureFormat::Rgba8Srgb,
            texels,
        )
        .unwrap_or_else(|_| TextureAsset::white())
    }

    /// The material asset, with its texture in slot 0 of the model that carries it.
    #[must_use]
    pub fn material(&self) -> MaterialAsset {
        let material = MaterialAsset::colored(self.name, 1.0, 1.0, 1.0)
            .with_roughness(self.roughness)
            .with_metallic(self.metallic)
            .with_base_color_texture(0);
        if self.opacity < 1.0 {
            material.with_opacity(self.opacity)
        } else {
            material
        }
    }
}

/// Deterministic value in `0..1` from a pair of integers.
fn hash01(x: u32, y: u32) -> f32 {
    let mut value = x.wrapping_mul(0x27D4_EB2D) ^ y.wrapping_mul(0x1656_67B1);
    value ^= value >> 15;
    value = value.wrapping_mul(0x2545_F491);
    value ^= value >> 13;
    (value & 0xFFFF) as f32 / 65_535.0
}

/// Linear interpolation between two colours.
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// A colour channel as a texel byte.
fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

// ---------------------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------------------

/// A box between two corners, added to `builder`.
///
/// Everything in this library that is not a wedge or a fan of leaves is one of these. A box
/// is a [`Prism`] with rectangular caps, so it costs the same twelve triangles every other
/// primitive in this crate costs and needs no second code path.
fn block(builder: &mut MeshBuilder, min: Vec3, max: Vec3) {
    let low = min.min(max);
    let high = min.max(max);
    let center_x = (low.x + high.x) * 0.5;
    let center_z = (low.z + high.z) * 0.5;
    let half = Vec2::new((high.x - low.x) * 0.5, (high.z - low.z) * 0.5);
    builder.prism(
        &Prism::new(
            Vec3::new(center_x, low.y, center_z),
            Vec3::new(center_x, high.y, center_z),
            1.0,
            1.0,
        )
        .with_halves(half, half),
    );
}

/// A box that tapers from one footprint to another: a wedge, a boulder lobe, a spire.
fn tapered(builder: &mut MeshBuilder, base: Vec3, top: Vec3, base_half: Vec2, top_half: Vec2) {
    builder.prism(&Prism::new(base, top, 1.0, 1.0).with_halves(base_half, top_half));
}

/// One model out of one builder, with one material.
fn single(name: &str, builder: MeshBuilder, material: &StarterMaterial) -> ModelAsset {
    let submesh = builder.into_submesh(name, 0);
    ModelAsset::rigid(
        name,
        vec![submesh],
        vec![material.material()],
        vec![material.texture()],
    )
}

/// One model out of two builders and two materials: a trunk and a canopy, a floor and a wall.
fn pair(
    name: &str,
    first: (MeshBuilder, &str, &StarterMaterial),
    second: (MeshBuilder, &str, &StarterMaterial),
) -> ModelAsset {
    let submeshes: Vec<SubMesh> = vec![
        first.0.into_submesh(first.1, 0),
        second.0.into_submesh(second.1, 1),
    ];
    let mut first_material = first.2.material();
    first_material.base_color_texture = Some(0);
    let mut second_material = second.2.material();
    second_material.base_color_texture = Some(1);
    ModelAsset::rigid(
        name,
        submeshes,
        vec![first_material, second_material],
        vec![first.2.texture(), second.2.texture()],
    )
}

/// The material a piece is made of, by name, falling back to rock.
fn material_named(name: &str) -> StarterMaterial {
    starter_material(name).unwrap_or(STARTER_MATERIALS[2])
}

// ---------------------------------------------------------------------------------------
// Rocks
// ---------------------------------------------------------------------------------------

/// A knee-high boulder: two lobes, 0.9 m across.
fn rock_small() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    tapered(
        &mut builder,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.05, 0.45, 0.03),
        Vec2::new(0.45, 0.40),
        Vec2::new(0.26, 0.22),
    );
    tapered(
        &mut builder,
        Vec3::new(0.22, 0.0, -0.16),
        Vec3::new(0.24, 0.26, -0.14),
        Vec2::new(0.24, 0.20),
        Vec2::new(0.12, 0.10),
    );
    single("rock_small", builder, &material_named("rock"))
}

/// A waist-high boulder, 1.6 m across.
fn rock_medium() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    tapered(
        &mut builder,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(-0.08, 0.95, 0.06),
        Vec2::new(0.80, 0.70),
        Vec2::new(0.42, 0.38),
    );
    tapered(
        &mut builder,
        Vec3::new(0.42, 0.0, 0.30),
        Vec3::new(0.46, 0.50, 0.32),
        Vec2::new(0.40, 0.34),
        Vec2::new(0.20, 0.18),
    );
    single("rock_medium", builder, &material_named("rock"))
}

/// A boulder taller than a character, 3 m across: cover, and a landmark.
fn boulder_large() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    tapered(
        &mut builder,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(-0.15, 2.10, 0.10),
        Vec2::new(1.50, 1.30),
        Vec2::new(0.75, 0.65),
    );
    tapered(
        &mut builder,
        Vec3::new(0.90, 0.0, 0.70),
        Vec3::new(0.95, 0.90, 0.72),
        Vec2::new(0.70, 0.60),
        Vec2::new(0.35, 0.30),
    );
    tapered(
        &mut builder,
        Vec3::new(-0.80, 0.0, -0.60),
        Vec3::new(-0.85, 0.60, -0.62),
        Vec2::new(0.55, 0.50),
        Vec2::new(0.28, 0.24),
    );
    single("boulder_large", builder, &material_named("rock"))
}

/// A leaning spire, 4.5 m tall.
fn rock_spire() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    tapered(
        &mut builder,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.55, 4.50, 0.30),
        Vec2::new(0.70, 0.60),
        Vec2::new(0.14, 0.12),
    );
    single("rock_spire", builder, &material_named("rock"))
}

// ---------------------------------------------------------------------------------------
// Cliffs
// ---------------------------------------------------------------------------------------

/// How tall one cliff module is.
const CLIFF_HEIGHT: f32 = 6.0;

/// One straight length of cliff face: 4 m wide, 6 m tall, 3 m deep.
///
/// The piece a run of cliff is made of. It stands on its origin and is centred on X, so
/// placing two of them 4 m apart on the editor's 4 m grid leaves no seam.
fn cliff_straight() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    block(
        &mut builder,
        Vec3::new(-MODULE * 0.5, 0.0, -3.0),
        Vec3::new(MODULE * 0.5, CLIFF_HEIGHT, 0.0),
    );
    // A ledge partway up, so a face reads as rock rather than as a wall.
    block(
        &mut builder,
        Vec3::new(-MODULE * 0.5, 3.4, -3.4),
        Vec3::new(MODULE * 0.5, 4.0, -3.0),
    );
    single("cliff_straight", builder, &material_named("cliff"))
}

/// The corner where two runs of cliff meet: an L in plan.
fn cliff_corner() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    block(
        &mut builder,
        Vec3::new(-MODULE * 0.5, 0.0, -3.0),
        Vec3::new(MODULE * 0.5, CLIFF_HEIGHT, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(-MODULE * 0.5, 0.0, -3.0),
        Vec3::new(-MODULE * 0.5 + 3.0, CLIFF_HEIGHT, MODULE * 0.5),
    );
    single("cliff_corner", builder, &material_named("cliff"))
}

/// Where a run of cliff stops: the same face, stepped down to nothing at one end.
fn cliff_end() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    block(
        &mut builder,
        Vec3::new(-MODULE * 0.5, 0.0, -3.0),
        Vec3::new(0.0, CLIFF_HEIGHT, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(0.0, 0.0, -3.0),
        Vec3::new(1.4, CLIFF_HEIGHT * 0.6, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(1.4, 0.0, -3.0),
        Vec3::new(MODULE * 0.5, CLIFF_HEIGHT * 0.25, 0.0),
    );
    single("cliff_end", builder, &material_named("cliff"))
}

/// A ramp up the height of one cliff module: the way onto the top of a cliff.
///
/// The slope is 6 m over 12 m, which is 27° — inside the character controller's 50° limit
/// with room to spare, so it is walkable rather than decorative.
fn cliff_slope() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE * 0.5;
    let top = CLIFF_HEIGHT;
    let run = 12.0;
    // The walking surface, and the two sides under it. A wedge rather than a box: the top
    // face is the thing a character stands on, and it has to be a slope.
    builder.quad(
        [
            Vec3::new(-half, 0.0, 0.0),
            Vec3::new(half, 0.0, 0.0),
            Vec3::new(half, top, -run),
            Vec3::new(-half, top, -run),
        ],
        Vec3::new(0.0, run, top).normalize(),
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 3.0),
            Vec2::new(0.0, 3.0),
        ],
    );
    // The block under the slope, so it is solid from every other angle.
    block(
        &mut builder,
        Vec3::new(-half, 0.0, -run),
        Vec3::new(half, top, -run + 1.0),
    );
    single("cliff_slope", builder, &material_named("cliff"))
}

/// A cliff a character can walk **underneath**: a back wall, and a slab cantilevered off it.
///
/// This is the piece the whole cliff system exists for. Nothing a heightmap can express has
/// space under it; this has 3.5 m of it, 4 m deep, with the roof starting at 3.5 m and the
/// supporting wall set back behind it.
fn cliff_overhang() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE * 0.5;
    // The wall at the back, from the ground up.
    block(
        &mut builder,
        Vec3::new(-half, 0.0, -1.0),
        Vec3::new(half, CLIFF_HEIGHT, 0.0),
    );
    // The slab that reaches out over the empty space in front of it.
    block(
        &mut builder,
        Vec3::new(-half, 3.5, -5.0),
        Vec3::new(half, 5.0, -1.0),
    );
    // A stub under the far corner, so the slab reads as rock rather than as a diving board.
    tapered(
        &mut builder,
        Vec3::new(-half, 3.5, -4.6),
        Vec3::new(-half + 0.2, 2.4, -4.4),
        Vec2::new(0.55, 0.55),
        Vec2::new(0.20, 0.20),
    );
    single("cliff_overhang", builder, &material_named("cliff"))
}

/// A natural arch: two legs and a span, with 4 m of headroom to walk through.
fn cliff_arch() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE;
    for side in [-1.0_f32, 1.0] {
        tapered(
            &mut builder,
            Vec3::new(side * half, 0.0, 0.0),
            Vec3::new(side * (half - 0.4), 4.0, 0.0),
            Vec2::new(1.0, 1.2),
            Vec2::new(0.7, 1.0),
        );
    }
    block(
        &mut builder,
        Vec3::new(-half - 1.0, 4.0, -1.2),
        Vec3::new(half + 1.0, 5.6, 1.2),
    );
    single("cliff_arch", builder, &material_named("cliff"))
}

// ---------------------------------------------------------------------------------------
// Ground
// ---------------------------------------------------------------------------------------

/// One 8 m tile of ground, 10 cm thick, its texture tiled four times across.
fn ground_tile() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE;
    builder.quad(
        [
            Vec3::new(-half, 0.1, half),
            Vec3::new(half, 0.1, half),
            Vec3::new(half, 0.1, -half),
            Vec3::new(-half, 0.1, -half),
        ],
        Vec3::Y,
        [
            Vec2::new(0.0, 4.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(0.0, 0.0),
        ],
    );
    block(
        &mut builder,
        Vec3::new(-half, 0.0, -half),
        Vec3::new(half, 0.1, half),
    );
    single("ground_tile", builder, &material_named("grass"))
}

/// A raised platform, 4 m square and half a metre thick: the blockout floor of a room.
fn platform_square() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE * 0.5;
    block(
        &mut builder,
        Vec3::new(-half, 0.0, -half),
        Vec3::new(half, 0.5, half),
    );
    single("platform_square", builder, &material_named("dirt"))
}

/// A ramp up onto a platform: 4 m wide, 0.5 m up over 2 m — 14°.
fn ramp_square() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE * 0.5;
    builder.quad(
        [
            Vec3::new(-half, 0.0, half),
            Vec3::new(half, 0.0, half),
            Vec3::new(half, 0.5, -half),
            Vec3::new(-half, 0.5, -half),
        ],
        Vec3::new(0.0, MODULE, 0.5).normalize(),
        [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ],
    );
    block(
        &mut builder,
        Vec3::new(-half, 0.0, -half),
        Vec3::new(half, 0.5, -half + 0.4),
    );
    single("ramp_square", builder, &material_named("dirt"))
}

// ---------------------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------------------

/// A one-metre cube. The scale reference, and the first thing to place in an empty level.
fn stone_cube() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    block(
        &mut builder,
        Vec3::new(-0.5, 0.0, -0.5),
        Vec3::new(0.5, 1.0, 0.5),
    );
    single("stone_cube", builder, &material_named("simple_test"))
}

/// A standing stone pillar, 4.5 m tall with a base and a cap.
fn pillar() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    block(
        &mut builder,
        Vec3::new(-0.55, 0.0, -0.55),
        Vec3::new(0.55, 0.35, 0.55),
    );
    tapered(
        &mut builder,
        Vec3::new(0.0, 0.35, 0.0),
        Vec3::new(0.0, 4.1, 0.0),
        Vec2::new(0.42, 0.42),
        Vec2::new(0.34, 0.34),
    );
    block(
        &mut builder,
        Vec3::new(-0.52, 4.1, -0.52),
        Vec3::new(0.52, 4.5, 0.52),
    );
    single("pillar", builder, &material_named("rock"))
}

/// A built arch: two piers and a lintel, 4 m of headroom.
fn arch() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    for side in [-1.0_f32, 1.0] {
        block(
            &mut builder,
            Vec3::new(side * 2.0 - 0.5, 0.0, -0.5),
            Vec3::new(side * 2.0 + 0.5, 4.0, 0.5),
        );
    }
    block(
        &mut builder,
        Vec3::new(-2.7, 4.0, -0.6),
        Vec3::new(2.7, 5.0, 0.6),
    );
    single("arch", builder, &material_named("rock"))
}

/// A crate, one metre on a side, with a banded lid.
fn crate_wooden() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    block(
        &mut builder,
        Vec3::new(-0.5, 0.0, -0.5),
        Vec3::new(0.5, 0.92, 0.5),
    );
    block(
        &mut builder,
        Vec3::new(-0.54, 0.92, -0.54),
        Vec3::new(0.54, 1.0, 0.54),
    );
    single("crate_wooden", builder, &material_named("dirt"))
}

// ---------------------------------------------------------------------------------------
// Vegetation
// ---------------------------------------------------------------------------------------

/// A low round bush, 1.4 m across.
fn bush_round() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    tapered(
        &mut builder,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.75, 0.0),
        Vec2::new(0.55, 0.50),
        Vec2::new(0.42, 0.38),
    );
    tapered(
        &mut builder,
        Vec3::new(0.30, 0.10, 0.20),
        Vec3::new(0.32, 0.55, 0.22),
        Vec2::new(0.34, 0.30),
        Vec2::new(0.20, 0.18),
    );
    single("bush_round", builder, &material_named("grass"))
}

/// A tree-shaped object: a trunk and three stacked canopy blocks, 6 m tall.
///
/// Two materials, because a trunk and a canopy are the one place in this library where one
/// texture on the whole model would read as a mistake rather than as blockout.
fn tree_simple() -> ModelAsset {
    let mut trunk = MeshBuilder::new();
    tapered(
        &mut trunk,
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 2.6, 0.0),
        Vec2::new(0.28, 0.28),
        Vec2::new(0.18, 0.18),
    );
    let mut canopy = MeshBuilder::new();
    tapered(
        &mut canopy,
        Vec3::new(0.0, 2.2, 0.0),
        Vec3::new(0.0, 4.2, 0.0),
        Vec2::new(1.60, 1.60),
        Vec2::new(1.10, 1.10),
    );
    tapered(
        &mut canopy,
        Vec3::new(0.0, 4.2, 0.0),
        Vec3::new(0.0, 6.0, 0.0),
        Vec2::new(1.05, 1.05),
        Vec2::new(0.20, 0.20),
    );
    tree_pair(&mut trunk, &mut canopy)
}

/// Assembles the tree, so [`tree_simple`] reads as its shape rather than as its plumbing.
fn tree_pair(trunk: &mut MeshBuilder, canopy: &mut MeshBuilder) -> ModelAsset {
    pair(
        "tree_simple",
        (
            std::mem::replace(trunk, MeshBuilder::new()),
            "trunk",
            &material_named("dirt"),
        ),
        (
            std::mem::replace(canopy, MeshBuilder::new()),
            "canopy",
            &material_named("grass"),
        ),
    )
}

// ---------------------------------------------------------------------------------------
// Caves
// ---------------------------------------------------------------------------------------

/// The four walls of a tunnel running along Z: floor, roof and two sides, with the middle
/// left empty.
fn tunnel_shell(builder: &mut MeshBuilder, length: f32) {
    let half_width = TUNNEL_WIDTH * 0.5;
    let outer = half_width + SHELL;
    let far = -length;
    // Floor, thick enough to stand on when the piece is placed above the ground.
    block(
        builder,
        Vec3::new(-outer, 0.0, far),
        Vec3::new(outer, SHELL, 0.0),
    );
    // Two walls.
    for side in [-1.0_f32, 1.0] {
        let inner = side * half_width;
        let out = side * outer;
        block(
            builder,
            Vec3::new(inner.min(out), SHELL, far),
            Vec3::new(inner.max(out), SHELL + TUNNEL_HEIGHT, 0.0),
        );
    }
    // Roof.
    block(
        builder,
        Vec3::new(-outer, SHELL + TUNNEL_HEIGHT, far),
        Vec3::new(outer, SHELL + TUNNEL_HEIGHT + SHELL, 0.0),
    );
}

/// One straight 4 m length of tunnel, open at both ends.
fn cave_tunnel_straight() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    tunnel_shell(&mut builder, MODULE);
    single("cave_tunnel_straight", builder, &material_named("cliff"))
}

/// A quarter turn: a tunnel that arrives along -Z and leaves along +X.
///
/// Built as two straight halves that meet at the corner, which is what keeps the cavity a
/// simple shape for the collision that will be written for it.
fn cave_tunnel_curve() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half_width = TUNNEL_WIDTH * 0.5;
    let outer = half_width + SHELL;
    let reach = MODULE;
    let height = SHELL + TUNNEL_HEIGHT;

    // The leg running along -Z, and the leg running along +X, sharing the corner block.
    block(
        &mut builder,
        Vec3::new(-outer, 0.0, -reach),
        Vec3::new(outer, SHELL, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(-outer, 0.0, -reach),
        Vec3::new(reach, SHELL, -reach + TUNNEL_WIDTH + SHELL),
    );
    // Outer wall of the turn.
    block(
        &mut builder,
        Vec3::new(-outer, SHELL, -reach),
        Vec3::new(-half_width, height, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(-outer, SHELL, -reach),
        Vec3::new(reach, height, -reach + SHELL),
    );
    // Inner corner: the block the tunnel turns around.
    block(
        &mut builder,
        Vec3::new(half_width, SHELL, -reach + TUNNEL_WIDTH),
        Vec3::new(reach, height, 0.0),
    );
    // Roof over both legs.
    block(
        &mut builder,
        Vec3::new(-outer, height, -reach),
        Vec3::new(outer, height + SHELL, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(-outer, height, -reach),
        Vec3::new(reach, height + SHELL, -reach + TUNNEL_WIDTH + SHELL),
    );
    single("cave_tunnel_curve", builder, &material_named("cliff"))
}

/// The mouth of a cave: a rock face with a tunnel-sized hole through it.
///
/// Placed against a cliff run, it is where a cave starts; placed on its own it is a doorway
/// in a mound. The opening lines up with [`cave_tunnel_straight`].
fn cave_entrance() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half_width = TUNNEL_WIDTH * 0.5;
    let face = MODULE;
    let height = SHELL + TUNNEL_HEIGHT;
    // The face, in three pieces around the opening.
    block(
        &mut builder,
        Vec3::new(-face, 0.0, -1.0),
        Vec3::new(-half_width, CLIFF_HEIGHT, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(half_width, 0.0, -1.0),
        Vec3::new(face, CLIFF_HEIGHT, 0.0),
    );
    block(
        &mut builder,
        Vec3::new(-half_width, height, -1.0),
        Vec3::new(half_width, CLIFF_HEIGHT, 0.0),
    );
    // A short length of tunnel behind it, so the opening reads as a hole rather than as a
    // painted rectangle.
    tunnel_shell(&mut builder, MODULE);
    single("cave_entrance", builder, &material_named("cliff"))
}

/// A room: 12 m square inside, with 5 m of headroom and a doorway on each side.
fn cave_room_small() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let inner = 6.0_f32;
    let outer = inner + SHELL;
    let height = 5.0;
    let door = TUNNEL_WIDTH * 0.5;

    block(
        &mut builder,
        Vec3::new(-outer, 0.0, -outer),
        Vec3::new(outer, SHELL, outer),
    );
    block(
        &mut builder,
        Vec3::new(-outer, SHELL + height, -outer),
        Vec3::new(outer, SHELL + height + SHELL, outer),
    );
    // Four walls, each in two pieces with a doorway between them, plus a lintel over it.
    for (along_x, side) in [(true, -1.0_f32), (true, 1.0), (false, -1.0), (false, 1.0)] {
        for end in [-1.0_f32, 1.0] {
            let (min, max) = if along_x {
                (
                    Vec3::new(
                        if end < 0.0 { -outer } else { door },
                        SHELL,
                        side * inner,
                    ),
                    Vec3::new(
                        if end < 0.0 { -door } else { outer },
                        SHELL + height,
                        side * outer,
                    ),
                )
            } else {
                (
                    Vec3::new(
                        side * inner,
                        SHELL,
                        if end < 0.0 { -outer } else { door },
                    ),
                    Vec3::new(
                        side * outer,
                        SHELL + height,
                        if end < 0.0 { -door } else { outer },
                    ),
                )
            };
            block(&mut builder, min, max);
        }
        // The lintel over the doorway.
        let (min, max) = if along_x {
            (
                Vec3::new(-door, SHELL + TUNNEL_HEIGHT, side * inner),
                Vec3::new(door, SHELL + height, side * outer),
            )
        } else {
            (
                Vec3::new(side * inner, SHELL + TUNNEL_HEIGHT, -door),
                Vec3::new(side * outer, SHELL + height, door),
            )
        };
        block(&mut builder, min, max);
    }
    single("cave_room_small", builder, &material_named("cliff"))
}

// ---------------------------------------------------------------------------------------
// Water
// ---------------------------------------------------------------------------------------

/// A sheet of water, 8 m square, at the height it is placed at.
///
/// Transparent, and one quad: what a water *volume* means to gameplay is a Phase 8 question
/// about the level, not about the mesh.
fn water_plane() -> ModelAsset {
    let mut builder = MeshBuilder::new();
    let half = MODULE;
    builder.quad(
        [
            Vec3::new(-half, 0.0, half),
            Vec3::new(half, 0.0, half),
            Vec3::new(half, 0.0, -half),
            Vec3::new(-half, 0.0, -half),
        ],
        Vec3::Y,
        [
            Vec2::new(0.0, 2.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(0.0, 0.0),
        ],
    );
    single("water_plane", builder, &material_named("water"))
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::gltf_export::export_glb;
    use crate::gltf_import::import_glb;
    use crate::validate::{ValidationLimits, validate_model};

    #[test]
    fn every_starter_asset_builds_something_valid() {
        for asset in STARTER_LIBRARY {
            let model = (asset.build)();
            assert_eq!(model.name, asset.name);
            assert!(
                model.triangle_count() > 0,
                "{} has no triangles",
                asset.name
            );
            validate_model(&model, &ValidationLimits::default())
                .unwrap_or_else(|error| panic!("{} is not a valid model: {error}", asset.name));
        }
    }

    #[test]
    fn every_starter_asset_survives_being_written_and_read_back() {
        for asset in STARTER_LIBRARY {
            let model = (asset.build)();
            let bytes = export_glb(&model)
                .unwrap_or_else(|error| panic!("{} does not export: {error}", asset.name));
            let restored = import_glb(asset.name, &bytes)
                .unwrap_or_else(|error| panic!("{} does not import: {error}", asset.name));
            assert_eq!(
                restored.triangle_count(),
                model.triangle_count(),
                "{} changed shape on the way through a file",
                asset.name
            );
            assert_eq!(
                restored.textures.len(),
                model.textures.len(),
                "{} lost a texture",
                asset.name
            );
        }
    }

    #[test]
    fn the_library_names_each_asset_once_and_puts_it_somewhere() {
        let mut names: Vec<&str> = STARTER_LIBRARY.iter().map(|asset| asset.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "two assets share a name");
        for asset in STARTER_LIBRARY {
            assert!(
                asset.folder.starts_with("environment/"),
                "{} is not under environment/",
                asset.name
            );
            assert_eq!(
                asset.relative_path(),
                format!("generated/{}/{}.glb", asset.folder, asset.name)
            );
        }
    }

    #[test]
    fn every_material_generates_the_texture_it_says_it_does() {
        for material in STARTER_MATERIALS {
            let texture = material.texture();
            assert_eq!((texture.width, texture.height), (TEXTURE_SIZE, TEXTURE_SIZE));
            assert_eq!(
                texture.texels.len(),
                (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize
            );
            assert_eq!(
                texture, material.texture(),
                "{} is not the same texture twice",
                material.name
            );
        }
    }

    #[test]
    fn water_is_the_only_transparent_starter_material() {
        for material in STARTER_MATERIALS {
            let transparent = material.material().is_transparent();
            assert_eq!(
                transparent,
                material.name == "water",
                "{} is transparent and should not be",
                material.name
            );
        }
    }

    #[test]
    fn a_character_fits_through_every_cave_piece() {
        // 1.8 m tall and roughly half a metre wide. The cavity of a tunnel is what the piece
        // exists for, so the room in it is measured off the geometry rather than off the
        // constants the geometry was built from — a wall placed at the wrong X is exactly the
        // mistake this is here to catch.
        for name in ["cave_tunnel_straight", "cave_entrance", "cave_room_small"] {
            let model = starter_model(name).expect("the library has it");
            let lod = model.lod0().expect("geometry");
            let mut floor_top = f32::MIN;
            let mut roof_bottom = f32::MAX;
            for submesh in &lod.submeshes {
                let crate::mesh::Geometry::Static(vertices) = &submesh.geometry else {
                    continue;
                };
                for vertex in vertices {
                    // Only the middle of the piece, where a character actually walks.
                    if vertex.position.x.abs() > 0.5 || vertex.position.z > -0.5 {
                        continue;
                    }
                    if vertex.position.y <= SHELL + 0.001 {
                        floor_top = floor_top.max(vertex.position.y);
                    } else {
                        roof_bottom = roof_bottom.min(vertex.position.y);
                    }
                }
            }
            let headroom = roof_bottom - floor_top;
            assert!(
                headroom >= 1.8 + 0.5,
                "{name} has {headroom} m of headroom; a character is 1.8 m"
            );
        }
    }
}
