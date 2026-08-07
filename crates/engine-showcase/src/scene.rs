//! The showcase arena: procedural geometry, a palette of materials, and the object layout.
//!
//! Everything here goes through the real engine types. Meshes are uploaded with
//! [`engine_renderer::Renderer::create_mesh`], materials with `create_material`, and every
//! drawable is an entity in a [`RenderWorld`] — the same path the game will use. There is no
//! second rendering implementation for the demo, which is the point: what is on screen is
//! evidence about the renderer, not about the showcase.

use engine_core::math::{Quat, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_renderer::error::RendererError;
use engine_renderer::mesh::MeshData;
use engine_renderer::{
    MaterialDesc, MaterialHandle, MeshHandle, RenderEntity, RenderEntityDesc, RenderWorld, Renderer,
};

use crate::geometry::cylinder;

/// Half-width of the ground plane, in metres.
///
/// Large enough that the free-fly camera can travel for a while without running off the
/// edge, which is what gives the scene a sense of scale.
const GROUND_HALF_EXTENT: f32 = 500.0;

/// Radius the scattered field is distributed over.
///
/// Far larger than the camera can see at once, so most of it is outside the frustum and
/// culling has something real to reject.
const FIELD_RADIUS: f32 = 1400.0;

/// Objects in the hand-placed part of the scene, before the scattered field starts.
///
/// The hand-placed set is ordered from most to least interesting, so even the ten-object
/// preset shows perspective, depth and lighting rather than ten identical cubes.
const SHOWCASE_CAPACITY: usize = 128;

/// How many objects animate.
///
/// Bounded on purpose: transform updates are the showcase's own cost, not the renderer's,
/// and animating 100,000 objects would put the showcase's `update` in every measurement of
/// the renderer's `render`.
const ANIMATED_LIMIT: usize = 96;

/// The preset object counts, smallest first. F3 to F7 select them.
pub const PRESETS: [usize; 5] = [10, 100, 1_000, 10_000, 100_000];

/// Human-readable names for [`PRESETS`], for the overlay.
pub const PRESET_NAMES: [&str; 5] = ["MINIMAL", "SMALL", "MEDIUM", "LARGE", "STRESS"];

/// Which uploaded mesh an object uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Cube,
    Sphere,
    Cylinder,
}

/// What an object does over time.
///
/// Deliberately four fixed behaviours rather than an animation system: the purpose is to
/// prove that transforms are rebuilt and re-culled every frame, not to author motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    /// Never moves.
    Static,
    /// Turns about its own vertical axis.
    Spin,
    /// Rises and falls.
    Bob,
    /// Grows and shrinks.
    Pulse,
    /// Travels along a circle about the arena's centre.
    Orbit,
}

/// One placed object, before it becomes an entity.
#[derive(Debug, Clone, Copy)]
struct Placement {
    shape: Shape,
    material: usize,
    transform: Transform,
    motion: Motion,
    /// Offset into the animation cycle, so a row of objects does not move in lockstep.
    phase: f32,
}

/// An entity whose transform is rewritten every frame.
#[derive(Debug, Clone, Copy)]
struct Animated {
    entity: RenderEntity,
    rest: Transform,
    bounds: Sphere,
    motion: Motion,
    phase: f32,
}

/// The uploaded meshes, the palette, and the world built from them.
pub struct Scene {
    cube: MeshHandle,
    sphere: MeshHandle,
    cylinder: MeshHandle,
    ground: MeshHandle,
    cube_bounds: Sphere,
    sphere_bounds: Sphere,
    cylinder_bounds: Sphere,
    ground_bounds: Sphere,
    palette: Vec<MaterialHandle>,
    ground_material: MaterialHandle,
    /// What the renderer draws.
    pub world: RenderWorld,
    animated: Vec<Animated>,
    /// Index into [`PRESETS`] of the object count currently loaded.
    preset: usize,
    /// Triangles the loaded world would draw if nothing were culled.
    scene_triangles: u64,
    /// Triangles in each mesh, indexed the way [`Shape`] is used.
    triangles_per_shape: [u32; 3],
    ground_triangles: u32,
}

