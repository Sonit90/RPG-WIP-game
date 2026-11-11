//! The Graveyard Crypt's monsters: seven stand-in models on the shared humanoid rig.
//!
//! ```text
//! skeleton_bonecaster   bare bones, cowl and mantle, a staff with a burning skull   Channel
//! crypt_archer          bare bones, leather hood, quiver, a bow in the left hand    Bow
//! skeleton_warrior      bare bones, rusted helm, pauldron and mail, sword, shield   Swing
//! grave_acolyte         a hooded robe with two violet eyes in it, a censer          Channel
//! grave_brute           a stitched giant, hunched, shackled fists                   Slam
//! crypt_ghoul           a crouched, starved thing with long claws                   Claw
//! lord_ashgrave         the boss: horned crown, ember-lit plate, cape, greatsword   Swing
//! ```
//!
//! # Temporary, on purpose
//!
//! Every monster's table row names the model it is drawn with (`MonsterLook::model`), and
//! until an artist delivers one these are it. They are built the way the heroes are — prisms
//! on the one 21-bone rig, generated textures, no files — so an export dropped in later under
//! the same name replaces a monster without a code change, and until then each monster reads
//! as itself from across a room: the silhouette first (a staff, a bow, a shield, a hunch, a
//! crouch, a crown), then the colour of its eyes.
//!
//! # What a monster adds to the rig
//!
//! Its **posture** ([`Posture`]) — the brute's hunch and the ghoul's crouch are offsets over
//! every clip, not different cycles — a **death**, and a **cast style** for its attacks. The
//! walk, the run and the idle are the heroes' cycles with a monster's amplitudes.
//!
//! # Props are authored in the hand
//!
//! A staff, a bow or a sword is described as it is *held at rest* — arm hanging, the staff
//! upright, the blade forward — in a frame at the hand ([`Grip`]), and turned back into the
//! T-pose the rig binds in. Writing a prop straight in bind space means working out where
//! "upright" goes when the arm comes down 84 degrees, for every prop, by hand.

use engine_core::math::{Quat, Vec2, Vec3};

use crate::AssetError;
use crate::character::{AnimationSlot, CharacterDef};
use crate::library::ModelId;
use crate::material::MaterialAsset;
use crate::mesh::SubMesh;
use crate::model::ModelAsset;
use crate::procedural::Prism;
use crate::procedural::humanoid::{
    self, CHEST, CastStyle, FOOT_L, FOOT_R, FOREARM_L, FOREARM_R, HAND_L, HAND_R, HEAD, HIPS,
    MODEL_HEIGHT, NECK, PROP, Posture, SHIN_L, SHIN_R, SHOULDER_L, SHOULDER_R, SPINE, SkinBuilder,
    Style, THIGH_L, THIGH_R,
};
use crate::texture::{TextureAsset, TextureFormat};

/// The Skeleton Bonecaster's model.
pub const SKELETON_BONECASTER: &str = "skeleton_bonecaster";
/// The Crypt Archer's model.
pub const CRYPT_ARCHER: &str = "crypt_archer";
/// The Skeleton Warrior's model.
pub const SKELETON_WARRIOR: &str = "skeleton_warrior";
/// The Grave Acolyte's model.
pub const GRAVE_ACOLYTE: &str = "grave_acolyte";
/// The Grave Brute's model.
pub const GRAVE_BRUTE: &str = "grave_brute";
/// The Crypt Ghoul's model.
pub const CRYPT_GHOUL: &str = "crypt_ghoul";
/// Lord Ashgrave's model.
pub const LORD_ASHGRAVE: &str = "lord_ashgrave";

/// Every monster model this module builds, by the name a table row asks for.
pub const MONSTER_MODELS: [&str; 7] = [
    SKELETON_BONECASTER,
    CRYPT_ARCHER,
    SKELETON_WARRIOR,
    GRAVE_ACOLYTE,
    GRAVE_BRUTE,
    CRYPT_GHOUL,
    LORD_ASHGRAVE,
];

/// Builds the monster model called `name`, or `None` for a name this module does not build.
///
/// # Errors
///
/// The inner result fails only if the shared rig is malformed.
#[must_use]
pub fn monster_model(name: &str) -> Option<Result<ModelAsset, AssetError>> {
    let design = design(name)?;
    Some(build(name, &design))
}

/// The character definition for the monster model called `name`: its clips by slot, and the
/// speeds its walk and run were authored at.
#[must_use]
pub fn monster_character(name: &str) -> Option<CharacterDef> {
    let design = design(name)?;
    let def = CharacterDef::new(name, ModelId::new(name), MODEL_HEIGHT)
        .with_speeds(design.speeds.0, design.speeds.1)
        .with_clip(AnimationSlot::Idle, "idle")
        .with_clip(AnimationSlot::Walk, "walk")
        .with_clip(AnimationSlot::Run, "run")
        .with_clip(AnimationSlot::Jump, "jump")
        .with_clip(AnimationSlot::Fall, "fall")
        .with_clip(AnimationSlot::Land, "land")
        .with_clip(AnimationSlot::Cast, "cast")
        .with_clip(AnimationSlot::CastHeavy, "cast_heavy")
        .with_clip(AnimationSlot::Death, "death");
    Some(def)
}

/// Everything one monster is: how it moves, how it stands, and what it is made of.
struct Design {
    style: Style,
    posture: Posture,
    /// `(walk, run)` metres per second its cycles were authored at.
    speeds: (f32, f32),
    materials: Vec<MaterialAsset>,
    textures: Vec<TextureAsset>,
    /// The geometry, one submesh per material that has any.
    geometry: fn(&Style) -> Vec<SubMesh>,
}

fn design(name: &str) -> Option<Design> {
    Some(match name {
        SKELETON_BONECASTER => bonecaster(),
        CRYPT_ARCHER => archer(),
        SKELETON_WARRIOR => warrior(),
        GRAVE_ACOLYTE => acolyte(),
        GRAVE_BRUTE => brute(),
        CRYPT_GHOUL => ghoul(),
        LORD_ASHGRAVE => ashgrave(),
        _ => return None,
    })
}

fn build(name: &str, design: &Design) -> Result<ModelAsset, AssetError> {
    let lod0 = (design.geometry)(&design.style);
    Ok(ModelAsset::rigid(
        name,
        lod0,
        design.materials.clone(),
        design.textures.clone(),
    )
    .with_animation(
        humanoid::skeleton()?,
        humanoid::monster_clips(&design.style, &design.posture),
    ))
}

// ------------------------------------------------------------------------------------ kit

