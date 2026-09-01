//! Placing an environment scene into a `RenderWorld`, and keeping its levels of detail
//! current.
//!
//! ```text
//! EnvironmentScene          EnvironmentVisuals            RenderWorld
//! ----------------          ------------------            -----------
//! asset id          ──────► ModelBinding (uploaded once)
//! transform         ─────────────────────────────────────► Transform
//!                           ModelPart.mesh + .material ──► MeshHandle + MaterialHandle
//!                           ModelPart.bounds           ──► Sphere
//! collision                 (never crosses — that is engine-game's copy of the scene)
//! ```
//!
//! # Why the environment is spawned once and never re-extracted
//!
//! `RenderExtractor::extract` walks *gameplay* slots and rebuilds what stands for each of
//! them every frame, because a dummy moves, is hit, dies and is reset. An environment object
//! does none of those things: its transform is a constant of the scene. So it is spawned
//! into the same `RenderWorld` once, at load, and after that it costs the per-frame path
//! exactly nothing — no extraction, no transform write, no style write. It still participates
//! fully in culling and batching, because those read the world's arrays and do not care who
//! filled them.
//!
//! The one thing that does change is which level of detail each object is drawn at, and
//! [`EnvironmentPlacer::update_lod`] is the whole of that: one distance per object per frame,
//! and a respawn only for the objects that crossed a threshold.
//!
//! # How instancing happens without an instancing system
//!
//! It falls out of sharing. Each asset is uploaded **once**, by
//! [`EnvironmentVisuals::upload`], and every placement of it refers to the `MeshHandle` and
//! `MaterialHandle` that upload returned. The renderer's
//! `RenderList::group_by_material_and_mesh` then finds every surviving placement of
//! `rock_small` contiguous under one `(material, mesh)` pair and issues one
//! `DrawIndexedInstanced` for all of them.
//!
//! So the answer to *"how would we add 1,000 instances of the same rock without creating
//! 1,000 GPU resources?"* is: push 1,000 `EnvironmentObject`s naming `rock_small` into the
//! scene, and change nothing else. The upload count is the number of distinct assets, and
//! the draw count is the number of distinct `(mesh, material)` pairs that survive the cull.
//! That is asserted in this module's tests rather than described.

use engine_core::math::{Sphere, Vec3};
use engine_environment::{EnvironmentAsset, EnvironmentObject, EnvironmentScene};
use engine_render_data::{
    InstanceStyle, Lighting, LocalLight, MeshHandle, ModelBinding, PartGeometry, RenderEntity,
    RenderEntityDesc, RenderWorld, SHADOW_LOD_MESHES, ShadowCasting,
};

/// The uploaded model for every asset a scene can name.
///
/// Built once, at load, from `Renderer::create_model` — or, in a test, from handles nothing
/// uploaded, because nothing here ever dereferences a handle.
#[derive(Debug, Default, Clone)]
pub struct EnvironmentVisuals {
    /// Parallel to [`EnvironmentVisuals::bindings`]. A `Vec` rather than a `HashMap` for the
    /// same reason `ModelLibrary` is one: sixteen entries, resolved once per placement at
    /// load, and a hash per placement per frame would be the only cost a map ever added.
    assets: Vec<EnvironmentAsset>,
    bindings: Vec<ModelBinding>,
}

impl EnvironmentVisuals {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the binding for `asset`, replacing any already registered.
    ///
    /// Replacement rather than rejection, because that is what a hot reload does and because
    /// a caller that uploads the same asset twice has a bug that a silent first-wins would
    /// hide behind stale geometry.
    pub fn insert(&mut self, asset: EnvironmentAsset, binding: ModelBinding) {
        if let Some(existing) = self.assets.iter().position(|known| *known == asset) {
            self.bindings[existing] = binding;
            return;
        }
        self.assets.push(asset);
        self.bindings.push(binding);
    }

    /// The binding for `asset`, if it was uploaded.
    #[must_use]
    pub fn get(&self, asset: EnvironmentAsset) -> Option<&ModelBinding> {
        let index = self.assets.iter().position(|known| *known == asset)?;
        self.bindings.get(index)
    }