impl Scene {
    /// Uploads the meshes and materials and builds the default preset.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if a mesh or material cannot be created.
    pub fn new(renderer: &mut Renderer, preset: usize) -> Result<Self, RendererError> {
        // Segment counts are a compromise: high enough that a sphere reads as round under
        // the directional light, low enough that 100,000 of them is a draw-call test rather
        // than a vertex-throughput test.
        let cube_data = MeshData::cube(1.0);
        let sphere_data = MeshData::uv_sphere(0.5, 20, 12);
        let cylinder_data = cylinder(0.5, 1.0, 20)?;
        let ground_data = MeshData::plane(GROUND_HALF_EXTENT * 2.0);

        let triangles_per_shape = [
            cube_data.triangle_count() as u32,
            sphere_data.triangle_count() as u32,
            cylinder_data.triangle_count() as u32,
        ];
        let ground_triangles = ground_data.triangle_count() as u32;

        let cube_bounds = cube_data.bounds();
        let sphere_bounds = sphere_data.bounds();
        let cylinder_bounds = cylinder_data.bounds();
        let ground_bounds = ground_data.bounds();

        let cube = renderer.create_mesh(&cube_data)?;
        let sphere = renderer.create_mesh(&sphere_data)?;
        let cylinder_mesh = renderer.create_mesh(&cylinder_data)?;
        let ground = renderer.create_mesh(&ground_data)?;

        // A small readable palette rather than a colour per object: distinct hues make the
        // shading of a surface legible, and a thousand near-identical colours do not.
        let palette_colors = [
            (0.86, 0.28, 0.24),
            (0.95, 0.70, 0.22),
            (0.35, 0.72, 0.40),
            (0.24, 0.55, 0.85),
            (0.62, 0.38, 0.80),
            (0.90, 0.90, 0.86),
            (0.20, 0.72, 0.72),
            (0.85, 0.50, 0.30),
        ];
        let mut palette = Vec::with_capacity(palette_colors.len());
        for (index, (red, green, blue)) in palette_colors.into_iter().enumerate() {
            // Roughness varies across the palette so the light reads differently on
            // neighbouring objects; it is a look, not a claim to be physically based.
            let roughness = 0.35 + (index as f32 / palette_colors.len() as f32) * 0.55;
            palette.push(renderer.create_material(
                MaterialDesc::colored(red, green, blue).with_roughness(roughness),
            )?);
        }
        let ground_material = renderer
            .create_material(MaterialDesc::colored(0.16, 0.20, 0.17).with_roughness(0.95))?;

        let mut scene = Self {
            cube,
            sphere,
            cylinder: cylinder_mesh,
            ground,
            cube_bounds,
            sphere_bounds,
            cylinder_bounds,
            ground_bounds,
            palette,
            ground_material,
            world: RenderWorld::new(),
            animated: Vec::with_capacity(ANIMATED_LIMIT),
            preset: preset.min(PRESETS.len() - 1),
            scene_triangles: 0,
            triangles_per_shape,
            ground_triangles,
        };
        let preset = scene.preset;
        scene.load(preset);
        Ok(scene)
    }

    /// Which preset is loaded.
    #[must_use]
    pub fn preset(&self) -> usize {
        self.preset
    }