/// One skinned builder per material, finished into the submeshes that have geometry.
struct Kit {
    builders: Vec<(&'static str, SkinBuilder)>,
}

impl Kit {
    fn new(names: &[&'static str]) -> Self {
        Self {
            builders: names
                .iter()
                .map(|name| (*name, SkinBuilder::new()))
                .collect(),
        }
    }

    /// Adds `prism` to material `material`, following `lower` at its lower cap and `upper` at
    /// its upper one.
    fn add(&mut self, material: usize, prism: &Prism, lower: u16, upper: u16) {
        if let Some((_, builder)) = self.builders.get_mut(material) {
            builder.prism(prism, lower, upper);
        }
    }

    /// Adds `prism` bound rigidly to `bone`.
    fn rigid(&mut self, material: usize, prism: &Prism, bone: u16) {
        self.add(material, prism, bone, bone);
    }

    fn finish(self) -> Vec<SubMesh> {
        self.builders
            .into_iter()
            .enumerate()
            .filter(|(_, (_, builder))| !builder.is_empty())
            .map(|(material, (name, builder))| builder.into_submesh(name, material))
            .collect()
    }
}

fn v3(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

/// A prism from `lower` up to `upper`, its cross-section `(width along X, depth)`.
fn seg(lower: [f32; 3], upper: [f32; 3], lower_half: [f32; 2], upper_half: [f32; 2]) -> Prism {
    Prism::new(v3(lower), v3(upper), lower_half[0], upper_half[0]).with_halves(
        Vec2::new(lower_half[0], lower_half[1]),
        Vec2::new(upper_half[0], upper_half[1]),
    )
}

/// A prism running sideways, as an arm does in the T-pose: `(height, depth)` cross-section.
fn bar(lower: [f32; 3], upper: [f32; 3], lower_half: [f32; 2], upper_half: [f32; 2]) -> Prism {
    seg(lower, upper, lower_half, upper_half).with_width_axis(Vec3::Y)
}

/// An axis-aligned box.
fn cube(centre: [f32; 3], half: [f32; 3]) -> Prism {
    seg(
        [centre[0], centre[1] - half[1], centre[2]],
        [centre[0], centre[1] + half[1], centre[2]],
        [half[0], half[2]],
        [half[0], half[2]],
    )
}

/// The frame of a hand at rest: authoring space for whatever it holds.
///
/// Origin at the hand joint; +Y up and −Z forward *as the monster stands idle*, arm hanging.
/// [`Grip::prism`] turns a prop written there back into the T-pose the rig binds in, by
/// undoing the roll that brought the arm down.
struct Grip {
    origin: Vec3,
    to_bind: Quat,
}

impl Grip {
    /// The right hand, for `style`'s resting arm.
    fn right(style: &Style) -> Self {
        // The idle clip's shoulder roll plus its forearm's.
        Self {
            origin: Vec3::new(-0.65, 1.39, 0.0),
            to_bind: humanoid::ypr(0.0, 0.0, style.arm_down_r + 12.0).conjugate(),
        }
    }

    /// The left hand.
    fn left(style: &Style) -> Self {
        Self {
            origin: Vec3::new(0.65, 1.39, 0.0),
            to_bind: humanoid::ypr(0.0, 0.0, style.arm_down_l - 12.0).conjugate(),
        }
    }

    fn point(&self, local: [f32; 3]) -> Vec3 {
        self.origin + self.to_bind.rotate(v3(local))
    }

    /// [`seg`], written in the hand's resting frame.
    fn seg(
        &self,
        lower: [f32; 3],
        upper: [f32; 3],
        lower_half: [f32; 2],
        upper_half: [f32; 2],
    ) -> Prism {
        Prism::new(
            self.point(lower),
            self.point(upper),
            lower_half[0],
            upper_half[0],
        )
        .with_halves(
            Vec2::new(lower_half[0], lower_half[1]),
            Vec2::new(upper_half[0], upper_half[1]),
        )
        .with_width_axis(self.to_bind.rotate(Vec3::X))
    }

    /// [`cube`], written in the hand's resting frame.
    fn cube(&self, centre: [f32; 3], half: [f32; 3]) -> Prism {
        self.seg(
            [centre[0], centre[1] - half[1], centre[2]],
            [centre[0], centre[1] + half[1], centre[2]],
            [half[0], half[2]],
            [half[0], half[2]],
        )
    }
}

/// Which side of the body, and the bones on it.
#[derive(Clone, Copy)]
struct Side {
    /// +1 for the left (+X), −1 for the right.
    sign: f32,
    shoulder: u16,
    forearm: u16,
    hand: u16,
    thigh: u16,
    shin: u16,
    foot: u16,
}

const SIDES: [Side; 2] = [
    Side {
        sign: 1.0,
        shoulder: SHOULDER_L,
        forearm: FOREARM_L,
        hand: HAND_L,
        thigh: THIGH_L,
        shin: SHIN_L,
        foot: FOOT_L,
    },
    Side {
        sign: -1.0,
        shoulder: SHOULDER_R,
        forearm: FOREARM_R,
        hand: HAND_R,
        thigh: THIGH_R,
        shin: SHIN_R,
        foot: FOOT_R,
    },
];

/// The bare skeleton three of the monsters share: skull, jaw, spine, ribs, pelvis, limbs,
/// and a pair of burning eye sockets.
fn skeleton_frame(kit: &mut Kit, bone: usize, eyes: usize) {
    // Skull and jaw. The skull sits a touch back so the jaw juts — the cheapest cue that it
    // is a skull and not a box.
    kit.rigid(
        bone,
        &seg(
            [0.0, 1.555, 0.01],
            [0.0, 1.78, 0.01],
            [0.090, 0.098],
            [0.084, 0.092],
        ),
        HEAD,
    );
    kit.rigid(
        bone,
        &seg(
            [0.0, 1.47, -0.025],
            [0.0, 1.56, -0.02],
            [0.060, 0.066],
            [0.074, 0.078],
        ),
        HEAD,
    );
    kit.add(
        bone,
        &seg(
            [0.0, 1.40, 0.01],
            [0.0, 1.50, 0.01],
            [0.026, 0.026],
            [0.026, 0.026],
        ),
        NECK,
        HEAD,
    );
    // Spine.
    kit.add(
        bone,
        &seg(
            [0.0, 0.95, 0.05],
            [0.0, 1.18, 0.06],
            [0.030, 0.030],
            [0.030, 0.030],
        ),
        HIPS,
        SPINE,
    );
    kit.add(
        bone,
        &seg(
            [0.0, 1.18, 0.06],
            [0.0, 1.42, 0.03],
            [0.030, 0.030],
            [0.028, 0.028],
        ),
        SPINE,
        CHEST,
    );
    // Ribs: four hoops narrowing downwards, and a sternum down the front.
    for (index, y) in [1.17_f32, 1.23, 1.29, 1.35].into_iter().enumerate() {
        let width = 0.118 + 0.014 * index as f32;
        let bone_of = if index == 0 { SPINE } else { CHEST };
        kit.rigid(
            bone,
            &seg(
                [0.0, y - 0.016, 0.015],
                [0.0, y + 0.016, 0.015],
                [width, 0.092],
                [width, 0.094],
            ),
            bone_of,
        );
    }
    kit.rigid(
        bone,
        &seg(
            [0.0, 1.15, -0.085],
            [0.0, 1.37, -0.085],
            [0.022, 0.012],
            [0.026, 0.012],
        ),
        CHEST,
    );
    kit.rigid(
        bone,
        &bar(
            [-0.21, 1.405, 0.0],
            [0.21, 1.405, 0.0],
            [0.020, 0.028],
            [0.020, 0.028],
        ),
        CHEST,
    );
    // Pelvis.
    kit.rigid(
        bone,
        &seg(
            [0.0, 0.84, 0.01],
            [0.0, 0.99, 0.01],
            [0.10, 0.065],
            [0.15, 0.080],
        ),
        HIPS,
    );

    for side in SIDES {
        let s = side.sign;
        kit.rigid(
            bone,
            &cube([s * 0.20, 1.39, 0.0], [0.042, 0.042, 0.042]),
            side.shoulder,
        );
        kit.add(
            bone,
            &bar(
                [s * 0.20, 1.39, 0.0],
                [s * 0.42, 1.39, 0.0],
                [0.028, 0.028],
                [0.024, 0.024],
            ),
            side.shoulder,
            side.forearm,
        );
        kit.rigid(
            bone,
            &cube([s * 0.42, 1.39, 0.0], [0.032, 0.032, 0.032]),
            side.forearm,
        );
        kit.add(
            bone,
            &bar(
                [s * 0.42, 1.39, 0.0],
                [s * 0.64, 1.39, 0.0],
                [0.022, 0.030],
                [0.020, 0.026],
            ),
            side.forearm,
            side.hand,
        );
        kit.rigid(
            bone,
            &bar(
                [s * 0.64, 1.39, 0.0],
                [s * 0.72, 1.387, 0.0],
                [0.040, 0.020],
                [0.036, 0.016],
            ),
            side.hand,
        );
        kit.rigid(
            bone,
            &bar(
                [s * 0.72, 1.387, 0.0],
                [s * 0.78, 1.38, 0.0],
                [0.032, 0.010],
                [0.022, 0.008],
            ),
            side.hand,
        );

        let x = s * 0.09;
        kit.add(
            bone,
            &seg(
                [x, 0.47, 0.0],
                [x, 0.88, 0.0],
                [0.028, 0.028],
                [0.034, 0.034],
            ),
            side.shin,
            side.thigh,
        );
        kit.rigid(
            bone,
            &cube([x, 0.455, -0.012], [0.038, 0.032, 0.038]),
            side.shin,
        );
        kit.add(
            bone,
            &seg(
                [x, 0.07, 0.0],
                [x, 0.44, 0.0],
                [0.024, 0.024],
                [0.029, 0.029],
            ),
            side.foot,
            side.shin,
        );
        kit.rigid(
            bone,
            &seg(
                [x, 0.0, -0.045],
                [x, 0.065, -0.03],
                [0.044, 0.095],
                [0.038, 0.066],
            ),
            side.foot,
        );
    }

    for s in [1.0_f32, -1.0] {
        kit.rigid(
            eyes,
            &cube([s * 0.036, 1.655, -0.086], [0.021, 0.017, 0.012]),
            HEAD,
        );
    }
}

// ------------------------------------------------------------------------------ textures

fn noise(u: f32, v: f32, scale: f32, seed: u32) -> f32 {
    let (x, y) = (u * scale, v * scale);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (fx, fy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |dx: f32, dy: f32| {
        humanoid::hash01(
            ((x0 + dx) as i32 as u32).wrapping_add(seed.wrapping_mul(7919)),
            (y0 + dy) as i32 as u32,
        )
    };
    let top = at(0.0, 0.0) + (at(1.0, 0.0) - at(0.0, 0.0)) * fx;
    let bottom = at(0.0, 1.0) + (at(1.0, 1.0) - at(0.0, 1.0)) * fx;
    top + (bottom - top) * fy
}

fn grain(u: f32, v: f32, size: u32) -> f32 {
    humanoid::hash01((u * size as f32) as u32, (v * size as f32) as u32)
}

/// Old bone: ivory, grimed in the creases, with hairline cracks.
fn bone_texture(name: &str, base: [f32; 3]) -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(name, SIZE, SIZE, TextureFormat::Rgba8Srgb, |u, v| {
        let grime = noise(u, v, 6.0, 1) * 0.35 + grain(u, v, SIZE) * 0.08;
        let crack = (noise(u, v, 14.0, 2) - 0.5).abs() < 0.018;
        let mut color = humanoid::mix(base, [0.36, 0.31, 0.24], grime);
        if crack {
            color = humanoid::mix(color, [0.20, 0.17, 0.13], 0.7);
        }
        humanoid::texel(color)
    })
}

/// Woven cloth, darker and ragged at its hem.
fn cloth_texture(name: &str, base: [f32; 3], trim: Option<[f32; 3]>) -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(name, SIZE, SIZE, TextureFormat::Rgba8Srgb, move |u, v| {
        let weave = ((u * 90.0).sin() * (v * 90.0).sin()).abs() * 0.10;
        let stain = noise(u, v, 5.0, 3) * 0.30;
        let mut color = humanoid::mix(base, [0.02, 0.02, 0.03], weave + stain);
        if v < 0.12 {
            color = humanoid::mix(color, [0.03, 0.03, 0.03], 0.5);
        }
        if let Some(trim) = trim
            && (0.86..0.92).contains(&v)
        {
            color = humanoid::mix(color, trim, 0.75);
        }
        humanoid::texel(color)
    })
}

/// Iron, pitted and rusting in patches.
fn metal_texture(name: &str, base: [f32; 3], rust: f32) -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(name, SIZE, SIZE, TextureFormat::Rgba8Srgb, move |u, v| {
        let patch = (noise(u, v, 7.0, 4) - (1.0 - rust)).max(0.0) * 2.5;
        let pit = grain(u, v, SIZE) * 0.10;
        let color = humanoid::mix(base, [0.36, 0.17, 0.08], patch.min(1.0));
        humanoid::texel(humanoid::mix(color, [0.0, 0.0, 0.0], pit))
    })
}

/// Dead skin, mottled, optionally stitched across in bands.
fn flesh_texture(name: &str, base: [f32; 3], dark: [f32; 3], stitched: bool) -> TextureAsset {
    const SIZE: u32 = 128;
    TextureAsset::generated(name, SIZE, SIZE, TextureFormat::Rgba8Srgb, move |u, v| {
        let mottle = noise(u, v, 8.0, 5) * 0.55 + grain(u, v, SIZE) * 0.08;
        let mut color = humanoid::mix(base, dark, mottle);
        if stitched {
            let band = (v * 3.0).fract();
            let on_seam = (band - 0.5).abs() < 0.012;
            let tick = (band - 0.5).abs() < 0.04 && (u * 40.0).fract() < 0.18;
            if on_seam || tick {
                color = humanoid::mix(color, [0.10, 0.05, 0.04], 0.85);
            }
        }
        humanoid::texel(color)
    })
}

/// Leather, with the horizontal grain of a strap.
fn leather_texture(name: &str, base: [f32; 3]) -> TextureAsset {
    const SIZE: u32 = 64;
    TextureAsset::generated(name, SIZE, SIZE, TextureFormat::Rgba8Srgb, move |u, v| {
        let band = ((v * 40.0).sin() * 0.5 + 0.5) * 0.16;
        let wear = grain(u, v, SIZE) * 0.10;
        humanoid::texel(humanoid::mix(base, [0.04, 0.03, 0.02], band + wear))
    })
}

/// Wood, grained along its length.
fn wood_texture(name: &str, base: [f32; 3]) -> TextureAsset {
    const SIZE: u32 = 64;
    TextureAsset::generated(name, SIZE, SIZE, TextureFormat::Rgba8Srgb, move |u, v| {
        let streak = ((u * 26.0 + noise(u, v, 3.0, 6) * 4.0).sin() * 0.5 + 0.5) * 0.25;
        humanoid::texel(humanoid::mix(
            base,
            [0.08, 0.05, 0.03],
            streak + grain(u, v, SIZE) * 0.06,
        ))
    })
}

fn textured(name: &str, texture: usize, roughness: f32, metallic: f32) -> MaterialAsset {
    MaterialAsset::colored(name, 1.0, 1.0, 1.0)
        .with_roughness(roughness)
        .with_metallic(metallic)
        .with_base_color_texture(texture)
}

/// A surface that lights itself: eyes, a burning orb, runes.
fn glow(name: &str, color: [f32; 3], strength: f32) -> MaterialAsset {
    MaterialAsset::colored(name, color[0], color[1], color[2])
        .with_roughness(0.4)
        .with_emissive(Vec3::new(color[0], color[1], color[2]) * strength)
}

// ------------------------------------------------------------------------ the bonecaster

fn bonecaster() -> Design {
    const BONE: usize = 0;
    const EYES: usize = 1;
    const ROBE: usize = 2;
    const WOOD: usize = 3;
    fn geometry(style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["bone", "eyes", "robe", "staff"]);
        skeleton_frame(&mut kit, BONE, EYES);
        // A cowl behind the skull, rising to a point, and a ragged mantle on the shoulders.
        kit.rigid(
            ROBE,
            &seg(
                [0.0, 1.50, 0.05],
                [0.0, 1.92, 0.12],
                [0.125, 0.10],
                [0.02, 0.02],
            ),
            HEAD,
        );
        kit.rigid(
            ROBE,
            &seg(
                [0.0, 1.33, 0.0],
                [0.0, 1.45, 0.01],
                [0.27, 0.15],
                [0.19, 0.13],
            ),
            CHEST,
        );
        // A torn skirt from the hips, split front and back so the legs show through it.
        for z in [-0.06_f32, 0.07] {
            kit.rigid(
                ROBE,
                &seg(
                    [0.0, 0.42, z * 1.4],
                    [0.0, 0.97, z],
                    [0.21, 0.045],
                    [0.15, 0.05],
                ),
                HIPS,
            );
        }
        kit.rigid(
            ROBE,
            &seg(
                [0.0, 0.93, 0.01],
                [0.0, 1.00, 0.01],
                [0.155, 0.095],
                [0.155, 0.095],
            ),
            HIPS,
        );

        // The staff, upright in the right hand, and the skull that burns on top of it.
        let grip = Grip::right(style);
        kit.rigid(
            WOOD,
            &grip.seg(
                [0.0, -0.62, 0.0],
                [0.0, 1.05, 0.0],
                [0.024, 0.024],
                [0.020, 0.020],
            ),
            PROP,
        );
        kit.rigid(
            WOOD,
            &grip.seg(
                [0.0, 1.00, 0.0],
                [0.0, 1.10, 0.0],
                [0.05, 0.05],
                [0.06, 0.06],
            ),
            PROP,
        );
        kit.rigid(
            BONE,
            &grip.cube([0.0, 1.17, 0.0], [0.065, 0.07, 0.07]),
            PROP,
        );
        kit.rigid(
            EYES,
            &grip.cube([0.0, 1.30, 0.0], [0.045, 0.045, 0.045]),
            PROP,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                EYES,
                &grip.cube([s * 0.026, 1.18, -0.068], [0.014, 0.012, 0.006]),
                PROP,
            );
            kit.rigid(
                BONE,
                &grip.seg(
                    [s * 0.05, 1.05, 0.0],
                    [s * 0.10, 1.28, 0.0],
                    [0.012, 0.012],
                    [0.004, 0.004],
                ),
                PROP,
            );
        }
        kit.finish()
    }
    let style = Style {
        arm_down_l: -76.0,
        arm_down_r: 76.0,
        walk_period: 1.1,
        run_period: 0.66,
        stride: 20.0,
        knee: 24.0,
        arm_swing: 12.0,
        bob: 0.018,
        lean: 4.0,
        breath: 2.0,
        cast: CastStyle::Channel,
    };
    Design {
        style,
        posture: Posture {
            hunch: 8.0,
            head_up: 6.0,
            ..Posture::default()
        },
        speeds: (1.5, 5.0),
        materials: vec![
            textured("bonecaster_bone", 0, 0.8, 0.0),
            glow("bonecaster_fire", [0.45, 0.85, 1.0], 3.2),
            textured("bonecaster_robe", 1, 0.95, 0.0),
            textured("bonecaster_staff", 2, 0.85, 0.0),
        ],
        textures: vec![
            bone_texture("bonecaster_bone", [0.80, 0.77, 0.66]),
            cloth_texture(
                "bonecaster_robe",
                [0.10, 0.13, 0.22],
                Some([0.35, 0.55, 0.70]),
            ),
            wood_texture("bonecaster_staff", [0.28, 0.20, 0.14]),
        ],
        geometry,
    }
}