    /// How many assets are registered. The number of `create_model` calls that were made.
    #[must_use]
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }
}

/// What the last placement or level-of-detail pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EnvironmentStats {
    /// Placements the scene asked for.
    pub objects: usize,
    /// Render entities currently standing for them.
    pub entities: usize,
    /// Render entities spawned by the last pass.
    pub spawned: usize,
    /// Render entities despawned by the last pass.
    pub despawned: usize,
    /// Placements whose level of detail changed in the last pass.
    pub lod_changes: usize,
    /// Placements whose asset was not uploaded, and which are therefore not drawn.
    pub unresolved: usize,
}

/// One placement's rendering counterpart.
#[derive(Debug, Clone, PartialEq)]
struct Placed {
    /// Which scene object this stands for.
    object: usize,
    /// One render entity per part of the current level.
    parts: Vec<RenderEntity>,
    /// The level the parts were spawned for.
    lod: usize,
    /// Where the object's centre is, for the distance test. Cached rather than recomputed:
    /// the transform never moves, and the level pass runs over every object every frame.
    center: Vec3,
}

/// Spawns an environment scene into a `RenderWorld` and keeps its detail levels current.
#[derive(Debug, Default)]
pub struct EnvironmentPlacer {
    visuals: EnvironmentVisuals,
    placed: Vec<Placed>,
    /// The scene's objects, copied so the placer owns what it is maintaining. 44 bytes each
    /// and a few hundred of them; borrowing the scene instead would tie the placer's lifetime
    /// to it for no gain.
    objects: Vec<EnvironmentObject>,
    stats: EnvironmentStats,
}

impl EnvironmentPlacer {
    /// A placer over `visuals`, with nothing placed yet.
    #[must_use]
    pub fn new(visuals: EnvironmentVisuals) -> Self {
        Self {
            visuals,
            placed: Vec::new(),
            objects: Vec::new(),
            stats: EnvironmentStats::default(),
        }
    }

    /// The uploaded models.
    #[must_use]
    pub fn visuals(&self) -> &EnvironmentVisuals {
        &self.visuals
    }

    /// The uploaded models, to add one to.
    ///
    /// What the application uses when a level names an asset that was not in the library at
    /// startup — a `.glb` discovered in `assets/`, or one placed by the editor while the game
    /// is running. Placing is otherwise unchanged: the new binding is found by the same
    /// lookup every other placement goes through.
    pub fn visuals_mut(&mut self) -> &mut EnvironmentVisuals {
        &mut self.visuals
    }

    /// What the last pass did.
    #[must_use]
    pub fn stats(&self) -> EnvironmentStats {
        self.stats
    }

    /// How many placements are being maintained.
    #[must_use]
    pub fn len(&self) -> usize {
        self.placed.len()
    }

    /// Whether nothing has been placed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    /// Spawns every object of `scene` into `world`.
    ///
    /// Anything previously placed is removed first, so placing a second scene replaces the
    /// first rather than adding to it — which is what a level change is.
    ///
    /// `camera_position` and `lod_bias` decide the level each object starts at, so the first
    /// frame after a load is already at the right detail rather than spending one frame at
    /// LOD0 for the whole level.
    ///
    /// An object whose asset was never uploaded is **skipped and counted**, not substituted:
    /// [`EnvironmentStats::unresolved`] being non-zero is a load-time bug, and the
    /// application logs it. Drawing a stand-in here would hide it behind geometry, and the
    /// asset library already has a marker model for the case where it should be visible.
    pub fn place(
        &mut self,
        world: &mut RenderWorld,
        scene: &EnvironmentScene,
        camera_position: Vec3,
        lod_bias: f32,
    ) {
        let mut stats = EnvironmentStats::default();
        for placed in self.placed.drain(..) {
            for entity in placed.parts {
                world.despawn(entity);
                stats.despawned += 1;
            }
        }

        // The scene's own lights, published once with the geometry rather than every frame:
        // a brazier does not move, and `RenderWorld` keeps its lights until somebody clears
        // them. A scene with no lights clears the previous scene's, which is what makes
        // loading a second scene not inherit the first one's torches.
        world.clear_lights();
        for light in &scene.lights {
            world.push_light(scene_light(light));
        }

        self.objects.clear();
        self.objects.extend_from_slice(&scene.objects);
        self.placed.reserve(self.objects.len());
        stats.objects = self.objects.len();

        for index in 0..self.objects.len() {
            let object = self.objects[index];
            let Some(binding) = self.visuals.get(object.asset) else {
                stats.unresolved += 1;
                continue;
            };
            let center = object_center(&object, binding);
            let lod = select_lod(binding, center, camera_position, lod_bias);
            let mut parts = Vec::new();
            spawn_parts(world, &object, binding, lod, &mut parts, &mut stats);
            self.placed.push(Placed {
                object: index,
                parts,
                lod,
                center,
            });
        }

        stats.entities = self.placed.iter().map(|placed| placed.parts.len()).sum();
        self.stats = stats;
    }