    /// The name of the loaded preset.
    #[must_use]
    pub fn preset_name(&self) -> &'static str {
        PRESET_NAMES[self.preset.min(PRESET_NAMES.len() - 1)]
    }

    /// Objects in the world, including the ground.
    #[must_use]
    pub fn object_count(&self) -> usize {
        self.world.len()
    }

    /// Triangles the world holds, before culling removes any.
    #[must_use]
    pub fn scene_triangles(&self) -> u64 {
        self.scene_triangles
    }

    /// Rebuilds the world for preset `index`.
    ///
    /// Meshes and materials are already on the GPU and are not touched, so switching
    /// presets costs a world rebuild and nothing else — no upload, no device stall.
    pub fn load(&mut self, index: usize) {
        let index = index.min(PRESETS.len() - 1);
        let target = PRESETS[index];
        self.preset = index;
        self.world.clear();
        self.animated.clear();
        self.scene_triangles = 0;

        // The ground first, so it exists at every preset: without it there is no horizon
        // and no sense of where the objects are.
        self.world.spawn(RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh: self.ground,
            material: self.ground_material,
            // Local bounds: `RenderWorld::spawn` places them in world space itself, and
            // pre-transforming them here would apply the transform twice.
            bounds: self.ground_bounds,
            visible: true,
        });
        self.scene_triangles += u64::from(self.ground_triangles);

        let remaining = target.saturating_sub(1);
        let mut placements = Vec::with_capacity(SHOWCASE_CAPACITY);
        build_showcase(&mut placements);
        placements.truncate(remaining);
        let scattered = remaining - placements.len();
        build_field(&mut placements, scattered);

        for placement in &placements {
            self.spawn(placement);
        }

        log::info!(
            "preset {} ({}): {} objects, {} triangles",
            index + 1,
            self.preset_name(),
            self.world.len(),
            self.scene_triangles
        );
    }

    /// Adds one placement to the world, recording it if it animates.
    fn spawn(&mut self, placement: &Placement) {
        let (mesh, local_bounds, triangles) = match placement.shape {
            Shape::Cube => (self.cube, self.cube_bounds, self.triangles_per_shape[0]),
            Shape::Sphere => (self.sphere, self.sphere_bounds, self.triangles_per_shape[1]),
            Shape::Cylinder => (
                self.cylinder,
                self.cylinder_bounds,
                self.triangles_per_shape[2],
            ),
        };
        let material = self.palette[placement.material % self.palette.len()];
        let entity = self.world.spawn(RenderEntityDesc {
            transform: placement.transform,
            mesh,
            material,
            bounds: local_bounds,
            visible: true,
        });
        self.scene_triangles += u64::from(triangles);

        if placement.motion != Motion::Static && self.animated.len() < ANIMATED_LIMIT {
            self.animated.push(Animated {
                entity,
                rest: placement.transform,
                bounds: local_bounds,
                motion: placement.motion,
                phase: placement.phase,
            });
        }
    }

    /// Advances the animated objects to `seconds` since the scene was loaded.
    ///
    /// Absolute time rather than an accumulated delta: a paused or slowed frame then leaves
    /// the scene where it should be instead of somewhere that depends on the frame rate.
    pub fn animate(&mut self, seconds: f32) {
        for animated in &self.animated {
            let time = seconds + animated.phase;
            let transform = match animated.motion {
                Motion::Static => animated.rest,
                Motion::Spin => {
                    let angle = time * 0.9;
                    animated.rest.with_rotation(
                        Quat::from_axis_angle(Vec3::Y, angle) * animated.rest.rotation,
                    )
                }
                Motion::Bob => {
                    let lift = time.sin() * 1.4 + 1.4;
                    animated
                        .rest
                        .with_translation(animated.rest.translation + Vec3::new(0.0, lift, 0.0))
                }
                Motion::Pulse => {
                    let scale = 1.0 + time.sin() * 0.22;
                    animated.rest.with_scale(animated.rest.scale * scale)
                }
                Motion::Orbit => {
                    let radius = animated
                        .rest
                        .translation
                        .x
                        .hypot(animated.rest.translation.z);
                    let angle = time * 0.25
                        + animated
                            .rest
                            .translation
                            .z
                            .atan2(animated.rest.translation.x);
                    animated.rest.with_translation(Vec3::new(
                        angle.cos() * radius,
                        animated.rest.translation.y,
                        angle.sin() * radius,
                    ))
                }
            };
            // Re-derives the world bound from the new transform, which is what keeps the
            // culler correct for a moving object. A cached bound is how a moving object
            // ends up culled while it is on screen.
            self.world
                .set_transform(animated.entity, transform, animated.bounds);
        }
    }
}