// ---------------------------------------------------------------------------- the archer

fn archer() -> Design {
    const BONE: usize = 0;
    const EYES: usize = 1;
    const LEATHER: usize = 2;
    const WOOD: usize = 3;
    const STRING: usize = 4;
    fn geometry(style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["bone", "eyes", "leather", "bow", "string"]);
        skeleton_frame(&mut kit, BONE, EYES);
        // A leather hood over the skull, open at the front.
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 1.60, 0.04],
                [0.0, 1.83, 0.05],
                [0.108, 0.085],
                [0.09, 0.07],
            ),
            HEAD,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                LEATHER,
                &seg(
                    [s * 0.098, 1.48, 0.0],
                    [s * 0.098, 1.74, 0.0],
                    [0.014, 0.10],
                    [0.014, 0.10],
                ),
                HEAD,
            );
        }
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 1.34, 0.02],
                [0.0, 1.48, 0.03],
                [0.20, 0.13],
                [0.11, 0.10],
            ),
            CHEST,
        );
        // A chest strap, a belt and a ragged loincloth.
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 1.21, 0.015],
                [0.0, 1.26, 0.015],
                [0.17, 0.11],
                [0.17, 0.11],
            ),
            CHEST,
        );
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 0.93, 0.01],
                [0.0, 0.99, 0.01],
                [0.16, 0.095],
                [0.16, 0.095],
            ),
            HIPS,
        );
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 0.60, -0.08],
                [0.0, 0.94, -0.085],
                [0.09, 0.012],
                [0.11, 0.012],
            ),
            HIPS,
        );
        // The quiver across the back, and the fletchings standing out of it.
        kit.rigid(
            LEATHER,
            &seg(
                [0.10, 1.04, 0.15],
                [-0.08, 1.52, 0.16],
                [0.055, 0.045],
                [0.065, 0.05],
            ),
            CHEST,
        );
        for (index, dx) in [-0.10_f32, -0.07, -0.05].into_iter().enumerate() {
            let x = dx + index as f32 * 0.005;
            kit.rigid(
                STRING,
                &seg(
                    [x, 1.52, 0.155],
                    [x - 0.02, 1.62, 0.16],
                    [0.012, 0.004],
                    [0.012, 0.004],
                ),
                CHEST,
            );
        }
        // A bracer on the bow arm.
        kit.add(
            LEATHER,
            &bar(
                [0.46, 1.39, 0.0],
                [0.62, 1.39, 0.0],
                [0.036, 0.040],
                [0.034, 0.038],
            ),
            FOREARM_L,
            HAND_L,
        );

        // The bow, carried across the body at rest: its length forward and back, its string
        // up. The draw turns the forearm a quarter, which stands it upright with the string
        // towards the archer — see `humanoid::bow_draw`.
        let grip = Grip::left(style);
        let arc = [
            ([0.0, 0.0, -0.05], [0.0, 0.03, -0.30], 0.022),
            ([0.0, 0.03, -0.30], [0.0, 0.11, -0.56], 0.016),
            ([0.0, 0.0, 0.05], [0.0, 0.03, 0.30], 0.022),
            ([0.0, 0.03, 0.30], [0.0, 0.11, 0.56], 0.016),
        ];
        kit.rigid(
            WOOD,
            &grip.cube([0.0, 0.0, 0.0], [0.024, 0.028, 0.06]),
            HAND_L,
        );
        for (lower, upper, half) in arc {
            kit.rigid(
                WOOD,
                &grip.seg(lower, upper, [half, half], [half * 0.8, half * 0.8]),
                HAND_L,
            );
        }
        kit.rigid(
            STRING,
            &grip.seg(
                [0.0, 0.115, -0.56],
                [0.0, 0.115, 0.56],
                [0.004, 0.004],
                [0.004, 0.004],
            ),
            HAND_L,
        );
        kit.finish()
    }
    let style = Style {
        arm_down_l: -76.0,
        arm_down_r: 76.0,
        walk_period: 1.0,
        run_period: 0.62,
        stride: 22.0,
        knee: 26.0,
        arm_swing: 14.0,
        bob: 0.02,
        lean: 5.0,
        breath: 1.8,
        cast: CastStyle::Bow,
    };
    Design {
        style,
        posture: Posture {
            hunch: 5.0,
            head_up: 4.0,
            ..Posture::default()
        },
        speeds: (1.6, 5.2),
        materials: vec![
            textured("archer_bone", 0, 0.8, 0.0),
            glow("archer_eyes", [0.55, 1.0, 0.55], 3.0),
            textured("archer_leather", 1, 0.75, 0.0),
            textured("archer_bow", 2, 0.7, 0.0),
            MaterialAsset::colored("archer_string", 0.72, 0.66, 0.52).with_roughness(0.9),
        ],
        textures: vec![
            bone_texture("archer_bone", [0.74, 0.72, 0.62]),
            leather_texture("archer_leather", [0.22, 0.20, 0.13]),
            wood_texture("archer_bow", [0.24, 0.16, 0.10]),
        ],
        geometry,
    }
}