    /// Removes everything this placer put in `world`.
    pub fn clear(&mut self, world: &mut RenderWorld) {
        for placed in self.placed.drain(..) {
            for entity in placed.parts {
                world.despawn(entity);
            }
        }
        self.objects.clear();
        self.stats = EnvironmentStats::default();
    }

    /// Re-selects each placement's level of detail for a camera at `camera_position`.
    ///
    /// One distance per placement — a subtract, a dot and a compare against a sorted list of
    /// thresholds — and nothing else unless a level actually changed. A placement that did
    /// change has its parts despawned and respawned, because a level is different geometry
    /// and a `RenderWorld` entity's mesh is fixed at spawn.
    ///
    /// Costs nothing at all when the camera is still: no threshold is crossed, so the loop is
    /// a read of a cached centre and a comparison.
    pub fn update_lod(&mut self, world: &mut RenderWorld, camera_position: Vec3, lod_bias: f32) {
        let mut stats = self.stats;
        stats.spawned = 0;
        stats.despawned = 0;
        stats.lod_changes = 0;

        for slot in 0..self.placed.len() {
            let object = self.objects[self.placed[slot].object];
            let Some(binding) = self.visuals.get(object.asset) else {
                continue;
            };
            let level = select_lod(binding, self.placed[slot].center, camera_position, lod_bias);
            if level == self.placed[slot].lod {
                continue;
            }

            // Taken and given back rather than replaced, so the `Vec` keeps its capacity: a
            // level change is a despawn and a respawn of one or two entities, and it must
            // not also be a heap allocation. `env_bench` counts these.
            let mut parts = std::mem::take(&mut self.placed[slot].parts);
            for entity in parts.drain(..) {
                world.despawn(entity);
                stats.despawned += 1;
            }
            spawn_parts(world, &object, binding, level, &mut parts, &mut stats);
            self.placed[slot].parts = parts;
            self.placed[slot].lod = level;
            stats.lod_changes += 1;
        }

        if stats.lod_changes > 0 {
            stats.entities = self.placed.iter().map(|placed| placed.parts.len()).sum();
        }
        self.stats = stats;
    }

    /// The level of detail placement `index` is currently drawn at.
    ///
    /// For a test and for the developer overlay. `None` for an unresolved placement.
    #[must_use]
    pub fn lod_of(&self, index: usize) -> Option<usize> {
        self.placed
            .iter()
            .find(|placed| placed.object == index)
            .map(|placed| placed.lod)
    }

    /// The render entities standing for placement `index`.
    #[must_use]
    pub fn parts_of(&self, index: usize) -> Option<&[RenderEntity]> {
        self.placed
            .iter()
            .find(|placed| placed.object == index)
            .map(|placed| placed.parts.as_slice())
    }
}