/// Fills `out` with the hand-placed part of the arena, most interesting first.
fn build_showcase(out: &mut Vec<Placement>) {
    // Four large objects near the camera. These are what make the first frame read as a 3D
    // scene rather than as a pattern: they are close, they occlude the rows behind them, and
    // their scale sets the perspective.
    let large = [
        // Raised by a little more than its radius: the pulse grows it by a fifth, and a
        // sphere resting exactly on the ground would sink into it at the top of the cycle.
        (
            Shape::Sphere,
            Vec3::new(-16.0, 7.6, 18.0),
            12.0,
            3,
            Motion::Pulse,
        ),
        (
            Shape::Cube,
            Vec3::new(16.0, 5.0, 20.0),
            10.0,
            0,
            Motion::Spin,
        ),
        (
            Shape::Cylinder,
            Vec3::new(0.0, 7.0, 6.0),
            14.0,
            4,
            Motion::Spin,
        ),
        // The one object that travels: a slow orbit about the arena, so something in the
        // scene is visibly changing position rather than only turning in place.
        (
            Shape::Cube,
            Vec3::new(-30.0, 4.0, 2.0),
            8.0,
            6,
            Motion::Orbit,
        ),
    ];
    for (index, (shape, position, size, material, motion)) in large.into_iter().enumerate() {
        out.push(Placement {
            shape,
            material,
            transform: Transform::from_translation(position)
                .with_scale(Vec3::splat(size))
                .with_rotation(Quat::from_axis_angle(Vec3::Y, index as f32 * 0.7)),
            motion,
            phase: index as f32 * 1.3,
        });
    }

    // A deliberately overlapping cluster: five objects at different depths on nearly the
    // same screen position. If the depth buffer were wrong, this is where it would show —
    // the far ones would paint over the near ones as the camera turns.
    for step in 0..5 {
        let depth = 24.0 - step as f32 * 5.0;
        let shape = match step % 3 {
            0 => Shape::Sphere,
            1 => Shape::Cube,
            _ => Shape::Cylinder,
        };
        out.push(Placement {
            shape,
            material: step + 1,
            transform: Transform::from_translation(Vec3::new(
                34.0 + step as f32 * 0.6,
                2.5 + step as f32 * 0.35,
                depth,
            ))
            .with_scale(Vec3::splat(4.0)),
            motion: Motion::Static,
            phase: 0.0,
        });
    }

    // Three grids, one per shape, at different distances. Rows demonstrate perspective
    // directly: the same object at the same size, smaller the further back it is.
    let groups = [
        (Shape::Cube, -14.0_f32, Motion::Spin),
        (Shape::Sphere, -34.0, Motion::Bob),
        (Shape::Cylinder, -54.0, Motion::Spin),
    ];
    for (group, (shape, base_z, motion)) in groups.into_iter().enumerate() {
        for row in 0..4_u32 {
            for column in 0..6_u32 {
                let index = row * 6 + column;
                let x = (column as f32 - 2.5) * 7.0;
                let z = base_z - row as f32 * 6.0;
                // Scale and rotation vary per object so the grid shows transforms rather
                // than one transform repeated.
                let scale = 2.0 + ((index % 5) as f32) * 0.55;
                out.push(Placement {
                    shape,
                    material: (group * 3 + index as usize) % 8,
                    transform: Transform::from_translation(Vec3::new(x, scale * 0.5, z))
                        .with_scale(Vec3::splat(scale))
                        .with_rotation(Quat::from_axis_angle(
                            Vec3::Y,
                            index as f32 * 0.31 + group as f32,
                        )),
                    motion,
                    phase: index as f32 * 0.4 + group as f32,
                });
            }
        }
    }
}