// --------------------------------------------------------------------------- the warrior

fn warrior() -> Design {
    const BONE: usize = 0;
    const EYES: usize = 1;
    const IRON: usize = 2;
    const MAIL: usize = 3;
    const WOOD: usize = 4;
    fn geometry(style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["bone", "eyes", "iron", "mail", "shield"]);
        skeleton_frame(&mut kit, BONE, EYES);
        // An open-faced helm with a nasal bar and a crest ridge.
        kit.rigid(
            IRON,
            &seg(
                [0.0, 1.66, 0.012],
                [0.0, 1.81, 0.012],
                [0.102, 0.108],
                [0.085, 0.09],
            ),
            HEAD,
        );
        kit.rigid(
            IRON,
            &seg(
                [0.0, 1.60, -0.098],
                [0.0, 1.70, -0.100],
                [0.012, 0.010],
                [0.014, 0.010],
            ),
            HEAD,
        );
        kit.rigid(
            IRON,
            &seg(
                [0.0, 1.80, -0.06],
                [0.0, 1.86, 0.10],
                [0.012, 0.02],
                [0.012, 0.02],
            ),
            HEAD,
        );
        // One great pauldron on the shield side, a strap across the ribs.
        kit.rigid(
            IRON,
            &bar(
                [0.13, 1.33, 0.0],
                [0.33, 1.45, 0.0],
                [0.14, 0.14],
                [0.10, 0.11],
            ),
            SHOULDER_L,
        );
        kit.rigid(
            IRON,
            &seg(
                [0.0, 1.24, 0.015],
                [0.0, 1.29, 0.015],
                [0.16, 0.105],
                [0.16, 0.105],
            ),
            CHEST,
        );
        // A mail skirt and a belt.
        kit.rigid(
            MAIL,
            &seg(
                [0.0, 0.62, 0.01],
                [0.0, 0.97, 0.01],
                [0.19, 0.13],
                [0.155, 0.095],
            ),
            HIPS,
        );
        kit.rigid(
            IRON,
            &seg(
                [0.0, 0.94, 0.01],
                [0.0, 1.00, 0.01],
                [0.16, 0.10],
                [0.16, 0.10],
            ),
            HIPS,
        );
        // Greaves on the shins.
        for side in SIDES {
            let x = side.sign * 0.09;
            kit.add(
                IRON,
                &seg(
                    [x, 0.12, -0.01],
                    [x, 0.40, -0.01],
                    [0.042, 0.042],
                    [0.048, 0.048],
                ),
                side.foot,
                side.shin,
            );
        }

        // A notched sword held forward, point a little down.
        let grip = Grip::right(style);
        kit.rigid(
            WOOD,
            &grip.seg(
                [0.0, 0.0, 0.10],
                [0.0, 0.0, -0.06],
                [0.022, 0.022],
                [0.022, 0.022],
            ),
            PROP,
        );
        kit.rigid(
            IRON,
            &grip.seg(
                [0.0, 0.0, -0.06],
                [0.0, 0.0, -0.10],
                [0.10, 0.026],
                [0.10, 0.026],
            ),
            PROP,
        );
        kit.rigid(
            IRON,
            &grip.seg(
                [0.0, -0.01, -0.10],
                [0.0, -0.14, -0.78],
                [0.042, 0.012],
                [0.030, 0.010],
            ),
            PROP,
        );
        kit.rigid(
            IRON,
            &grip.seg(
                [0.0, -0.14, -0.78],
                [0.0, -0.16, -0.88],
                [0.030, 0.010],
                [0.003, 0.003],
            ),
            PROP,
        );
        // A round shield strapped to the outside of the left forearm.
        let shield = Grip::left(style);
        kit.rigid(
            WOOD,
            &shield.cube([0.05, 0.14, 0.0], [0.022, 0.25, 0.25]),
            FOREARM_L,
        );
        kit.rigid(
            IRON,
            &shield.cube([0.08, 0.14, 0.0], [0.02, 0.06, 0.06]),
            FOREARM_L,
        );
        for (dy, dz) in [(0.24, 0.0), (-0.24, 0.0)] {
            kit.rigid(
                IRON,
                &shield.cube([0.06, 0.14 + dy * 0.98, dz], [0.02, 0.02, 0.24]),
                FOREARM_L,
            );
        }
        kit.finish()
    }
    let style = Style {
        arm_down_l: -74.0,
        arm_down_r: 74.0,
        walk_period: 1.0,
        run_period: 0.60,
        stride: 24.0,
        knee: 28.0,
        arm_swing: 14.0,
        bob: 0.024,
        lean: 6.0,
        breath: 1.8,
        cast: CastStyle::Swing,
    };
    Design {
        style,
        posture: Posture {
            hunch: 6.0,
            head_up: 4.0,
            arms_forward: 6.0,
            ..Posture::default()
        },
        speeds: (1.7, 5.4),
        materials: vec![
            textured("skwarrior_bone", 0, 0.8, 0.0),
            glow("skwarrior_eyes", [0.55, 0.75, 1.0], 3.0),
            textured("skwarrior_iron", 1, 0.55, 0.7),
            textured("skwarrior_mail", 2, 0.6, 0.6),
            textured("skwarrior_shield", 3, 0.8, 0.0),
        ],
        textures: vec![
            bone_texture("skwarrior_bone", [0.78, 0.75, 0.64]),
            metal_texture("skwarrior_iron", [0.34, 0.34, 0.36], 0.55),
            metal_texture("skwarrior_mail", [0.24, 0.24, 0.26], 0.35),
            wood_texture("skwarrior_shield", [0.30, 0.18, 0.10]),
        ],
        geometry,
    }
}