/// Spawns one placement's parts at level `lod`.
fn spawn_parts(
    world: &mut RenderWorld,
    object: &EnvironmentObject,
    binding: &ModelBinding,
    lod: usize,
    parts: &mut Vec<RenderEntity>,
    stats: &mut EnvironmentStats,
) {
    let Some(level) = binding.lod(lod) else {
        return;
    };
    parts.reserve(level.parts.len());
    for (index, part) in level.parts.iter().enumerate() {
        let PartGeometry::Rigid(mesh) = part.geometry else {
            // A skinned part on an environment prop. Nothing authors one — every asset in
            // the library is rigid, and a test asserts it — and the rigid path has no bone
            // palette to give it.
            continue;
        };
        parts.push(world.spawn(RenderEntityDesc {
            transform: object.transform,
            mesh,
            material: part.material,
            bounds: part.bounds,
            visible: true,
            // Environment objects are never tinted, hit-flashed or faded. `InstanceStyle`
            // describes what is happening to one entity right now, and nothing happens to a
            // rock — so it stays neutral and costs the multiply the shader does anyway.
            style: InstanceStyle::NEUTRAL,
            // Scenery casts and receives, and hands the shadow passes the coarser levels the
            // model already carries. This is the whole of what §10 asks for: no shadow
            // meshes are authored, the asset's own levels of detail are reused, and a far
            // cascade draws a rock's third level because its texels could not resolve the
            // first.
            shadow: shadow_lods(object.asset, binding, lod, index),
        }));
        stats.spawned += 1;
    }
}

/// The shadow behaviour for one part of a placed object.
///
/// Looks the *same part index* up in the levels coarser than the one being drawn. Parts are
/// in a stable order across a model's levels — the importer preserves submesh order — so
/// part `index` of level 2 is the same piece of the object as part `index` of level 0. A
/// level that does not have that part contributes nothing, and the shadow pass falls back to
/// the mesh it is already drawing.
fn shadow_lods(
    asset: EnvironmentAsset,
    binding: &ModelBinding,
    lod: usize,
    index: usize,
) -> ShadowCasting {
    let base = base_casting(asset);
    if !base.casts {
        // Nothing to hand a coarser mesh to. Building the array anyway would be four handle
        // lookups per part of every floor in the scene, to fill a field the shadow pass will
        // never read.
        return base;
    }
    let mut meshes = [MeshHandle::INVALID; SHADOW_LOD_MESHES];
    for (offset, slot) in meshes.iter_mut().enumerate() {
        let Some(level) = binding.lods.get(lod + offset + 1) else {
            break;
        };
        if let Some(part) = level.parts.get(index)
            && let PartGeometry::Rigid(mesh) = part.geometry
        {
            *slot = mesh;
        }
    }
    base.with_lod_meshes(meshes)
}

/// Whether an asset casts shadows, receives them, or neither.
///
/// Props, architecture, rocks and plants cast: that is the default and the interesting case.
/// Three kinds of asset are deliberately excluded, and the exclusions are worth more than
/// they look:
///
/// * **The floors** — the ground plain and the arena floor. A flat horizontal plane's shadow
///   is the plane itself, so drawing one into every cascade is one draw per cascade for a
///   shadow that changes no pixel. The ground plain is a 640 m quad and would be drawn into
///   all of them.
/// * **The backdrop** — the sky dome and the ridges. The dome surrounds the camera, so its
///   shadow is the whole world; the ridges stand hundreds of metres out, past every cascade,
///   where they can only ever contribute to the far one's extruded region.
/// * Everything else casts and receives.
///
/// This is the same decision `extract` makes about a gameplay floor, in the one other place
/// that spawns render entities from level data. The two agree on purpose.
fn base_casting(asset: EnvironmentAsset) -> ShadowCasting {
    use engine_environment::asset as ids;
    match asset {
        a if a == ids::GROUND_PLAIN || a == ids::ARENA_FLOOR => ShadowCasting::RECEIVES_ONLY,
        // Not even a receiver: the dome is unlit backdrop drawn behind everything, and a
        // shadow falling across the sky is the one artefact nobody could explain away.
        a if a == ids::SKY_DOME => ShadowCasting::NONE,
        a if a == ids::RIDGE => ShadowCasting::RECEIVES_ONLY,
        _ => ShadowCasting::CASTS_AND_RECEIVES,
    }
}

/// Where an object's geometry is centred, in world space.
///
/// From the model's own LOD0 bounds rather than from its origin. A pine is authored standing
/// on its origin and is six metres tall, so its origin is four metres below the middle of
/// what is drawn — and selecting a level from the distance to its feet swaps a tree to its
/// coarsest form while its canopy fills the screen.
fn object_center(object: &EnvironmentObject, binding: &ModelBinding) -> Vec3 {
    let local = binding
        .lod(0)
        .map_or(Sphere::new(Vec3::ZERO, 0.0), |level| level.bounds);
    object.transform.transform_point(local.center)
}