/// Appends `count` objects scattered over the field.
///
/// Deterministic: the same count always produces the same scene, so two runs of the stress
/// preset are comparable measurements rather than two different workloads.
fn build_field(out: &mut Vec<Placement>, count: usize) {
    let mut random = Rng::new(0x5EED_1234_ABCD_0001);
    out.reserve(count);
    for index in 0..count {
        // Square-rooted radius, so the objects are spread evenly over the disc instead of
        // bunching at its centre — a bunched field would be culled all at once and would say
        // nothing about how culling scales.
        let radius = random.unit().sqrt() * FIELD_RADIUS;
        let angle = random.unit() * std::f32::consts::TAU;
        let scale = 1.2 + random.unit() * 3.4;
        let shape = match index % 3 {
            0 => Shape::Cube,
            1 => Shape::Sphere,
            _ => Shape::Cylinder,
        };
        out.push(Placement {
            shape,
            material: index % 8,
            transform: Transform::from_translation(Vec3::new(
                angle.cos() * radius,
                scale * 0.5 + random.unit() * 6.0,
                angle.sin() * radius,
            ))
            .with_scale(Vec3::splat(scale))
            .with_rotation(Quat::from_axis_angle(
                Vec3::Y,
                random.unit() * std::f32::consts::TAU,
            )),
            motion: Motion::Static,
            phase: 0.0,
        });
    }
}

/// A small deterministic generator (SplitMix64).
///
/// Not `rand`: the showcase needs reproducibility and eight lines of arithmetic, not a
/// dependency. Not cryptographic and not used for anything that needs to be.
struct Rng {
    state: u64,
}

impl Rng {
    /// Seeds the generator.
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 bits.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in [0, 1).
    fn unit(&mut self) -> f32 {
        // The top 24 bits, which is exactly the mantissa an f32 can hold without rounding.
        (self.next_u64() >> 40) as f32 / (1_u32 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_grow_and_end_at_the_stress_count() {
        assert!(PRESETS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(PRESETS[0], 10);
        assert_eq!(PRESETS[PRESETS.len() - 1], 100_000);
        assert_eq!(PRESETS.len(), PRESET_NAMES.len());
    }

    #[test]
    fn the_hand_placed_scene_fits_its_reserved_capacity() {
        let mut placements = Vec::new();
        build_showcase(&mut placements);
        assert!(!placements.is_empty());
        assert!(
            placements.len() <= SHOWCASE_CAPACITY,
            "{} placements exceed the reserved {SHOWCASE_CAPACITY}",
            placements.len()
        );
    }

    #[test]
    fn the_smallest_preset_still_shows_the_large_objects() {
        // Ten objects must not be ten identical cubes in a row; the interesting ones are
        // ordered first so a truncated scene keeps them.
        let mut placements = Vec::new();
        build_showcase(&mut placements);
        placements.truncate(PRESETS[0] - 1);
        assert!(placements.iter().any(|p| p.transform.scale.x >= 8.0));
        assert!(placements.iter().any(|p| p.shape == Shape::Sphere));
        assert!(placements.iter().any(|p| p.shape == Shape::Cube));
    }

    #[test]
    fn the_field_is_deterministic() {
        let mut first = Vec::new();
        build_field(&mut first, 256);
        let mut second = Vec::new();
        build_field(&mut second, 256);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.transform.translation, b.transform.translation);
        }
    }

    #[test]
    fn the_field_stays_inside_its_radius_and_above_the_ground() {
        let mut placements = Vec::new();
        build_field(&mut placements, 4096);
        for placement in &placements {
            let position = placement.transform.translation;
            let distance = position.x.hypot(position.z);
            assert!(distance <= FIELD_RADIUS + 1e-3, "{position:?}");
            assert!(position.y > 0.0, "{position:?}");
        }
    }

    #[test]
    fn the_field_spreads_rather_than_bunching_at_the_centre() {
        // A uniform angle with a uniform radius piles objects at the origin, which would
        // cull as a single blob and say nothing about how culling scales.
        let mut placements = Vec::new();
        build_field(&mut placements, 4096);
        let inner = placements
            .iter()
            .filter(|placement| {
                let position = placement.transform.translation;
                position.x.hypot(position.z) < FIELD_RADIUS * 0.5
            })
            .count();
        // A uniform disc puts a quarter of the objects inside half the radius.
        let fraction = inner as f32 / placements.len() as f32;
        assert!((0.2..0.3).contains(&fraction), "{fraction}");
    }

    #[test]
    fn the_generator_stays_in_the_unit_interval() {
        let mut random = Rng::new(1);
        for _ in 0..10_000 {
            let value = random.unit();
            assert!((0.0..1.0).contains(&value), "{value}");
        }
    }
}