// --------------------------------------------------------------------------- the acolyte

fn acolyte() -> Design {
    const ROBE: usize = 0;
    const EYES: usize = 1;
    const SKIN: usize = 2;
    const SHADOW: usize = 3;
    const IRON: usize = 4;
    fn geometry(style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["robe", "eyes", "skin", "shadow", "censer"]);
        // A robe to the floor from the hips — the Acolyte glides — and a body over the ribs.
        kit.rigid(
            ROBE,
            &seg(
                [0.0, 0.02, 0.02],
                [0.0, 0.96, 0.0],
                [0.27, 0.22],
                [0.17, 0.12],
            ),
            HIPS,
        );
        kit.add(
            ROBE,
            &seg(
                [0.0, 0.94, 0.0],
                [0.0, 1.18, 0.0],
                [0.17, 0.12],
                [0.18, 0.12],
            ),
            HIPS,
            SPINE,
        );
        kit.add(
            ROBE,
            &seg(
                [0.0, 1.18, 0.0],
                [0.0, 1.44, 0.01],
                [0.18, 0.12],
                [0.20, 0.13],
            ),
            SPINE,
            CHEST,
        );
        kit.rigid(
            ROBE,
            &seg(
                [0.0, 1.36, 0.02],
                [0.0, 1.48, 0.03],
                [0.25, 0.15],
                [0.14, 0.12],
            ),
            CHEST,
        );
        // The hood: tall, drawn to a point that falls back, with only dark inside it.
        kit.add(
            ROBE,
            &seg(
                [0.0, 1.44, 0.03],
                [0.0, 1.78, 0.04],
                [0.135, 0.14],
                [0.12, 0.13],
            ),
            NECK,
            HEAD,
        );
        kit.rigid(
            ROBE,
            &seg(
                [0.0, 1.76, 0.05],
                [0.0, 1.96, 0.16],
                [0.10, 0.10],
                [0.012, 0.012],
            ),
            HEAD,
        );
        kit.rigid(
            SHADOW,
            &cube([0.0, 1.61, -0.138], [0.085, 0.10, 0.012]),
            HEAD,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                EYES,
                &cube([s * 0.036, 1.64, -0.152], [0.019, 0.012, 0.008]),
                HEAD,
            );
        }
        // A rope belt, and a stole in the violet of the order down the front.
        kit.rigid(
            SHADOW,
            &seg(
                [0.0, 0.92, 0.0],
                [0.0, 0.97, 0.0],
                [0.175, 0.125],
                [0.175, 0.125],
            ),
            HIPS,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                EYES,
                &seg(
                    [s * 0.07, 0.55, -0.20],
                    [s * 0.07, 0.93, -0.13],
                    [0.03, 0.008],
                    [0.03, 0.008],
                ),
                HIPS,
            );
        }
        // Wide sleeves, and the pale hands that come out of them.
        for side in SIDES {
            let s = side.sign;
            kit.add(
                ROBE,
                &bar(
                    [s * 0.14, 1.39, 0.0],
                    [s * 0.42, 1.39, 0.0],
                    [0.07, 0.07],
                    [0.075, 0.075],
                ),
                side.shoulder,
                side.forearm,
            );
            kit.add(
                ROBE,
                &bar(
                    [s * 0.42, 1.39, 0.0],
                    [s * 0.63, 1.39, 0.0],
                    [0.075, 0.075],
                    [0.11, 0.11],
                ),
                side.forearm,
                side.hand,
            );
            kit.rigid(
                SKIN,
                &bar(
                    [s * 0.62, 1.39, 0.0],
                    [s * 0.74, 1.385, 0.0],
                    [0.034, 0.018],
                    [0.03, 0.014],
                ),
                side.hand,
            );
        }
        // A censer on a chain in the left hand.
        let grip = Grip::left(style);
        kit.rigid(
            IRON,
            &grip.seg(
                [0.0, 0.0, 0.0],
                [0.0, -0.30, -0.02],
                [0.006, 0.006],
                [0.006, 0.006],
            ),
            HAND_L,
        );
        kit.rigid(
            IRON,
            &grip.cube([0.0, -0.36, -0.02], [0.05, 0.06, 0.05]),
            HAND_L,
        );
        kit.rigid(
            EYES,
            &grip.cube([0.0, -0.36, -0.02], [0.056, 0.02, 0.056]),
            HAND_L,
        );
        kit.finish()
    }
    let style = Style {
        arm_down_l: -70.0,
        arm_down_r: 70.0,
        walk_period: 1.2,
        run_period: 0.7,
        stride: 16.0,
        knee: 18.0,
        arm_swing: 6.0,
        bob: 0.012,
        lean: 3.0,
        breath: 1.4,
        cast: CastStyle::Channel,
    };
    Design {
        style,
        posture: Posture {
            hunch: 10.0,
            head_up: 4.0,
            arms_forward: 14.0,
            ..Posture::default()
        },
        speeds: (1.4, 5.0),
        materials: vec![
            textured("acolyte_robe", 0, 0.95, 0.0),
            glow("acolyte_eyes", [0.72, 0.32, 1.0], 3.4),
            textured("acolyte_skin", 1, 0.8, 0.0),
            MaterialAsset::colored("acolyte_shadow", 0.02, 0.015, 0.03).with_roughness(1.0),
            textured("acolyte_censer", 2, 0.5, 0.8),
        ],
        textures: vec![
            cloth_texture("acolyte_robe", [0.16, 0.10, 0.20], Some([0.50, 0.28, 0.70])),
            flesh_texture(
                "acolyte_skin",
                [0.72, 0.70, 0.68],
                [0.50, 0.46, 0.50],
                false,
            ),
            metal_texture("acolyte_censer", [0.30, 0.27, 0.22], 0.3),
        ],
        geometry,
    }
}

// ----------------------------------------------------------------------------- the brute