/// Which level of detail an object at `center` should be drawn at.
fn select_lod(binding: &ModelBinding, center: Vec3, camera: Vec3, lod_bias: f32) -> usize {
    binding.select_lod((center - camera).length(), lod_bias)
}

/// The renderer's lighting, from a scene's.
///
/// A five-field copy, and the only place in the project that performs it. `engine-environment`
/// cannot name a `Lighting` — it does not depend on `engine-render-data`, deliberately — and
/// `engine-render-data` cannot name an `EnvironmentLighting`. This crate can name both, which
/// is the whole reason it exists.
#[must_use]
pub fn scene_lighting(lighting: &engine_environment::EnvironmentLighting) -> Lighting {
    Lighting {
        direction: lighting.sun_direction,
        color: lighting.sun_color,
        intensity: lighting.sun_intensity,
        ambient_color: lighting.ambient_color,
        ambient_intensity: lighting.ambient_intensity,
    }
}

/// Turns a scene's placed light into the one the renderer draws with.
///
/// The same five-field copy [`scene_lighting`] makes, for the same reason: `engine-environment`
/// describes a place and `engine-render-data` describes a frame, and this crate is the only
/// one allowed to know both. The angles become cosines here because that is what the shader
/// compares against a dot product.
#[must_use]
pub fn scene_light(light: &engine_environment::SceneLight) -> LocalLight {
    let mut converted = match light.cone_radians {
        Some((inner, outer)) => LocalLight::spot(
            light.position,
            light.direction,
            light.color,
            light.intensity,
            light.range,
            inner,
            outer,
        ),
        None => LocalLight::point(light.position, light.color, light.intensity, light.range),
    };
    converted.casts_shadow = light.casts_shadow;
    converted.priority = light.priority;
    converted
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_assets::procedural::environment_library;
    use engine_environment::asset as ids;
    use engine_environment::{EnvironmentScene, arena};
    use engine_render_data::{MaterialHandle, MeshHandle, ModelLodBinding, ModelPart};

    #[test]
    fn the_floors_and_the_backdrop_do_not_cast_and_everything_else_does() {
        // A flat floor's shadow is the floor, and the ground plain is a 640 m quad that would
        // otherwise be drawn into every cascade to change no pixel. The dome is the one asset
        // that must not even *receive*: it is backdrop, and a shadow across the sky is the
        // artefact nobody could explain.
        for asset in [ids::GROUND_PLAIN, ids::ARENA_FLOOR, ids::RIDGE] {
            let casting = base_casting(asset);
            assert!(!casting.casts, "{asset} is drawn into every cascade");
            assert!(casting.receives, "{asset} is what shadows land on");
        }
        let sky = base_casting(ids::SKY_DOME);
        assert!(
            !sky.casts && !sky.receives,
            "the sky takes no part in shadows"
        );

        for asset in [
            ids::PILLAR,
            ids::ARCH,
            ids::ROCK_MEDIUM,
            ids::PINE,
            ids::BRAZIER,
        ] {
            let casting = base_casting(asset);
            assert!(casting.casts && casting.receives, "{asset} should cast");
        }
    }

    /// Uploads the whole library over handles nothing created.
    ///
    /// The placer never dereferences a handle — that is the renderer's job — so a whole
    /// scene can be placed, levelled and culled with no device present. Every asset gets a
    /// distinct handle range, so a test can tell which asset a render entity came from and,
    /// crucially, so two placements of *one* asset share handles and two placements of two
    /// assets do not. That sharing is the property the instancing tests assert.
    fn visuals() -> EnvironmentVisuals {
        let mut visuals = EnvironmentVisuals::new();
        for (index, (asset, model)) in environment_library().into_iter().enumerate() {
            let base = index as u32 * 32;
            visuals.insert(
                asset,
                ModelBinding {
                    lods: model
                        .lods
                        .iter()
                        .enumerate()
                        .map(|(level, lod)| ModelLodBinding {
                            parts: lod
                                .submeshes
                                .iter()
                                .enumerate()
                                .map(|(part, submesh)| ModelPart {
                                    // One mesh per (level, submesh); one material per
                                    // material index. A coarser level is different geometry
                                    // and the *same* surface, which is what makes a level
                                    // change stay inside its material's batch.
                                    geometry: PartGeometry::Rigid(MeshHandle::new(
                                        base + (level * 4 + part) as u32,
                                        1,
                                    )),
                                    material: MaterialHandle::new(
                                        base + submesh.material as u32,
                                        1,
                                    ),
                                    bounds: submesh.bounds,
                                })
                                .collect(),
                            min_distance: lod.min_distance,
                            bounds: lod.bounds(),
                        })
                        .collect(),
                    skinned: false,
                    model_height: 1.0,
                },
            );
        }
        visuals
    }

    fn placed_arena() -> (RenderWorld, EnvironmentPlacer, EnvironmentScene) {
        let scene = arena::combat_arena();
        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());
        placer.place(&mut world, &scene, Vec3::new(0.0, 2.0, 0.0), 1.0);
        (world, placer, scene)
    }

    #[test]
    fn every_object_in_the_scene_becomes_something_drawable() {
        let (world, placer, scene) = placed_arena();
        assert_eq!(placer.stats().objects, scene.len());
        assert_eq!(placer.stats().unresolved, 0, "every asset resolved");
        assert_eq!(placer.len(), scene.len());
        assert_eq!(world.len(), placer.stats().entities);
        assert!(world.len() >= scene.len(), "at least one entity each");
    }

    #[test]
    fn the_upload_count_is_the_number_of_assets_and_not_the_number_of_objects() {
        // The instancing precondition, stated as the number it is: 16 uploads for ~190
        // objects. Everything downstream — shared handles, one batch per pair, one
        // instanced draw — follows from this and nothing else.
        let (_, placer, scene) = placed_arena();
        assert_eq!(placer.visuals().len(), ids::LIBRARY.len());
        assert!(
            scene.len() > placer.visuals().len() * 10,
            "{} objects over {} uploads",
            scene.len(),
            placer.visuals().len()
        );
    }

    #[test]
    fn a_thousand_rocks_share_one_mesh_and_one_material() {
        // The question the phase brief asks in as many words. The answer is that the scene
        // gains a thousand 44-byte records and the GPU gains nothing at all.
        let mut scene = EnvironmentScene::with_capacity("thousand", 1_000);
        for index in 0..1_000_u32 {
            let angle = index as f32 * 0.0628;
            scene.push(EnvironmentObject::decoration(
                ids::ROCK_SMALL,
                Vec3::new(angle.cos() * 60.0, 0.0, angle.sin() * 60.0),
            ));
        }

        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());
        // A bias of zero pins the finest level everywhere, so this measures sharing rather
        // than where the camera happens to be.
        placer.place(&mut world, &scene, Vec3::ZERO, 0.0);

        assert_eq!(world.len(), 1_000, "one part each");
        let meshes: std::collections::HashSet<_> = world.meshes().iter().copied().collect();
        let materials: std::collections::HashSet<_> = world.materials().iter().copied().collect();
        assert_eq!(meshes.len(), 1, "a thousand rocks want one mesh");
        assert_eq!(materials.len(), 1, "and one material");
        // And a thousand distinct transforms, or they would all be in the same place.
        let positions: std::collections::HashSet<_> = world
            .transforms()
            .iter()
            .map(|transform| {
                (
                    transform.translation.x.to_bits(),
                    transform.translation.z.to_bits(),
                )
            })
            .collect();
        assert_eq!(positions.len(), 1_000);
    }

    #[test]
    fn the_arena_collapses_into_a_small_number_of_mesh_and_material_pairs() {
        // What the renderer's grouping pass will find. Nineteen-odd pairs for nearly two
        // hundred objects is the whole draw-call story of the environment.
        let (world, _, _) = placed_arena();
        let pairs: std::collections::HashSet<_> = world
            .meshes()
            .iter()
            .zip(world.materials())
            .map(|(mesh, material)| (mesh.index(), material.index()))
            .collect();
        assert!(
            pairs.len() <= 30,
            "{} distinct (mesh, material) pairs over {} entities",
            pairs.len(),
            world.len()
        );
        assert!(pairs.len() * 5 < world.len(), "sharing is not happening");
    }

    #[test]
    fn a_distant_object_is_drawn_at_a_coarser_level_than_a_near_one() {
        let scene = arena::combat_arena();
        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());

        // A pine in the treeline, which is the asset with the most levels.
        let pine = scene
            .objects
            .iter()
            .position(|object| object.asset == ids::PINE)
            .expect("the arena has a treeline");
        let position = scene.objects[pine].position();

        placer.place(&mut world, &scene, position + Vec3::new(0.0, 2.0, 4.0), 1.0);
        let near = placer.lod_of(pine).expect("the pine was placed");
        assert_eq!(near, 0, "standing next to it");

        placer.update_lod(&mut world, position + Vec3::new(0.0, 2.0, 400.0), 1.0);
        let far = placer.lod_of(pine).expect("the pine is still placed");
        assert!(far > near, "near {near}, far {far}");
        assert!(placer.stats().lod_changes >= 1);
    }

    #[test]
    fn a_level_change_replaces_the_parts_and_a_still_camera_changes_nothing() {
        let scene = arena::combat_arena();
        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());
        let eye = Vec3::new(0.0, 2.0, 0.0);
        placer.place(&mut world, &scene, eye, 1.0);
        let entities = placer.stats().entities;

        // A camera that has not moved crosses no threshold, so the pass is a distance and a
        // comparison per object and nothing else.
        placer.update_lod(&mut world, eye, 1.0);
        assert_eq!(placer.stats().lod_changes, 0);
        assert_eq!(placer.stats().spawned, 0);
        assert_eq!(placer.stats().despawned, 0);
        assert_eq!(placer.stats().entities, entities);

        // Pinning the finest level everywhere does change things, and the parts are
        // replaced rather than accumulated.
        placer.update_lod(&mut world, eye, 0.0);
        assert!(placer.stats().lod_changes > 0);
        assert_eq!(world.len(), placer.stats().entities);
    }

    #[test]
    fn a_level_change_keeps_an_object_inside_the_same_material_batch() {
        // A coarser level is different geometry and the same surface. If a level change also
        // changed the material, crossing a threshold would move an object between batches
        // and the draw-call count would jump for a reason nobody could see.
        let scene = arena::combat_arena();
        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());
        placer.place(&mut world, &scene, Vec3::new(0.0, 2.0, 0.0), 1.0);

        let pine = scene
            .objects
            .iter()
            .position(|object| object.asset == ids::PINE)
            .expect("the arena has a treeline");
        let materials_of = |placer: &EnvironmentPlacer, world: &RenderWorld| {
            let mut list: Vec<_> = placer
                .parts_of(pine)
                .expect("the pine is placed")
                .iter()
                .map(|entity| world.materials()[entity.index()])
                .collect();
            list.sort_by_key(|handle: &MaterialHandle| handle.index());
            list
        };
        let before = materials_of(&placer, &world);

        placer.update_lod(&mut world, Vec3::new(0.0, 2.0, 500.0), 1.0);
        assert_eq!(materials_of(&placer, &world), before);
    }

    #[test]
    fn the_ground_and_the_sky_are_placed_where_the_scene_put_them() {
        let (world, placer, scene) = placed_arena();
        let find = |asset| {
            let index = scene
                .objects
                .iter()
                .position(|object| object.asset == asset)
                .expect("the scene has one");
            let parts = placer.parts_of(index).expect("it was placed");
            world.transforms()[parts[0].index()]
        };

        let sky = find(ids::SKY_DOME);
        assert_eq!(sky.translation, Vec3::ZERO);

        let plain = find(ids::GROUND_PLAIN);
        assert_eq!(plain.translation, Vec3::ZERO);
        assert!(plain.scale.x > 100.0, "the plain has to reach the horizon");

        let floor = find(ids::ARENA_FLOOR);
        assert!(
            floor.translation.y > 0.0 && floor.translation.y < 0.1,
            "the paving is a couple of centimetres above the plain, not coplanar with it"
        );
    }

    #[test]
    fn placing_a_second_scene_replaces_the_first_rather_than_adding_to_it() {
        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());
        placer.place(&mut world, &arena::combat_arena(), Vec3::ZERO, 1.0);
        let first = world.len();

        placer.place(&mut world, &arena::environment_demo(), Vec3::ZERO, 1.0);
        assert!(placer.stats().despawned >= first, "the first scene stayed");
        assert_eq!(world.len(), placer.stats().entities);
        assert_eq!(placer.len(), arena::environment_demo().len());

        // And clearing takes everything back out.
        placer.clear(&mut world);
        assert!(placer.is_empty());
        assert!(
            world.is_empty(),
            "{} entities were left behind",
            world.len()
        );
    }

    #[test]
    fn an_object_whose_asset_was_never_uploaded_is_counted_rather_than_substituted() {
        let mut scene = EnvironmentScene::with_capacity("partial", 2);
        scene.push(EnvironmentObject::decoration(ids::PILLAR, Vec3::ZERO));
        scene.push(EnvironmentObject::decoration(
            EnvironmentAsset::new("obelisk_of_typos"),
            Vec3::X,
        ));

        let mut world = RenderWorld::new();
        let mut placer = EnvironmentPlacer::new(visuals());
        placer.place(&mut world, &scene, Vec3::ZERO, 1.0);

        assert_eq!(placer.stats().unresolved, 1);
        assert_eq!(placer.len(), 1, "only the pillar was placed");
        assert_eq!(placer.lod_of(1), None);
    }

    #[test]
    fn a_levels_distance_is_measured_to_the_middle_of_the_model_not_to_its_feet() {
        // A pine is authored standing on its origin and is six metres tall. Measuring to the
        // origin swaps it to its coarsest level while its canopy still fills the screen.
        let mut scene = EnvironmentScene::with_capacity("one_pine", 1);
        scene.push(EnvironmentObject::decoration(ids::PINE, Vec3::ZERO));
        let visuals = visuals();
        let binding = visuals.get(ids::PINE).expect("the pine is uploaded");
        let center = object_center(&scene.objects[0], binding);
        assert!(center.y > 1.5, "the centre is at the feet: {center:?}");
    }

    #[test]
    fn the_scenes_light_becomes_the_renderers_light_field_for_field() {
        let scene = arena::combat_arena();
        let lighting = scene_lighting(&scene.lighting);
        assert_eq!(lighting.direction, scene.lighting.sun_direction);
        assert_eq!(lighting.color, scene.lighting.sun_color);
        assert_eq!(lighting.intensity, scene.lighting.sun_intensity);
        assert_eq!(lighting.ambient_color, scene.lighting.ambient_color);
        assert_eq!(lighting.ambient_intensity, scene.lighting.ambient_intensity);
    }

    #[test]
    fn registering_an_asset_twice_replaces_rather_than_growing() {
        let mut visuals = visuals();
        let before = visuals.len();
        let Some(binding) = visuals.get(ids::PILLAR).cloned() else {
            panic!("the pillar is uploaded");
        };
        visuals.insert(ids::PILLAR, binding);
        assert_eq!(visuals.len(), before);
        assert!(!visuals.is_empty());
    }

    #[test]
    fn placing_the_same_scene_twice_produces_the_same_world() {
        // Determinism through the placement path: two loads of one scene draw the same
        // picture, which is what makes a screenshot and a benchmark comparable.
        let describe = || {
            let mut world = RenderWorld::new();
            let mut placer = EnvironmentPlacer::new(visuals());
            placer.place(
                &mut world,
                &arena::combat_arena(),
                Vec3::new(0.0, 2.0, 8.0),
                1.0,
            );
            let entries: Vec<_> = (0..world.slot_count())
                .filter(|slot| world.is_drawable(*slot))
                .map(|slot| {
                    (
                        world.meshes()[slot].index(),
                        world.materials()[slot].index(),
                        world.transforms()[slot].translation.x.to_bits(),
                        world.transforms()[slot].translation.z.to_bits(),
                    )
                })
                .collect();
            entries
        };
        assert_eq!(describe(), describe());
    }
}