fn brute() -> Design {
    const FLESH: usize = 0;
    const EYES: usize = 1;
    const IRON: usize = 2;
    const LEATHER: usize = 3;
    const BONE: usize = 4;
    fn geometry(_style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["flesh", "eyes", "iron", "leather", "bone"]);
        // A gut, a barrel of a chest and a hump of muscle over the shoulders. The widest
        // thing on it is the shoulders by half again, which is the brute's whole silhouette.
        kit.add(
            FLESH,
            &seg(
                [0.0, 0.84, -0.01],
                [0.0, 1.12, -0.03],
                [0.21, 0.16],
                [0.25, 0.19],
            ),
            HIPS,
            SPINE,
        );
        kit.add(
            FLESH,
            &seg(
                [0.0, 1.10, -0.02],
                [0.0, 1.46, 0.02],
                [0.25, 0.19],
                [0.32, 0.20],
            ),
            SPINE,
            CHEST,
        );
        kit.rigid(
            FLESH,
            &seg(
                [0.0, 1.38, 0.05],
                [0.0, 1.58, 0.10],
                [0.28, 0.16],
                [0.16, 0.10],
            ),
            CHEST,
        );
        // The head: small, sunk forward between the shoulders, with a heavy jaw.
        kit.add(
            FLESH,
            &seg(
                [0.0, 1.44, -0.06],
                [0.0, 1.58, -0.10],
                [0.075, 0.075],
                [0.085, 0.085],
            ),
            NECK,
            HEAD,
        );
        kit.rigid(
            FLESH,
            &seg(
                [0.0, 1.50, -0.14],
                [0.0, 1.68, -0.13],
                [0.090, 0.090],
                [0.078, 0.080],
            ),
            HEAD,
        );
        kit.rigid(
            FLESH,
            &seg(
                [0.0, 1.46, -0.17],
                [0.0, 1.53, -0.16],
                [0.08, 0.06],
                [0.09, 0.07],
            ),
            HEAD,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                EYES,
                &cube([s * 0.033, 1.61, -0.222], [0.018, 0.012, 0.008]),
                HEAD,
            );
            // Tusks.
            kit.rigid(
                BONE,
                &seg(
                    [s * 0.05, 1.51, -0.22],
                    [s * 0.065, 1.59, -0.24],
                    [0.012, 0.012],
                    [0.003, 0.003],
                ),
                HEAD,
            );
        }
        // Bone spurs through the hump.
        for (index, x) in [-0.12_f32, 0.0, 0.12].into_iter().enumerate() {
            let y = 1.50 + 0.02 * (1 - index as i32).abs() as f32;
            kit.rigid(
                BONE,
                &seg(
                    [x, y, 0.20],
                    [x * 1.2, y + 0.16, 0.30],
                    [0.03, 0.03],
                    [0.004, 0.004],
                ),
                CHEST,
            );
        }
        // A studded belt and a loincloth.
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 0.84, -0.01],
                [0.0, 0.93, -0.01],
                [0.225, 0.17],
                [0.225, 0.17],
            ),
            HIPS,
        );
        kit.rigid(
            LEATHER,
            &seg(
                [0.0, 0.50, -0.16],
                [0.0, 0.86, -0.17],
                [0.12, 0.014],
                [0.14, 0.014],
            ),
            HIPS,
        );

        for side in SIDES {
            let s = side.sign;
            // Arms like legs: the upper arm thick as a thigh, the forearm thicker still, and
            // fists as big as the head.
            kit.rigid(
                FLESH,
                &cube([s * 0.26, 1.40, 0.0], [0.12, 0.12, 0.12]),
                side.shoulder,
            );
            kit.add(
                FLESH,
                &bar(
                    [s * 0.26, 1.39, 0.0],
                    [s * 0.42, 1.39, 0.0],
                    [0.105, 0.10],
                    [0.09, 0.09],
                ),
                side.shoulder,
                side.forearm,
            );
            kit.add(
                FLESH,
                &bar(
                    [s * 0.42, 1.39, 0.0],
                    [s * 0.62, 1.39, 0.0],
                    [0.10, 0.10],
                    [0.115, 0.11],
                ),
                side.forearm,
                side.hand,
            );
            kit.rigid(
                FLESH,
                &bar(
                    [s * 0.62, 1.39, 0.0],
                    [s * 0.80, 1.38, 0.0],
                    [0.12, 0.11],
                    [0.10, 0.10],
                ),
                side.hand,
            );
            // Shackles with a length of broken chain.
            kit.rigid(
                IRON,
                &bar(
                    [s * 0.55, 1.39, 0.0],
                    [s * 0.62, 1.39, 0.0],
                    [0.13, 0.13],
                    [0.13, 0.13],
                ),
                side.forearm,
            );
            kit.rigid(
                IRON,
                &seg(
                    [s * 0.58, 1.26, 0.0],
                    [s * 0.58, 1.39, 0.0],
                    [0.018, 0.018],
                    [0.018, 0.018],
                ),
                side.forearm,
            );

            // Short, thick legs.
            let x = s * 0.12;
            kit.add(
                FLESH,
                &seg([x, 0.46, 0.0], [x, 0.90, 0.0], [0.10, 0.10], [0.13, 0.13]),
                side.shin,
                side.thigh,
            );
            kit.add(
                FLESH,
                &seg([x, 0.08, 0.0], [x, 0.46, 0.0], [0.085, 0.085], [0.10, 0.10]),
                side.foot,
                side.shin,
            );
            kit.rigid(
                FLESH,
                &seg(
                    [x, 0.0, -0.05],
                    [x, 0.10, -0.03],
                    [0.09, 0.14],
                    [0.085, 0.10],
                ),
                side.foot,
            );
        }
        kit.finish()
    }
    let style = Style {
        arm_down_l: -64.0,
        arm_down_r: 64.0,
        walk_period: 1.3,
        run_period: 0.78,
        stride: 18.0,
        knee: 22.0,
        arm_swing: 14.0,
        bob: 0.04,
        lean: 8.0,
        breath: 2.3,
        cast: CastStyle::Slam,
    };
    Design {
        style,
        posture: Posture {
            hunch: 16.0,
            chest: 8.0,
            head_up: 22.0,
            crouch: 12.0,
            arms_forward: 12.0,
        },
        speeds: (1.4, 5.0),
        materials: vec![
            textured("brute_flesh", 0, 0.75, 0.0),
            glow("brute_eyes", [1.0, 0.35, 0.18], 3.0),
            textured("brute_iron", 1, 0.5, 0.75),
            textured("brute_leather", 2, 0.75, 0.0),
            textured("brute_bone", 3, 0.8, 0.0),
        ],
        textures: vec![
            flesh_texture("brute_flesh", [0.50, 0.55, 0.46], [0.30, 0.32, 0.30], true),
            metal_texture("brute_iron", [0.28, 0.28, 0.30], 0.6),
            leather_texture("brute_leather", [0.20, 0.14, 0.09]),
            bone_texture("brute_bone", [0.80, 0.76, 0.64]),
        ],
        geometry,
    }
}

// ----------------------------------------------------------------------------- the ghoul

fn ghoul() -> Design {
    const FLESH: usize = 0;
    const EYES: usize = 1;
    const CLAW: usize = 2;
    const RAG: usize = 3;
    fn geometry(_style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["flesh", "eyes", "claws", "rags"]);
        // A starved torso: narrow at the waist, a ridge of spine along the back.
        kit.add(
            FLESH,
            &seg(
                [0.0, 0.88, 0.0],
                [0.0, 1.14, 0.01],
                [0.12, 0.09],
                [0.13, 0.10],
            ),
            HIPS,
            SPINE,
        );
        kit.add(
            FLESH,
            &seg(
                [0.0, 1.12, 0.01],
                [0.0, 1.42, 0.02],
                [0.14, 0.10],
                [0.19, 0.12],
            ),
            SPINE,
            CHEST,
        );
        for (index, y) in [1.00_f32, 1.12, 1.24, 1.36].into_iter().enumerate() {
            let bone = if index < 2 { SPINE } else { CHEST };
            kit.rigid(
                CLAW,
                &seg(
                    [0.0, y, 0.10],
                    [0.0, y + 0.03, 0.17],
                    [0.018, 0.02],
                    [0.004, 0.004],
                ),
                bone,
            );
        }
        // A long skull, pushed forward on the neck, with a hanging jaw.
        kit.add(
            FLESH,
            &seg(
                [0.0, 1.40, 0.0],
                [0.0, 1.54, -0.03],
                [0.05, 0.05],
                [0.055, 0.055],
            ),
            NECK,
            HEAD,
        );
        kit.rigid(
            FLESH,
            &seg(
                [0.0, 1.54, -0.04],
                [0.0, 1.72, 0.02],
                [0.080, 0.105],
                [0.065, 0.08],
            ),
            HEAD,
        );
        kit.rigid(
            FLESH,
            &seg(
                [0.0, 1.46, -0.10],
                [0.0, 1.55, -0.08],
                [0.05, 0.05],
                [0.065, 0.06],
            ),
            HEAD,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                EYES,
                &cube([s * 0.034, 1.62, -0.145], [0.016, 0.011, 0.008]),
                HEAD,
            );
            kit.rigid(
                CLAW,
                &seg(
                    [s * 0.03, 1.535, -0.13],
                    [s * 0.03, 1.50, -0.13],
                    [0.006, 0.006],
                    [0.002, 0.002],
                ),
                HEAD,
            );
            // Pointed ears.
            kit.rigid(
                FLESH,
                &seg(
                    [s * 0.07, 1.64, 0.0],
                    [s * 0.15, 1.72, 0.04],
                    [0.012, 0.03],
                    [0.002, 0.004],
                ),
                HEAD,
            );
        }
        // Rags at the hips.
        kit.rigid(
            RAG,
            &seg(
                [0.0, 0.62, 0.0],
                [0.0, 0.94, 0.0],
                [0.16, 0.12],
                [0.13, 0.10],
            ),
            HIPS,
        );

        for side in SIDES {
            let s = side.sign;
            // Long, thin arms ending in hooked claws that reach past the knees.
            kit.add(
                FLESH,
                &bar(
                    [s * 0.15, 1.39, 0.0],
                    [s * 0.42, 1.39, 0.0],
                    [0.045, 0.045],
                    [0.035, 0.035],
                ),
                side.shoulder,
                side.forearm,
            );
            kit.add(
                FLESH,
                &bar(
                    [s * 0.42, 1.39, 0.0],
                    [s * 0.64, 1.39, 0.0],
                    [0.035, 0.035],
                    [0.03, 0.03],
                ),
                side.forearm,
                side.hand,
            );
            kit.rigid(
                FLESH,
                &bar(
                    [s * 0.64, 1.39, 0.0],
                    [s * 0.74, 1.39, 0.0],
                    [0.045, 0.02],
                    [0.05, 0.018],
                ),
                side.hand,
            );
            for finger in [-1.0_f32, 0.0, 1.0] {
                let z = finger * 0.022;
                kit.rigid(
                    CLAW,
                    &bar(
                        [s * 0.74, 1.39, z],
                        [s * 0.92, 1.36, z * 1.4],
                        [0.012, 0.008],
                        [0.002, 0.002],
                    ),
                    side.hand,
                );
            }
            // Wiry legs and long feet.
            let x = s * 0.095;
            kit.add(
                FLESH,
                &seg([x, 0.46, 0.0], [x, 0.88, 0.0], [0.045, 0.045], [0.06, 0.06]),
                side.shin,
                side.thigh,
            );
            kit.add(
                FLESH,
                &seg(
                    [x, 0.07, 0.0],
                    [x, 0.46, 0.0],
                    [0.035, 0.035],
                    [0.045, 0.045],
                ),
                side.foot,
                side.shin,
            );
            kit.rigid(
                FLESH,
                &seg(
                    [x, 0.0, -0.06],
                    [x, 0.07, -0.05],
                    [0.042, 0.12],
                    [0.036, 0.08],
                ),
                side.foot,
            );
            kit.rigid(
                CLAW,
                &seg(
                    [x, 0.0, -0.19],
                    [x, 0.03, -0.23],
                    [0.03, 0.01],
                    [0.004, 0.004],
                ),
                side.foot,
            );
        }
        kit.finish()
    }
    let style = Style {
        arm_down_l: -66.0,
        arm_down_r: 66.0,
        walk_period: 0.8,
        run_period: 0.48,
        stride: 26.0,
        knee: 32.0,
        arm_swing: 24.0,
        bob: 0.03,
        lean: 10.0,
        breath: 1.0,
        cast: CastStyle::Claw,
    };
    Design {
        style,
        posture: Posture {
            hunch: 28.0,
            chest: 10.0,
            head_up: 34.0,
            crouch: 30.0,
            arms_forward: 20.0,
        },
        speeds: (1.8, 6.0),
        materials: vec![
            textured("ghoul_flesh", 0, 0.7, 0.0),
            glow("ghoul_eyes", [1.0, 0.85, 0.25], 3.4),
            textured("ghoul_claws", 1, 0.6, 0.0),
            textured("ghoul_rags", 2, 0.95, 0.0),
        ],
        textures: vec![
            flesh_texture("ghoul_flesh", [0.46, 0.50, 0.40], [0.22, 0.26, 0.22], false),
            bone_texture("ghoul_claws", [0.62, 0.58, 0.46]),
            cloth_texture("ghoul_rags", [0.20, 0.17, 0.13], None),
        ],
        geometry,
    }
}

// ------------------------------------------------------------------------------ the boss

fn ashgrave() -> Design {
    const PLATE: usize = 0;
    const EMBER: usize = 1;
    const CAPE: usize = 2;
    const STEEL: usize = 3;
    const BONE: usize = 4;
    fn geometry(style: &Style) -> Vec<SubMesh> {
        let mut kit = Kit::new(&["plate", "ember", "cape", "blade", "bone"]);
        // Plate from the faulds to the gorget, flaring wide at the chest.
        kit.rigid(
            PLATE,
            &seg(
                [0.0, 0.62, 0.0],
                [0.0, 0.97, 0.0],
                [0.21, 0.16],
                [0.18, 0.14],
            ),
            HIPS,
        );
        kit.add(
            PLATE,
            &seg(
                [0.0, 0.95, 0.0],
                [0.0, 1.18, 0.0],
                [0.18, 0.135],
                [0.22, 0.15],
            ),
            HIPS,
            SPINE,
        );
        kit.add(
            PLATE,
            &seg(
                [0.0, 1.18, 0.0],
                [0.0, 1.44, 0.0],
                [0.22, 0.15],
                [0.26, 0.16],
            ),
            SPINE,
            CHEST,
        );
        kit.rigid(
            PLATE,
            &seg(
                [0.0, 1.42, 0.0],
                [0.0, 1.52, 0.0],
                [0.15, 0.13],
                [0.10, 0.10],
            ),
            CHEST,
        );
        // The ember seam where breastplate meets faulds, and a burning heart in the chest.
        kit.rigid(
            EMBER,
            &seg(
                [0.0, 0.95, 0.0],
                [0.0, 0.985, 0.0],
                [0.185, 0.14],
                [0.19, 0.142],
            ),
            HIPS,
        );
        kit.rigid(
            EMBER,
            &cube([0.0, 1.30, -0.158], [0.05, 0.06, 0.008]),
            CHEST,
        );
        // Skulls on the belt.
        for x in [-0.12_f32, 0.12] {
            kit.rigid(BONE, &cube([x, 0.88, -0.17], [0.04, 0.045, 0.035]), HIPS);
        }
        // A horned helm and a crown of spikes; two ember eyes in the dark of the visor.
        kit.add(
            PLATE,
            &seg(
                [0.0, 1.50, 0.0],
                [0.0, 1.80, 0.0],
                [0.115, 0.12],
                [0.105, 0.11],
            ),
            NECK,
            HEAD,
        );
        kit.rigid(
            EMBER,
            &cube([0.0, 1.655, -0.118], [0.07, 0.014, 0.006]),
            HEAD,
        );
        for s in [1.0_f32, -1.0] {
            kit.rigid(
                BONE,
                &seg(
                    [s * 0.10, 1.72, 0.0],
                    [s * 0.24, 1.86, 0.02],
                    [0.035, 0.035],
                    [0.02, 0.02],
                ),
                HEAD,
            );
            kit.rigid(
                BONE,
                &seg(
                    [s * 0.24, 1.86, 0.02],
                    [s * 0.26, 2.04, -0.02],
                    [0.02, 0.02],
                    [0.003, 0.003],
                ),
                HEAD,
            );
        }
        for x in [-0.06_f32, 0.0, 0.06] {
            let height = if x == 0.0 { 0.14 } else { 0.09 };
            kit.rigid(
                PLATE,
                &seg(
                    [x, 1.79, -0.06],
                    [x, 1.79 + height, -0.07],
                    [0.018, 0.018],
                    [0.003, 0.003],
                ),
                HEAD,
            );
        }
        // A cape from the shoulders to the calves, hung off the chest.
        kit.rigid(
            CAPE,
            &seg(
                [0.0, 0.34, 0.24],
                [0.0, 1.46, 0.16],
                [0.34, 0.02],
                [0.24, 0.02],
            ),
            CHEST,
        );

        for side in SIDES {
            let s = side.sign;
            // Pauldrons with a spike each: the widest thing on the boss.
            kit.rigid(
                PLATE,
                &bar(
                    [s * 0.16, 1.32, 0.0],
                    [s * 0.38, 1.48, 0.0],
                    [0.16, 0.16],
                    [0.12, 0.13],
                ),
                side.shoulder,
            );
            kit.rigid(
                BONE,
                &seg(
                    [s * 0.30, 1.52, 0.0],
                    [s * 0.36, 1.72, 0.03],
                    [0.03, 0.03],
                    [0.003, 0.003],
                ),
                side.shoulder,
            );
            kit.add(
                PLATE,
                &bar(
                    [s * 0.30, 1.39, 0.0],
                    [s * 0.46, 1.39, 0.0],
                    [0.075, 0.075],
                    [0.065, 0.065],
                ),
                side.shoulder,
                side.forearm,
            );
            kit.add(
                PLATE,
                &bar(
                    [s * 0.46, 1.39, 0.0],
                    [s * 0.66, 1.39, 0.0],
                    [0.07, 0.07],
                    [0.08, 0.08],
                ),
                side.forearm,
                side.hand,
            );
            kit.rigid(
                PLATE,
                &bar(
                    [s * 0.66, 1.39, 0.0],
                    [s * 0.76, 1.385, 0.0],
                    [0.05, 0.035],
                    [0.045, 0.03],
                ),
                side.hand,
            );
            let x = s * 0.10;
            kit.add(
                PLATE,
                &seg([x, 0.46, 0.0], [x, 0.90, 0.0], [0.08, 0.08], [0.10, 0.10]),
                side.shin,
                side.thigh,
            );
            kit.add(
                PLATE,
                &seg([x, 0.10, 0.0], [x, 0.46, 0.0], [0.07, 0.075], [0.08, 0.085]),
                side.foot,
                side.shin,
            );
            kit.rigid(
                PLATE,
                &seg(
                    [x, 0.0, -0.035],
                    [x, 0.14, -0.03],
                    [0.085, 0.14],
                    [0.075, 0.10],
                ),
                side.foot,
            );
        }

        // The greatsword: a long black blade with a burning edge, held forward and low.
        let grip = Grip::right(style);
        kit.rigid(
            PLATE,
            &grip.seg(
                [0.0, 0.02, 0.26],
                [0.0, 0.0, -0.06],
                [0.026, 0.026],
                [0.026, 0.026],
            ),
            PROP,
        );
        kit.rigid(
            BONE,
            &grip.cube([0.0, 0.02, 0.28], [0.04, 0.04, 0.04]),
            PROP,
        );
        kit.rigid(
            PLATE,
            &grip.seg(
                [0.0, 0.0, -0.06],
                [0.0, 0.0, -0.12],
                [0.20, 0.035],
                [0.18, 0.035],
            ),
            PROP,
        );
        kit.rigid(
            STEEL,
            &grip.seg(
                [0.0, -0.01, -0.12],
                [0.0, -0.20, -1.20],
                [0.075, 0.016],
                [0.058, 0.014],
            ),
            PROP,
        );
        kit.rigid(
            EMBER,
            &grip.seg(
                [0.0, -0.01, -0.14],
                [0.0, -0.19, -1.14],
                [0.018, 0.020],
                [0.012, 0.018],
            ),
            PROP,
        );
        kit.rigid(
            STEEL,
            &grip.seg(
                [0.0, -0.20, -1.20],
                [0.0, -0.22, -1.34],
                [0.058, 0.014],
                [0.004, 0.004],
            ),
            PROP,
        );
        kit.finish()
    }
    let style = Style {
        arm_down_l: -70.0,
        arm_down_r: 70.0,
        walk_period: 1.2,
        run_period: 0.70,
        stride: 22.0,
        knee: 26.0,
        arm_swing: 12.0,
        bob: 0.026,
        lean: 5.0,
        breath: 2.2,
        cast: CastStyle::Swing,
    };
    Design {
        style,
        posture: Posture {
            hunch: 4.0,
            head_up: 2.0,
            arms_forward: 6.0,
            ..Posture::default()
        },
        speeds: (1.6, 5.2),
        materials: vec![
            textured("ashgrave_plate", 0, 0.5, 0.55),
            glow("ashgrave_ember", [1.0, 0.45, 0.12], 3.6),
            textured("ashgrave_cape", 1, 0.95, 0.0),
            MaterialAsset::colored("ashgrave_blade", 0.10, 0.10, 0.12)
                .with_roughness(0.25)
                .with_metallic(0.95),
            textured("ashgrave_bone", 2, 0.8, 0.0),
        ],
        textures: vec![
            metal_texture("ashgrave_plate", [0.44, 0.41, 0.42], 0.25),
            cloth_texture(
                "ashgrave_cape",
                [0.30, 0.05, 0.05],
                Some([0.70, 0.45, 0.15]),
            ),
            bone_texture("ashgrave_bone", [0.76, 0.72, 0.60]),
        ],
        geometry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{ValidationLimits, validate_model};

    fn model(name: &str) -> ModelAsset {
        match monster_model(name) {
            Some(Ok(model)) => model,
            Some(Err(error)) => panic!("{name} must build: {error}"),
            None => panic!("{name} is not a monster model"),
        }
    }

    #[test]
    fn every_monster_passes_the_validation_an_imported_model_would() {
        for name in MONSTER_MODELS {
            let model = model(name);
            let Ok(report) = validate_model(&model, &ValidationLimits::default()) else {
                panic!("{name} must validate");
            };
            assert_eq!(report.bones as usize, humanoid::BONE_COUNT, "{name}");
            assert_eq!(report.clips, 9, "{name}: the hero set and a death");
            assert!(
                (100..1500).contains(&report.triangles),
                "{name}: a stand-in should stay cheap: {} triangles",
                report.triangles
            );
        }
    }

    #[test]
    fn every_monster_definition_resolves_against_its_own_model() {
        for name in MONSTER_MODELS {
            let model = model(name);
            let Some(def) = monster_character(name) else {
                panic!("{name} has no definition");
            };
            let Ok(clips) = def.resolve(&model) else {
                panic!("{name}'s definition names a clip its model lacks");
            };
            assert!(
                clips.get(AnimationSlot::Death).is_some(),
                "{name} cannot die"
            );
            for slot in AnimationSlot::REQUIRED {
                assert!(clips.get(slot).is_some(), "{name} leaves {slot:?} empty");
            }
        }
    }

    #[test]
    fn every_monster_stands_on_the_floor() {
        for name in MONSTER_MODELS {
            let model = model(name);
            let Some(lod) = model.lod0() else {
                panic!("{name} has no LOD0");
            };
            let lowest = lod
                .submeshes
                .iter()
                .flat_map(|submesh| submesh.geometry.positions())
                .map(|position| position.y)
                .fold(f32::MAX, f32::min);
            assert!(lowest.abs() < 0.03, "{name}'s feet are at y = {lowest}");
        }
    }

    #[test]
    fn the_monsters_do_not_share_a_silhouette() {
        // Seven stand-ins that were one model tinted seven ways would pass every other test
        // here. Their bind-pose extents differ, which a tint cannot fake.
        let mut extents: Vec<(String, [i32; 3])> = Vec::new();
        for name in MONSTER_MODELS {
            let model = model(name);
            let Some(lod) = model.lod0() else {
                panic!("{name} has no LOD0");
            };
            let mut max = [0.0_f32; 3];
            let mut triangles = 0;
            for submesh in &lod.submeshes {
                triangles += submesh.indices.len() / 3;
                for position in submesh.geometry.positions() {
                    max[0] = max[0].max(position.x.abs());
                    max[1] = max[1].max(position.y);
                    max[2] = max[2].max(position.z.abs());
                }
            }
            let key = [
                (max[0] * 100.0) as i32,
                (max[1] * 100.0) as i32,
                triangles as i32,
            ];
            assert!(
                extents.iter().all(|(_, other)| *other != key),
                "{name} has the same extents and triangle count as another monster"
            );
            extents.push((name.to_owned(), key));
        }
    }

    #[test]
    fn a_crouch_keeps_the_feet_under_the_hips() {
        let posture = Posture {
            crouch: 30.0,
            ..Posture::default()
        };
        let (knee, drop) = posture.crouch_geometry();
        assert!(knee < -50.0 && knee > -75.0, "knee {knee}");
        assert!(drop > 0.05 && drop < 0.2, "drop {drop}");
        let straight = Posture::default().crouch_geometry();
        assert!(straight.0.abs() < 1e-4 && straight.1.abs() < 1e-4);
    }

    #[test]
    fn a_monster_the_module_does_not_build_is_none_not_a_panic() {
        assert!(monster_model("training_dummy").is_none());
        assert!(monster_character("nobody").is_none());
    }
}
