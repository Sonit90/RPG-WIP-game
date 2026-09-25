//! Gameplay state to `RenderWorld`.
//!
//! ```text
//! GameWorld                         RenderWorld
//! ---------                         -----------
//! position, facing, height   --->   Transform
//! kind, alive                --->   MeshHandle, MaterialHandle
//! radius, height             --->   Sphere bounds
//! health, statuses, cooldowns  X    (never crosses)
//! ```
//!
//! Extraction runs once per rendered frame, after the simulation has advanced. It is a
//! copy, not a synchronization: the `RenderWorld` is rebuilt from gameplay state every
//! frame and holds nothing gameplay reads back.
//!
//! ## Why entities are not respawned every frame
//!
//! Clearing and refilling the `RenderWorld` would be simpler and would also throw away the
//! culler's stable slot ordering and make every render entity's handle change every frame.
//! Instead each gameplay slot keeps its render entity for as long as the gameplay entity
//! lives, and a slot whose generation changed is despawned and respawned — which is exactly
//! the case a stale handle exists to catch.

use engine_core::math::{Quat, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::movement::MovementState;
use engine_game::sim::Simulation;
use engine_game::world::EntityKind;
use engine_render_data::{
    InstanceStyle, MaterialHandle, MeshHandle, ModelBinding, ModelLodBinding, ModelPart,
    PartGeometry, RenderEntity, RenderEntityDesc, RenderWorld, ShadowCasting,
};

use crate::character::{CastVisual, CharacterState, CharacterVisual, Heading};
use crate::monster_visual::{MonsterFrame, MonsterModel, MonsterVisuals};

/// Which mesh and material each kind of thing is drawn with.
///
/// Supplied by the application, because uploading a mesh needs a device and this crate does
/// not have one. Everything here is a handle the renderer issued plus the local bounds it
/// reported, so extraction can place the bounds in world space without asking the renderer
/// anything.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualBindings {
    /// The single-target training dummy.
    ///
    /// A whole model — levels of detail, submeshes, materials — rather than one mesh and
    /// one material. That is the change this phase makes to the seam: extraction places a
    /// *model*, and how many draws that model costs is the model's business.
    pub dummy: ModelBinding,
    /// The dummies in the group. May be the same model; it is not, so the two roles are
    /// distinguishable at a glance.
    pub dummy_group: ModelBinding,
    /// Mesh for the arena floor.
    pub ground_mesh: MeshHandle,
    /// Local bounds of [`VisualBindings::ground_mesh`].
    pub ground_bounds: Sphere,
    /// Material for the arena floor.
    pub ground_material: MaterialHandle,
    /// Mesh used for the target marker under the selected dummy.
    pub marker_mesh: MeshHandle,
    /// Local bounds of [`VisualBindings::marker_mesh`].
    pub marker_bounds: Sphere,
    /// Material for the target marker.
    pub marker_material: MaterialHandle,
}

impl VisualBindings {
    /// Bindings over handles nothing uploaded.
    ///
    /// For a benchmark or a test that measures *extraction* — how many render entities a
    /// scene produces, which level of detail it picks, how much it churns — rather than
    /// rendering. The extractor never dereferences a handle, which is exactly what makes
    /// this legitimate rather than a fake.
    ///
    /// A caller with a device uses `Renderer::create_model` instead and gets the same type.
    #[must_use]
    pub fn unbound(dummy: &engine_assets::ModelAsset, group: &engine_assets::ModelAsset) -> Self {
        let unit = Sphere::new(Vec3::ZERO, 0.87);
        Self {
            dummy: unbound_binding(dummy, 0),
            dummy_group: unbound_binding(group, 100),
            ground_mesh: MeshHandle::new(900, 1),
            ground_bounds: Sphere::new(Vec3::ZERO, 1.0),
            ground_material: MaterialHandle::new(900, 1),
            marker_mesh: MeshHandle::new(901, 1),
            marker_bounds: unit,
            marker_material: MaterialHandle::new(901, 1),
        }
    }
}

/// One model's worth of [`VisualBindings::unbound`].
///
/// `base` keeps two models' handles distinguishable, so a test can tell which one a render
/// entity came from.
fn unbound_binding(model: &engine_assets::ModelAsset, base: u32) -> ModelBinding {
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
                    .map(|(index, submesh)| ModelPart {
                        geometry: PartGeometry::Rigid(MeshHandle::new(
                            base + (level * 8 + index) as u32,
                            1,
                        )),
                        material: MaterialHandle::new(base + submesh.material as u32, 1),
                        bounds: submesh.bounds,
                    })
                    .collect(),
                min_distance: lod.min_distance,
                bounds: lod.bounds(),
            })
            .collect(),
        skinned: false,
        model_height: model
            .lod0()
            .map_or(1.0, |lod| lod.bounds().radius.max(0.5) * 2.0),
    }
}

/// What the last extraction did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VisualStats {
    /// Gameplay entities extracted.
    pub extracted: usize,
    /// Render entities spawned this frame.
    pub spawned: usize,
    /// Render entities despawned this frame.
    pub despawned: usize,
    /// Entities whose material changed because they died or were revived.
    pub restyled: usize,
}

/// One gameplay slot's rendering counterpart.
///
/// A *list* of render entities now, one per submesh of the model's selected level of
/// detail. A dummy is a post, a straw body and a wrap; each is a different material, and one
/// draw can bind one material.
#[derive(Debug, Clone, PartialEq)]
struct Mapped {
    /// One render entity per part of the current level of detail.
    parts: Vec<RenderEntity>,
    /// The gameplay generation this mapping was made for. A different one means the slot
    /// has been reused by another entity.
    generation: u32,
    /// Whether the mapping was made for a living entity.
    alive: bool,
    /// Which level of detail the parts were spawned for.
    ///
    /// A level change means different meshes, so the parts are despawned and respawned. That
    /// is the one thing that still churns slots, and it happens when an entity crosses a
    /// distance threshold rather than every frame.
    lod: usize,
}

/// Where the camera is and how aggressively to drop detail.
///
/// Set once per frame by the application. Carried on the extractor rather than passed to
/// `extract` so the signature every existing caller uses stays what it was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewParams {
    /// Where the camera is, for level-of-detail selection.
    pub camera_position: Vec3,
    /// Real seconds since the last frame, for animation.
    ///
    /// Real, not simulated: an animation is a visual and should run at the frame rate, while
    /// the simulation runs at its own fixed tick. Feeding it the tick would make a character
    /// animate in 60 Hz steps while moving smoothly.
    pub delta_seconds: f32,
    /// Scales distance before a level of detail is chosen. 1 is the authored thresholds; 0
    /// pins the finest level.
    pub lod_bias: f32,
}

impl Default for ViewParams {
    fn default() -> Self {
        Self {
            camera_position: Vec3::ZERO,
            delta_seconds: 0.0,
            lod_bias: 1.0,
        }
    }
}

/// How dead a dead dummy looks: a dark, desaturated multiply on whatever it was.
///
/// A tint rather than the second material this used to be. `InstanceStyle` already carries a
/// per-entity multiply, so "it died" costs a store instead of a material handle that has to
/// exist for every model in the game.
const DEAD_TINT: [f32; 4] = [0.34, 0.31, 0.28, 1.0];

/// What a monster's corpse is drawn at: dimmed, not greyed, because its own model still says
/// what it was.
const CORPSE_TINT: [f32; 4] = [0.62, 0.60, 0.58, 1.0];

/// How far a monster model is washed towards its table colour. The stand-in took the whole
/// colour from the tint; a model with its own textures takes a hint of it.
const MONSTER_TINT_WASH: f32 = 0.2;

/// How dark a nearly dead monster model gets. Lighter than the stand-in's floor: a model's
/// textures are already darker than a flat dummy.
const MONSTER_HEALTH_FLOOR: f32 = 0.7;

/// Builds and maintains a `RenderWorld` from a [`Simulation`].
pub struct RenderExtractor {
    world: RenderWorld,
    bindings: VisualBindings,
    /// Gameplay slot to render entity. Grows with the world and is never freed.
    mapped: Vec<Option<Mapped>>,
    ground: RenderEntity,
    /// How wide the drawn floor is, so it can be moved without being re-measured.
    ground_size: f32,
    marker: RenderEntity,
    stats: VisualStats,
    /// Half the height of the target marker, so it sits on the floor rather than in it.
    marker_height: f32,
    /// When each gameplay slot was last damaged. Grows with the world; never freed.
    hits: Vec<Option<Hit>>,
    /// The player's visual controller, if the scene set one up.
    ///
    /// `Option` because a scene may not want a character — a culling benchmark builds an
    /// extractor to measure visibility and has no interest in a skeleton. A scene with no
    /// visual simply does not draw its player, which is loud rather than subtle.
    player_visual: Option<CharacterVisual>,
    /// Which dummy is the single-target one.
    ///
    /// Gameplay does not distinguish them — `EntityKind` is `Player` or `Dummy`, and the doc
    /// on it says branching on an enum is how a list grows a new case for every entity kind.
    /// So the *scene* says which one is featured, because the scene is what placed it.
    featured_dummy: Option<EntityId>,
    /// This frame's camera and detail settings.
    view: ViewParams,
    /// Monsters drawn as animated characters, where their table row names a model there is
    /// one of. The rest are drawn with the rigid stand-in.
    monsters: MonsterVisuals,
}

/// Height above the floor the target marker floats at, in metres.
const MARKER_HEIGHT: f32 = 3.0;

/// Half-extent of the marker, in metres.
const MARKER_SIZE: f32 = 0.4;

/// Darkest a full-health-to-dead tint gets.
///
/// Not zero: a tint that reached black would make a nearly-dead dummy invisible against the
/// arena floor, which reads as a rendering bug rather than as low health.
const HEALTH_TINT_FLOOR: f32 = 0.45;

/// Emissive added to the current target, so selection is visible on the enemy itself.
const TARGET_GLOW: f32 = 0.22;

/// Emissive a monster with a full glow in its look carries: an empowered add, lit from inside.
const EMPOWERED_GLOW: f32 = 0.35;

/// How long a hit flash lasts, in simulated seconds.
///
/// Short enough to read as an impact rather than a state, long enough to survive a frame at
/// any frame rate the engine reaches.
const HIT_FLASH_SECONDS: f32 = 0.18;

/// Emissive added at the peak of a hit flash.
const HIT_FLASH_STRENGTH: f32 = 0.9;

/// How much a hit flash lifts the tint towards white at its peak.
const HIT_FLASH_LIFT: f32 = 0.6;

/// How much longer and brighter a critical strike's flash is.
///
/// A crit already has its own colour and size on the damage number. This is the same fact
/// said on the *target*, because that is where the player is looking when a big hit lands,
/// and reading it there costs no draw call — it is the emissive term the flash already uses.
const CRITICAL_FLASH_SCALE: f32 = 1.7;

/// Blends two positions by `alpha`.
///
/// A plain lerp. Positions are metres and a tick is 16.67 ms, so nothing an entity can do in
/// one step makes the straight line between two samples visibly wrong.
#[must_use]
pub fn lerp_position(previous: Vec3, current: Vec3, alpha: f32) -> Vec3 {
    let alpha = alpha.clamp(0.0, 1.0);
    previous + (current - previous) * alpha
}

/// How much of its speed a body must spend moving against its facing to count as
/// backpedalling. Below this — a strafe, or a strafe drifting slightly back — the forward
/// or sideways cycle still reads better than a backwards one.
const BACKPEDAL_SHARE: f32 = 0.3;

/// How much of its speed a body must spend moving sideways to count as strafing. A diagonal
/// forwards (about 0.71 each way) stays a forward run, which is what it looks like.
const STRAFE_SHARE: f32 = 0.8;

/// Which way `velocity` goes relative to a body facing `yaw`.
///
/// Yaw zero faces -Z with +X on the right. Backwards wins over sideways, so a diagonal
/// backwards backpedals rather than sidestepping.
fn heading(velocity: Vec3, yaw: f32) -> Heading {
    let speed = velocity.x.hypot(velocity.z);
    if speed <= 1e-4 {
        return Heading::Forward;
    }
    let (sin, cos) = yaw.sin_cos();
    let forward = -velocity.x * sin - velocity.z * cos;
    let right = velocity.x * cos - velocity.z * sin;
    if forward < -BACKPEDAL_SHARE * speed {
        Heading::Backward
    } else if right >= STRAFE_SHARE * speed {
        Heading::Right
    } else if right <= -STRAFE_SHARE * speed {
        Heading::Left
    } else {
        Heading::Forward
    }
}

/// The direction `velocity` goes relative to a body facing `yaw`, in radians: zero ahead,
/// positive to its right, `±pi` behind. Zero for a body standing still.
///
/// Same axes as [`heading`]; the angle rather than the bucket, for the stride turn a diagonal
/// run gets (see `CharacterVisual::stride_yaw`).
fn move_angle(velocity: Vec3, yaw: f32) -> f32 {
    let speed = velocity.x.hypot(velocity.z);
    if speed <= 1e-4 {
        return 0.0;
    }
    let (sin, cos) = yaw.sin_cos();
    let forward = -velocity.x * sin - velocity.z * cos;
    let right = velocity.x * cos - velocity.z * sin;
    right.atan2(forward)
}

/// Blends two yaw angles by `alpha`, taking the short way round.
///
/// A plain lerp on angles spins a character the long way round whenever it crosses the
/// wrap point — a turn from +179 to −179 degrees is two degrees of movement and 358 degrees
/// of lerp. Wrapping the difference into (−pi, pi] first is what makes a character that
/// turns past south not whip around through north.
#[must_use]
pub fn lerp_angle(previous: f32, current: f32, alpha: f32) -> f32 {
    let alpha = alpha.clamp(0.0, 1.0);
    let mut difference = (current - previous) % std::f32::consts::TAU;
    if difference > std::f32::consts::PI {
        difference -= std::f32::consts::TAU;
    } else if difference < -std::f32::consts::PI {
        difference += std::f32::consts::TAU;
    }
    previous + difference * alpha
}

/// Where the player is drawn this frame, interpolated between the last two ticks.
///
/// The camera follows this rather than `Simulation::player_position`, so the character and
/// the camera agree about where the character is. Following the raw simulation position
/// would make the camera step at 60 Hz while the character it is watching moved smoothly.
#[must_use]
pub fn interpolated_player_position(simulation: &Simulation) -> Vec3 {
    let world = simulation.world();
    let player = simulation.player().entity;
    let Some(body) = world.body(player) else {
        return Vec3::ZERO;
    };
    let previous = world.previous_body(player).unwrap_or(body);
    lerp_position(
        previous.position,
        body.position,
        simulation.timestep().alpha(),
    )
}

/// Whether the player was teleported during the frame just simulated.
///
/// The camera smooths its way to the character, which is what makes running read as
/// momentum and is exactly wrong for Blink: at the follow rate the camera spends a quarter
/// of a second travelling ground the mage never crossed. A frame this returns `true` for is
/// one the camera snaps on instead.
///
/// Answered from the event stream rather than by measuring how far the player moved: a
/// large step between two ticks is also what a fast run, a knockback or a dropped frame
/// look like, and only the simulation knows which of them happened.
#[must_use]
pub fn player_teleported(simulation: &Simulation) -> bool {
    let player = simulation.player().entity;
    simulation
        .events()
        .iter()
        .any(|event| matches!(event, CombatEvent::Teleported { entity, .. } if *entity == player))
}

/// The tick of the latest event this frame that `pick` recognises.
///
/// Ticks rather than booleans: the stream is per advance, and a frame that draws without
/// advancing sees the same events again. A tick lets the reader act on each one once.
fn last_tick(simulation: &Simulation, pick: impl Fn(&CombatEvent) -> Option<u64>) -> Option<u64> {
    simulation.events().iter().rev().find_map(pick)
}

/// Which model an entity is drawn with.
///
/// The entire replacement for `if entity == fire_mage { load fire_mage.glb }`: one lookup
/// that returns a *binding*, with no mesh, material or file name anywhere in it. Adding a
/// third kind of dummy is a third binding and a line here; adding a second playable
/// character does not touch this function at all, because characters go through
/// [`CharacterVisual`].
///
/// A free function rather than a method so it can be called while another field of the
/// extractor is mutably borrowed, which is what the spawn loop does.
fn binding_for(
    bindings: &VisualBindings,
    featured: Option<EntityId>,
    id: EntityId,
) -> &ModelBinding {
    if featured == Some(id) {
        &bindings.dummy
    } else {
        &bindings.dummy_group
    }
}

/// When an entity was last damaged, for the hit flash.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Hit {
    /// The generation that was hit, so a recycled slot does not inherit the flash.
    generation: u32,
    /// Simulation time of the hit.
    at: f32,
    /// Whether it critically struck, which flashes harder and for longer.
    critical: bool,
}

impl RenderExtractor {
    /// Creates the extractor and spawns the parts of the scene that are not entities.
    ///
    /// The floor and the target marker exist for the lifetime of the world: the floor never
    /// changes, and the marker is hidden rather than despawned when nothing is targeted,
    /// because a despawn per target change would churn slots for no reason.
    #[must_use]
    pub fn new(bindings: VisualBindings, ground_size: f32) -> Self {
        let mut world = RenderWorld::with_capacity(64);
        let ground = world.spawn(RenderEntityDesc {
            transform: Transform::from_scale(Vec3::new(ground_size, 1.0, ground_size)),
            mesh: bindings.ground_mesh,
            material: bindings.ground_material,
            bounds: bindings.ground_bounds,
            visible: true,
            style: InstanceStyle::NEUTRAL,
            // The floor receives every shadow in the scene and casts none: it is a flat
            // plane, so its own shadow is either nothing or — seen from a low sun — a
            // horizon-wide slab of darkness over everything behind it.
            shadow: ShadowCasting::RECEIVES_ONLY,
        });
        let marker = world.spawn(RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh: bindings.marker_mesh,
            material: bindings.marker_material,
            bounds: bindings.marker_bounds,
            visible: false,
            style: InstanceStyle::NEUTRAL,
            // A ring drawn on the floor under the selected target. It is an interface
            // element that happens to be geometry: it must not cast a shadow, and it must
            // not be darkened by one either, or the selection it marks becomes unreadable
            // exactly when the target steps into shade.
            shadow: ShadowCasting::NONE,
        });

        Self {
            world,
            bindings,
            mapped: Vec::with_capacity(64),
            ground,
            ground_size,
            marker,
            stats: VisualStats::default(),
            marker_height: MARKER_HEIGHT,
            hits: Vec::with_capacity(64),
            player_visual: None,
            featured_dummy: None,
            view: ViewParams::default(),
            monsters: MonsterVisuals::new(),
        }
    }

    /// Gives the monsters their models: every enemy whose table row names one of these is
    /// drawn with it, animated, from the next frame.
    pub fn set_monster_models(&mut self, models: Vec<MonsterModel>) {
        self.monsters.set_models(models);
    }

    /// The monsters' animated visuals, for the developer overlay and the test scene.
    #[must_use]
    pub fn monsters(&self) -> &MonsterVisuals {
        &self.monsters
    }

    /// Gives the player a character visual.
    ///
    /// Until this is called the player is not drawn. That is deliberate: a character is a
    /// model, a skeleton and a set of clips, and inventing a fallback for a scene that
    /// forgot to supply one would hide the omission behind a cube.
    pub fn set_player_visual(&mut self, visual: CharacterVisual) {
        self.player_visual = Some(visual);
    }

    /// The player's visual controller, for the developer overlay.
    #[must_use]
    pub fn player_visual(&self) -> Option<&CharacterVisual> {
        self.player_visual.as_ref()
    }

    /// The player's visual controller, for the character test scene's animation override.
    pub fn player_visual_mut(&mut self) -> Option<&mut CharacterVisual> {
        self.player_visual.as_mut()
    }

    /// Names the single-target dummy, so it is drawn with its own model.
    pub fn set_featured_dummy(&mut self, id: EntityId) {
        self.featured_dummy = Some(id);
    }

    /// Sets this frame's camera position, frame delta and level-of-detail bias.
    ///
    /// Called before [`RenderExtractor::extract`]. A scene that never calls it gets the
    /// finest level of detail everywhere and a character that does not animate, which is
    /// the correct behaviour for a benchmark that only cares about visibility.
    pub fn set_view(&mut self, view: ViewParams) {
        self.view = view;
    }

    /// The world the renderer draws.
    #[must_use]
    pub fn render_world(&self) -> &RenderWorld {
        &self.world
    }

    /// Moves the drawn floor to height `y`.
    ///
    /// The floor is a visual, not a collision surface — the simulation's floor is whatever
    /// is in its [`engine_collision::world::CollisionWorld`]. A scene whose own geometry
    /// provides the ground has to be able to move this one out of the way, because two
    /// coplanar surfaces at the same height are a depth-buffer tie and a tie flickers.
    pub fn set_ground_height(&mut self, y: f32) {
        let transform = Transform::from_translation(Vec3::new(0.0, y, 0.0)).with_scale(Vec3::new(
            self.ground_size,
            1.0,
            self.ground_size,
        ));
        self.world
            .set_transform(self.ground, transform, self.bindings.ground_bounds);
    }

    /// The world the renderer draws, for something that adds to it.
    ///
    /// The developer collision view spawns its own entities here rather than being given a
    /// second `RenderWorld` to draw: two worlds would be two culls, two instance buffers
    /// and two draw passes for something that is a handful of boxes.
    pub fn render_world_mut(&mut self) -> &mut RenderWorld {
        &mut self.world
    }

    /// The floor's render entity.
    #[must_use]
    pub fn ground(&self) -> RenderEntity {
        self.ground
    }

    /// What the last extraction did.
    #[must_use]
    pub fn stats(&self) -> VisualStats {
        self.stats
    }

    /// The render entity standing in for a gameplay entity, if it has one.
    #[must_use]
    pub fn render_entity(&self, id: EntityId) -> Option<RenderEntity> {
        self.render_parts(id)
            .and_then(|parts| parts.first().copied())
    }

    /// Every render entity standing in for a gameplay entity.
    ///
    /// One per submesh of the model's current level of detail.
    #[must_use]
    pub fn render_parts(&self, id: EntityId) -> Option<&[RenderEntity]> {
        self.mapped
            .get(id.index())?
            .as_ref()
            .filter(|mapped| mapped.generation == id.generation())
            .map(|mapped| mapped.parts.as_slice())
    }

    /// Rebuilds the `RenderWorld` from the simulation's current state.
    ///
    /// Allocation: after the first frame in which the world reached its size, this only
    /// writes into existing storage — the mapping vector and the `RenderWorld`'s arrays are
    /// both grown, never cleared.
    pub fn extract(&mut self, simulation: &Simulation) -> &RenderWorld {
        let game = simulation.world();
        let mut stats = VisualStats::default();

        if self.mapped.len() < game.slot_count() {
            self.mapped.resize(game.slot_count(), None);
        }

        // The skinned list is transient: it holds this frame's characters and is rebuilt
        // from scratch, so it is cleared before anything submits into it.
        self.world.skinned_mut().clear();
        self.monsters.observe(simulation);

        for slot in 0..self.mapped.len() {
            let id = game.id_at(slot);
            match id {
                Some(id) => {
                    // The player is not a `RenderWorld` slot. It is a skinned character, and
                    // its controller submits it below.
                    if game.kind(id) == Some(EntityKind::Player) {
                        self.despawn_parts(slot, &mut stats);
                        stats.extracted += 1;
                        continue;
                    }

                    let alive = game.is_alive(id);
                    // A monster with a model is a skinned character, like the player, and
                    // holds no `RenderWorld` slot.
                    if let Some(model) = self.monster_model(simulation, id) {
                        self.despawn_parts(slot, &mut stats);
                        self.drive_monster(simulation, id, model, alive);
                        stats.extracted += 1;
                        continue;
                    }
                    self.monsters.forget(slot);
                    let transform = self.transform_for(simulation, id, alive);
                    let style = self.style_for(simulation, id, alive);
                    let lod = self.select_lod(simulation, id);

                    // A *different entity* in this slot, or a different level of detail,
                    // means different render entities. Dying does not: `InstanceStyle`
                    // carries "it died" as a tint, so it costs a store rather than a churn.
                    let stale = self.mapped[slot].as_ref().is_some_and(|mapped| {
                        mapped.generation != id.generation() || mapped.lod != lod
                    });
                    if stale {
                        self.despawn_parts(slot, &mut stats);
                    }

                    match self.mapped[slot].as_mut() {
                        Some(mapped) => {
                            let binding = binding_for(&self.bindings, self.featured_dummy, id);
                            let Some(level) = binding.lod(mapped.lod) else {
                                continue;
                            };
                            for (entity, part) in mapped.parts.iter().zip(&level.parts) {
                                self.world.set_transform(*entity, transform, part.bounds);
                                self.world.set_style(*entity, style);
                            }
                            if mapped.alive != alive {
                                mapped.alive = alive;
                                stats.restyled += 1;
                            }
                        }
                        None => {
                            let binding = binding_for(&self.bindings, self.featured_dummy, id);
                            let Some(level) = binding.lod(lod) else {
                                continue;
                            };
                            let mut parts = Vec::with_capacity(level.parts.len());
                            for part in &level.parts {
                                let PartGeometry::Rigid(mesh) = part.geometry else {
                                    // A skinned part on a non-character. Nothing authors one,
                                    // and the rigid path has no palette to give it.
                                    continue;
                                };
                                parts.push(self.world.spawn(RenderEntityDesc {
                                    transform,
                                    mesh,
                                    material: part.material,
                                    bounds: part.bounds,
                                    visible: true,
                                    style,
                                    // A dummy, an enemy or anything else the simulation
                                    // owns. Its shadow on the ground is what tells the
                                    // player where it is standing.
                                    shadow: ShadowCasting::CASTS_AND_RECEIVES,
                                }));
                                stats.spawned += 1;
                            }
                            self.mapped[slot] = Some(Mapped {
                                parts,
                                generation: id.generation(),
                                alive,
                                lod,
                            });
                        }
                    }
                    stats.extracted += 1;
                }
                None => {
                    self.despawn_parts(slot, &mut stats);
                    self.monsters.forget(slot);
                }
            }
        }

        self.update_player_visual(simulation);
        self.update_marker(simulation);
        self.stats = stats;
        &self.world
    }

    /// The monster model enemy `id` is drawn with, if its table row names one there is.
    fn monster_model(&self, simulation: &Simulation, id: EntityId) -> Option<usize> {
        if simulation.world().kind(id) != Some(EntityKind::Enemy) {
            return None;
        }
        let look = simulation.brain(id)?.archetype().look;
        self.monsters.model_index(look.model)
    }

    /// Animates and draws one monster from its body, its brain and this frame's events.
    fn drive_monster(&mut self, simulation: &Simulation, id: EntityId, model: usize, alive: bool) {
        let game = simulation.world();
        let Some(body) = game.body(id) else {
            return;
        };
        let alpha = simulation.timestep().alpha();
        let previous = game.previous_body(id).unwrap_or(body);
        let cast = simulation
            .brain(id)
            .and_then(|brain| brain.cast())
            .filter(|_| alive);
        let frame = MonsterFrame {
            position: lerp_position(previous.position, body.position, alpha),
            facing_yaw: lerp_angle(previous.facing_yaw, body.facing_yaw, alpha),
            speed: if alive { body.horizontal_speed() } else { 0.0 },
            heading: heading(body.velocity, body.facing_yaw),
            grounded: body.grounded || !alive,
            rising: body.state == MovementState::Jumping,
            height: game.height(id),
            cast: cast.map(|cast| (cast.duration, cast.fraction())),
            dead: !alive,
        };
        let style = self.monster_style(simulation, id, alive);
        self.monsters.drive(
            id,
            model,
            frame,
            self.view.delta_seconds,
            style,
            &mut self.world,
            self.view.camera_position,
            self.view.lod_bias,
        );
    }

    /// [`RenderExtractor::style_for`] for a monster drawn with its own model.
    ///
    /// The model carries its own colours, so the table's tint is only a wash over them — a
    /// fifth of the way — where on the shared stand-in it *was* the colour. A corpse dims
    /// rather than greys: it is still that monster, lying there.
    fn monster_style(&self, simulation: &Simulation, id: EntityId, alive: bool) -> InstanceStyle {
        if !alive {
            return InstanceStyle {
                tint: CORPSE_TINT,
                emissive: 0.0,
            };
        }
        let look = simulation.brain(id).map(|brain| brain.archetype().look);
        let fraction = simulation
            .world()
            .health(id)
            .map_or(1.0, |health| health.fraction().clamp(0.0, 1.0));
        let shade = MONSTER_HEALTH_FLOOR + (1.0 - MONSTER_HEALTH_FLOOR) * fraction;
        let tint = look.map_or([1.0; 3], |look| look.tint);
        let wash = |channel: f32| shade * (1.0 + (channel - 1.0) * MONSTER_TINT_WASH);
        let mut style = InstanceStyle {
            tint: [wash(tint[0]), wash(tint[1]), wash(tint[2]), 1.0],
            emissive: look.map_or(0.0, |look| look.glow * EMPOWERED_GLOW),
        };
        if simulation.player().target == Some(id) {
            style.emissive += TARGET_GLOW;
        }
        self.apply_hit_flash(simulation, id, &mut style);
        style
    }

    /// Removes every render entity standing in for gameplay slot `slot`.
    fn despawn_parts(&mut self, slot: usize, stats: &mut VisualStats) {
        let Some(mapped) = self.mapped[slot].take() else {
            return;
        };
        for entity in mapped.parts {
            self.world.despawn(entity);
            stats.despawned += 1;
        }
    }

    /// Which level of detail an entity's model should be drawn at.
    fn select_lod(&self, simulation: &Simulation, id: EntityId) -> usize {
        let binding = binding_for(&self.bindings, self.featured_dummy, id);
        let Some(body) = simulation.world().body(id) else {
            return 0;
        };
        // From the middle of the entity rather than its feet, for the same reason a
        // character's level is: a tall model whose feet are close and whose head is far
        // should not swap while its head is still in frame.
        let centre = body.position + Vec3::new(0.0, simulation.world().height(id) * 0.5, 0.0);
        let distance = (centre - self.view.camera_position).length();
        binding.select_lod(distance, self.view.lod_bias)
    }

    /// Drives the player's character visual from the simulation and submits it.
    ///
    /// This is the whole of the gameplay-to-animation seam:
    ///
    /// ```text
    /// Body { position, facing_yaw, velocity, grounded, state }   Simulation::casting()
    ///                              │                                      │
    ///                              └──────────────┬───────────────────────┘
    ///                                             ▼
    ///                                     CharacterState
    /// ```
    ///
    /// Every field is read. None is written. The simulation does not know this ran.
    fn update_player_visual(&mut self, simulation: &Simulation) {
        let Some(visual) = self.player_visual.as_mut() else {
            return;
        };
        let game = simulation.world();
        let player = simulation.player().entity;
        let Some(body) = game.body(player) else {
            return;
        };

        // Interpolated between the last two ticks, exactly like every other drawable, so the
        // character and the camera that follows it agree about where it is.
        let alpha = simulation.timestep().alpha();
        let previous = game.previous_body(player).unwrap_or(body);
        let position = lerp_position(previous.position, body.position, alpha);
        let facing_yaw = lerp_angle(previous.facing_yaw, body.facing_yaw, alpha);

        let speed = body.horizontal_speed();
        let heading = heading(body.velocity, body.facing_yaw);
        let cast = simulation.casting();
        let state = CharacterState {
            position,
            facing_yaw,
            speed,
            heading,
            move_angle: move_angle(body.velocity, body.facing_yaw),
            grounded: body.grounded,
            // From the movement state the simulation already derives, rather than from a
            // second look at the velocity: two sources for "am I falling" is exactly what
            // `MovementState`'s own documentation says not to have.
            rising: body.state == MovementState::Jumping,
            height: game.height(player),
            cast: cast.map(|cast| CastVisual::for_duration(cast.duration)),
            cast_progress: cast.map_or(0.0, |cast| cast.fraction()),
            cast_started: last_tick(simulation, |event| match event {
                CombatEvent::CastStarted { source, tick, .. } if *source == player => Some(*tick),
                _ => None,
            }),
            released: last_tick(simulation, |event| match event {
                CombatEvent::AbilityUsed { source, tick, .. } if *source == player => Some(*tick),
                _ => None,
            }),
            // A model with a death clip plays it and holds the last frame; one without keeps
            // standing. Respawning clears it and the character stands straight back up.
            dead: !simulation.player_alive(),
        };

        if let Err(error) = visual.update(self.view.delta_seconds, state) {
            // A clip bound to the wrong skeleton. Validation rejects it at load, so reaching
            // here means an asset changed underneath a loaded model — worth a line, not a
            // crash, and the character holds its last pose.
            log::error!("character animation failed: {error}");
        }
        visual.submit(
            &mut self.world,
            self.view.camera_position,
            self.view.lod_bias,
        );
    }

    /// The per-entity visual state an entity is drawn with.
    ///
    /// This is where "what is happening to this one right now" turns into pixels, and it is
    /// the reason Milestone 7 exists: every rule below used to be inexpressible, because the
    /// only per-entity visual channel was the material handle and changing it meant
    /// respawning the render entity.
    ///
    /// Gameplay is not consulted about any of it. `engine-game` has no idea a tint exists.
    fn style_for(&self, simulation: &Simulation, id: EntityId, alive: bool) -> InstanceStyle {
        if !alive {
            // What used to be a second material handle. A dead dummy darkens towards a
            // lifeless grey; it does not need a `dead_material` for every model in the game.
            return InstanceStyle {
                tint: DEAD_TINT,
                emissive: 0.0,
            };
        }

        let mut style = InstanceStyle::NEUTRAL;

        // An enemy is tinted towards its own colour so a fight is legible at a glance: the
        // things that hit back do not look like the things that stand there. It is a tint
        // on the shared dummy material rather than a second model, which costs no upload,
        // no binding and no draw call - the same trick the health shade uses.
        let hostile = simulation.world().kind(id) == Some(EntityKind::Enemy);

        // Wounded enemies darken towards their material's own colour rather than towards
        // black: a health tint that reached zero would make a nearly-dead dummy invisible
        // against the arena floor.
        // Each monster's own colour, from its table row: a Bonecaster reads as bone and an
        // Acolyte as violet from across the room, and an empowered add glows. Numbers the
        // gameplay table carries and presentation reads — nothing here knows which monster
        // is which.
        let look = simulation.brain(id).map(|brain| brain.archetype().look);
        if let Some(health) = simulation.world().health(id) {
            let fraction = health.fraction().clamp(0.0, 1.0);
            let shade = HEALTH_TINT_FLOOR + (1.0 - HEALTH_TINT_FLOOR) * fraction;
            style.tint = match look {
                Some(look) if hostile => [
                    shade * look.tint[0],
                    shade * look.tint[1],
                    shade * look.tint[2],
                    1.0,
                ],
                _ if hostile => [shade, shade * 0.45, shade * 0.38, 1.0],
                _ => [shade, shade * 0.94, shade * 0.90, 1.0],
            };
        }
        if let Some(look) = look {
            style.emissive += look.glow * EMPOWERED_GLOW;
        }

        // The current target glows. This is what replaces "the marker is the only thing that
        // says which one you are hitting", and it costs no draw call and no respawn.
        if simulation.player().target == Some(id) {
            style.emissive += TARGET_GLOW;
        }

        self.apply_hit_flash(simulation, id, &mut style);

        style
    }

    /// Lifts `style` towards white while `id`'s last hit is still flashing.
    fn apply_hit_flash(&self, simulation: &Simulation, id: EntityId, style: &mut InstanceStyle) {
        // A recent hit flashes white. Driven from simulation time, so the flash lasts the
        // same wall-clock duration at 60 FPS and at 1,400.
        if let Some(hit) = self.hits.get(id.index()).copied().flatten()
            && hit.generation == id.generation()
        {
            let age = (simulation.time() as f32 - hit.at).max(0.0);
            let emphasis = if hit.critical {
                CRITICAL_FLASH_SCALE
            } else {
                1.0
            };
            let duration = HIT_FLASH_SECONDS * emphasis;
            if age < duration {
                let strength = 1.0 - age / duration;
                style.emissive += HIT_FLASH_STRENGTH * emphasis * strength;
                let lift = 1.0 + HIT_FLASH_LIFT * strength;
                style.tint = [
                    (style.tint[0] * lift).min(1.0),
                    (style.tint[1] * lift).min(1.0),
                    (style.tint[2] * lift).min(1.0),
                    style.tint[3],
                ];
            }
        }
    }

    /// Records that an entity was damaged at a simulation time, for the hit flash.
    ///
    /// Called from the combat-event reader rather than from gameplay: the simulation
    /// announces damage as an event and presentation observes it, which is the same
    /// arrangement the damage meter uses.
    pub fn note_hit(&mut self, id: EntityId, at: f32) {
        self.note_damage(id, at, false);
    }

    /// Records that an entity was damaged, and whether the hit critically struck.
    ///
    /// A later ordinary hit does **not** downgrade a critical flash that is still running:
    /// during a burn every target takes a tick every second, and letting one overwrite the
    /// flash of the Pyroblast that landed a frame earlier would hide exactly the hit worth
    /// seeing.
    pub fn note_damage(&mut self, id: EntityId, at: f32, critical: bool) {
        if self.hits.len() <= id.index() {
            self.hits.resize(id.index() + 1, None);
        }
        let slot = &mut self.hits[id.index()];
        let keep_critical = slot.is_some_and(|hit| {
            hit.critical
                && hit.generation == id.generation()
                && at - hit.at < HIT_FLASH_SECONDS * CRITICAL_FLASH_SCALE
        });
        *slot = Some(Hit {
            generation: id.generation(),
            at,
            critical: critical || keep_critical,
        });
    }

    /// Where and how an entity is drawn.
    ///
    /// Gameplay stores the position of an entity's **feet**, and every model in this project
    /// is authored standing on its own origin — so the two spaces agree and there is no lift.
    /// That is a change from the placeholder era, where a mesh was a cube centred on itself
    /// and had to be raised by half its height.
    ///
    /// The scale is **uniform**, from gameplay's height and the model's authored height.
    /// Stretching a model to the capsule's radius as well, which is what the cube did, would
    /// squash a dummy sideways to fit a collision volume that was never meant to describe its
    /// shape.
    fn transform_for(&self, simulation: &Simulation, id: EntityId, alive: bool) -> Transform {
        let game = simulation.world();
        let Some(body) = game.body(id) else {
            return Transform::IDENTITY;
        };
        let height = game.height(id);
        let binding = binding_for(&self.bindings, self.featured_dummy, id);
        let scale = if binding.model_height > 1e-3 {
            height / binding.model_height
        } else {
            1.0
        };

        // Where the entity is *now*, for a value of "now" between the last two ticks.
        let alpha = simulation.timestep().alpha();
        let previous = game.previous_body(id).unwrap_or(body);
        let position = lerp_position(previous.position, body.position, alpha);
        let yaw = lerp_angle(previous.facing_yaw, body.facing_yaw, alpha);

        if alive {
            Transform::from_translation(position)
                .with_rotation(Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0))
                .with_scale(Vec3::splat(scale))
        } else {
            // A dead dummy lies down. A tint alone reads as a recolour; falling over is what
            // makes a kill legible from across the arena. Tipping about its own base means
            // the model ends up lying on the floor rather than half inside it.
            Transform::from_translation(position + Vec3::new(0.0, game.radius(id), 0.0))
                .with_rotation(
                    Quat::from_axis_angle(Vec3::X, std::f32::consts::FRAC_PI_2)
                        * Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0),
                )
                .with_scale(Vec3::splat(scale))
        }
    }

    /// Moves the target marker over the selected target, or hides it.
    fn update_marker(&mut self, simulation: &Simulation) {
        // Never over the player's own head: selecting yourself (to heal, to read your own
        // frame) is a selection, and a cube floating above your character says nothing the
        // unit frame does not already say better.
        let player = simulation.player().entity;
        let target = simulation
            .player()
            .target
            .filter(|id| *id != player && simulation.world().is_alive(*id));

        let Some(target) = target else {
            self.world.set_visible(self.marker, false);
            return;
        };
        let Some(body) = simulation.world().body(target) else {
            self.world.set_visible(self.marker, false);
            return;
        };
        // Interpolated like everything else, or the marker would step at 60 Hz above a
        // target that moves smoothly.
        let previous = simulation
            .world()
            .previous_body(target)
            .unwrap_or(body)
            .position;
        let position = lerp_position(previous, body.position, simulation.timestep().alpha());

        let height = simulation.world().height(target);
        let transform = Transform::from_translation(
            position + Vec3::new(0.0, height + self.marker_height, 0.0),
        )
        .with_scale(Vec3::splat(MARKER_SIZE));
        self.world
            .set_transform(self.marker, transform, self.bindings.marker_bounds);
        self.world.set_visible(self.marker, true);
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::command::PlayerCommand;

    /// A binding over a real procedural model, with handles nothing uploaded.
    ///
    /// The extractor never dereferences a handle — that is the renderer's job — so a model
    /// can be placed, levelled and styled with no device present. `base` keeps the two
    /// dummy variants' handles distinguishable.
    fn rigid_binding(model: &engine_assets::ModelAsset, base: u32) -> ModelBinding {
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
                        .map(|(index, submesh)| ModelPart {
                            geometry: PartGeometry::Rigid(MeshHandle::new(
                                base + (level * 8 + index) as u32,
                                1,
                            )),
                            material: MaterialHandle::new(base + submesh.material as u32, 1),
                            bounds: submesh.bounds,
                        })
                        .collect(),
                    min_distance: lod.min_distance,
                    bounds: lod.bounds(),
                })
                .collect(),
            skinned: false,
            model_height: 1.9,
        }
    }

    fn bindings() -> VisualBindings {
        let unit = Sphere::new(Vec3::ZERO, 0.87);
        let single = engine_assets::procedural::training_dummy(
            engine_assets::procedural::DummyVariant::Single,
        );
        let group = engine_assets::procedural::training_dummy(
            engine_assets::procedural::DummyVariant::Group,
        );
        VisualBindings {
            dummy: rigid_binding(&single, 0),
            dummy_group: rigid_binding(&group, 100),
            ground_mesh: MeshHandle::new(900, 1),
            ground_bounds: Sphere::new(Vec3::ZERO, 1.0),
            ground_material: MaterialHandle::new(900, 1),
            marker_mesh: MeshHandle::new(901, 1),
            marker_bounds: unit,
            marker_material: MaterialHandle::new(901, 1),
        }
    }

    /// How many render entities one dummy costs at its finest level of detail.
    fn parts_per_dummy() -> usize {
        let Some(lod) = bindings().dummy.lods.first().cloned() else {
            panic!("LOD0 exists");
        };
        lod.parts.len()
    }

    /// A FireMage visual over handles nothing uploaded, for the same reason as above.
    fn player_visual() -> CharacterVisual {
        let Ok(model) = engine_assets::procedural::fire_mage() else {
            panic!("the FireMage must build");
        };
        let model = std::sync::Arc::new(model);
        let binding = ModelBinding {
            lods: model
                .lods
                .iter()
                .enumerate()
                .map(|(level, lod)| ModelLodBinding {
                    parts: lod
                        .submeshes
                        .iter()
                        .enumerate()
                        .map(|(index, submesh)| ModelPart {
                            geometry: PartGeometry::Skinned(
                                engine_render_data::SkinnedMeshHandle::new(
                                    (500 + level * 8 + index) as u32,
                                    1,
                                ),
                            ),
                            material: MaterialHandle::new(500 + submesh.material as u32, 1),
                            bounds: submesh.bounds,
                        })
                        .collect(),
                    min_distance: lod.min_distance,
                    bounds: lod.bounds(),
                })
                .collect(),
            skinned: true,
            model_height: 1.8,
        };
        match CharacterVisual::new(
            engine_assets::procedural::fire_mage_character(),
            model,
            binding,
        ) {
            Ok(visual) => visual,
            Err(error) => panic!("the FireMage must resolve: {error}"),
        }
    }

    fn extractor() -> RenderExtractor {
        let mut extractor = RenderExtractor::new(bindings(), 200.0);
        extractor.set_player_visual(player_visual());
        // The finest level everywhere, so a test that counts render entities is measuring
        // extraction rather than where the default camera happens to be. Level selection
        // has its own tests, which set a view on purpose.
        extractor.set_view(ViewParams {
            lod_bias: 0.0,
            ..ViewParams::default()
        });
        extractor
    }

    fn arena_sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    #[test]
    fn a_dead_player_is_drawn_dead_and_stands_up_when_revived() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let dead = |extractor: &RenderExtractor| {
            extractor
                .player_visual()
                .expect("the scene set one up")
                .state()
                .dead
        };
        assert!(!dead(&extractor));

        simulation.kill_player();
        extractor.extract(&simulation);
        assert!(
            dead(&extractor),
            "the player died and the model was not told"
        );

        simulation.revive_player();
        extractor.extract(&simulation);
        assert!(!dead(&extractor), "a revived player is still drawn dead");
    }

    #[test]
    fn a_blink_is_reported_as_a_teleport_and_running_is_not() {
        // The distinction the camera turns on: both move the mage several metres between two
        // ticks, and only one of them is somewhere they never travelled.
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);
        assert!(!player_teleported(&simulation), "running is not a teleport");

        simulation
            .cast(engine_game::firemage::FireAbility::Blink)
            .expect("blink is ready");
        assert!(player_teleported(&simulation));

        // And it is over once the frame it happened on is: the stream is cleared per
        // advance, so the camera snaps once rather than for as long as it keeps looking.
        simulation.step_once();
        assert!(!player_teleported(&simulation));
    }

    #[test]
    fn every_gameplay_entity_becomes_something_drawable() {
        let simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let world = extractor.render_world();

        // Every dummy costs one render entity per submesh of its model; the player costs
        // none, because it is a skinned character rather than a `RenderWorld` slot. Plus the
        // floor and the target marker.
        let dummies = simulation.world().len() - 1;
        let parts = dummies * parts_per_dummy();
        assert_eq!(world.len(), parts + 2);
        // The player is drawn, and it is drawn through the skinned path.
        assert_eq!(world.skinned().len(), 4, "the FireMage's four submeshes");
        assert!(!world.skinned().palette().is_empty());

        assert_eq!(extractor.stats().extracted, simulation.world().len());
        assert_eq!(extractor.stats().spawned, parts);
    }

    #[test]
    fn the_player_is_drawn_standing_on_the_floor_facing_where_it_moves() {
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);

        let mut extractor = extractor();
        extractor.extract(&simulation);

        let player = simulation.player().entity;
        let visual = extractor.player_visual().expect("the scene set one up");
        let transform = visual.transform();
        let body = simulation.player_body().expect("the player exists");
        let height = simulation.world().height(player);

        // Within the controller's skin width of standing on the floor. Collision keeps the
        // capsule a few millimetres clear of every surface it rests on, and the model is
        // authored standing on its own origin — so the drawn position *is* the feet, with
        // none of the half-height lift a centred cube needed.
        assert!(
            transform.translation.y.abs() < 0.02,
            "the model is not standing on the floor: {transform:?}"
        );
        let expected_scale = height / visual.def().model_height;
        assert!((transform.scale.y - expected_scale).abs() < 1e-4);

        // Since Milestone 9 the drawn position is *interpolated*, so it is the blend of the
        // last two ticks rather than the latest one. At alpha 0 — which is where
        // `step_many` leaves the accumulator — that is the previous tick exactly. This is
        // the deliberate one-tick presentation lag; extrapolating instead would overshoot
        // and snap back whenever the character changed direction.
        let previous = simulation
            .world()
            .previous_body(player)
            .expect("history exists");
        let drawn = lerp_position(
            previous.position,
            body.position,
            simulation.timestep().alpha(),
        );
        assert!((transform.translation.z - drawn.z).abs() < 1e-4);
        assert!(
            transform.translation.z < 0.0,
            "moving forward should still have carried it down -Z: {transform:?}"
        );
    }

    #[test]
    fn moving_under_a_camera_pinned_facing_reports_which_way_it_goes() {
        let run = |forward: f32, right: f32| {
            let mut simulation = arena_sim();
            simulation.push_command(PlayerCommand::Move {
                forward,
                right,
                yaw: 0.0,
                face_camera: true,
            });
            simulation.step_many(30);
            let mut extractor = extractor();
            extractor.extract(&simulation);
            extractor
                .player_visual()
                .expect("the scene set one up")
                .state()
                .heading
        };
        assert_eq!(run(-1.0, 0.0), Heading::Backward);
        assert_eq!(run(1.0, 0.0), Heading::Forward);
        assert_eq!(run(0.0, 1.0), Heading::Right);
        assert_eq!(run(0.0, -1.0), Heading::Left);
        assert_eq!(
            run(1.0, 1.0),
            Heading::Forward,
            "a diagonal forwards runs forwards"
        );
        assert_eq!(
            run(-1.0, 1.0),
            Heading::Backward,
            "a diagonal backwards backpedals"
        );
    }

    #[test]
    fn heading_turns_with_the_facing() {
        // Facing +X, which is what `face_movement` makes of moving +X: moving +X is then
        // forwards, +Z is the character's right and -Z its left.
        let yaw = -std::f32::consts::FRAC_PI_2;
        assert_eq!(heading(Vec3::new(1.0, 0.0, 0.0), yaw), Heading::Forward);
        assert_eq!(heading(Vec3::new(0.0, 0.0, -1.0), yaw), Heading::Left);
        assert_eq!(heading(Vec3::new(0.0, 0.0, 1.0), yaw), Heading::Right);
        assert_eq!(heading(Vec3::ZERO, yaw), Heading::Forward);
    }

    #[test]
    fn gameplay_state_does_not_leak_into_the_render_world() {
        // The architectural assertion: `RenderWorld` carries transforms, handles, bounds and
        // visibility, and there is nowhere in it for health to go. What this test can check
        // is that damage alone changes nothing about what is drawn.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let target = simulation.player().target.expect("a dummy is targeted");
        let before = extractor.render_world().transforms()[extractor
            .render_entity(target)
            .expect("the dummy was extracted")
            .index()];

        simulation.world_mut().damage(target, 100.0);
        simulation.step_once();
        extractor.extract(&simulation);

        let after = extractor.render_world().transforms()[extractor
            .render_entity(target)
            .expect("the dummy is still extracted")
            .index()];
        assert_eq!(before, after, "a wounded dummy is drawn no differently");
    }

    #[test]
    fn a_dead_dummy_falls_over_and_changes_material() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let upright = extractor
            .render_entity(target)
            .expect("the dummy was extracted");
        assert_ne!(
            extractor.render_world().styles()[upright.index()].tint,
            DEAD_TINT,
            "a living dummy is not tinted like a corpse"
        );

        simulation.world_mut().damage(target, 1_000_000.0);
        simulation.step_once();
        extractor.extract(&simulation);

        let fallen = extractor
            .render_entity(target)
            .expect("the corpse is still drawn");
        assert_eq!(
            extractor.render_world().styles()[fallen.index()].tint,
            DEAD_TINT
        );
        assert_eq!(extractor.stats().restyled, 1);

        let transform = extractor.render_world().transforms()[fallen.index()];
        assert!(
            transform.translation.y < 1.0,
            "the corpse is still standing: {transform:?}"
        );
    }

    #[test]
    fn dying_changes_how_a_dummy_is_drawn_without_respawning_its_render_entity() {
        // Milestone 7, stated as the property it exists for. Phase 3 expressed "it died" by
        // despawning the render entity and respawning it with a different material, which
        // churned a slot for a colour change and invalidated every outstanding handle.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let before = extractor.render_entity(target).expect("it was extracted");

        simulation.world_mut().damage(target, 1_000_000.0);
        simulation.step_once();
        extractor.extract(&simulation);

        let after = extractor.render_entity(target).expect("it is still drawn");
        assert_eq!(after, before, "the render entity survived the death");
        assert_eq!(extractor.stats().spawned, 0, "nothing was respawned");
        assert_eq!(extractor.stats().despawned, 0, "nothing was despawned");
        assert_eq!(
            extractor.stats().restyled,
            1,
            "the change was still noticed"
        );
        assert_eq!(
            extractor.render_world().styles()[after.index()].tint,
            DEAD_TINT
        );
    }

    #[test]
    fn a_wounded_dummy_darkens_without_any_render_churn() {
        // The health tint: a per-entity visual that changes every time damage lands, and
        // therefore the one that would have been unaffordable as a respawn.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let entity = extractor.render_entity(target).expect("it was extracted");
        let healthy = extractor.render_world().styles()[entity.index()].tint[0];

        let max = simulation
            .world()
            .health(target)
            .expect("the dummy has health")
            .max();
        simulation.world_mut().damage(target, max * 0.8);
        extractor.extract(&simulation);

        let wounded = extractor.render_world().styles()[entity.index()].tint[0];
        assert!(
            wounded < healthy,
            "a wounded dummy should darken: {wounded} vs {healthy}"
        );
        assert!(
            wounded >= HEALTH_TINT_FLOOR,
            "it must not fade into the floor: {wounded}"
        );
        assert_eq!(extractor.stats().spawned, 0);
        assert_eq!(extractor.stats().despawned, 0);
    }

    #[test]
    fn the_current_target_glows_and_stops_glowing_when_it_is_dropped() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let entity = extractor.render_entity(target).expect("it was extracted");
        assert!(
            extractor.render_world().styles()[entity.index()].emissive > 0.0,
            "the target should be lit up"
        );

        simulation.player_mut().target = None;
        extractor.extract(&simulation);
        assert_eq!(
            extractor.render_world().styles()[entity.index()].emissive,
            0.0,
            "an unselected dummy stops glowing"
        );
        assert_eq!(extractor.stats().spawned, 0, "selection is not a respawn");
    }

    #[test]
    fn a_hit_flashes_and_fades_on_simulation_time() {
        // Driven from simulated seconds rather than frames, so the flash lasts the same
        // wall-clock time at 60 FPS and at 1,400.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let target = simulation.player().target.expect("a dummy is targeted");
        let entity = extractor.render_entity(target).expect("it was extracted");
        let resting = extractor.render_world().styles()[entity.index()].emissive;

        extractor.note_hit(target, simulation.time() as f32);
        extractor.extract(&simulation);
        let flashing = extractor.render_world().styles()[entity.index()].emissive;
        assert!(
            flashing > resting,
            "the hit should brighten it: {flashing} vs {resting}"
        );

        // Past the flash's lifetime it is back to where it started.
        simulation.step_many(60);
        extractor.extract(&simulation);
        let faded = extractor.render_world().styles()[entity.index()].emissive;
        assert!(
            (faded - resting).abs() < 1e-4,
            "the flash should have faded: {faded} vs {resting}"
        );
    }

    #[test]
    fn a_recycled_slot_does_not_inherit_the_previous_occupants_flash() {
        // The generation check on a hit. Without it a fresh dummy would spawn mid-flash.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let old = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        extractor.note_hit(old, simulation.time() as f32);
        simulation.world_mut().despawn(old);
        let new = simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(5.0, 0.0, -5.0),
                100.0,
            ));
        assert_eq!(new.index(), old.index(), "the slot was reused");

        extractor.extract(&simulation);
        let entity = extractor
            .render_entity(new)
            .expect("the new dummy is drawn");
        let style = extractor.render_world().styles()[entity.index()];
        assert_eq!(
            style.emissive, 0.0,
            "a new entity must not inherit a flash aimed at the last one"
        );
    }

    #[test]
    fn a_long_fight_causes_no_render_entity_churn_at_all() {
        // The benchmark-shaped assertion Milestone 7 asks for: visual state changes
        // constantly for hundreds of ticks and the render world never gains or loses a slot.
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(32));
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let slots = extractor.render_world().slot_count();
        let live = extractor.render_world().len();

        let mut churn = 0;
        for tick in 0..600 {
            // Damage something every tick, cycle the target, and kill a few outright.
            if let Some(target) = simulation.player().target {
                simulation.world_mut().damage(target, 3.0);
                extractor.note_hit(target, simulation.time() as f32);
            }
            if tick % 15 == 0 {
                simulation.push_command(PlayerCommand::CycleTarget);
            }
            simulation.step_once();
            extractor.extract(&simulation);
            churn += extractor.stats().spawned + extractor.stats().despawned;
        }

        assert_eq!(churn, 0, "no entity was spawned or despawned in 600 ticks");
        assert_eq!(extractor.render_world().slot_count(), slots);
        assert_eq!(extractor.render_world().len(), live);
    }

    #[test]
    fn a_position_is_blended_by_the_alpha_and_clamped_outside_it() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(10.0, 4.0, -2.0);
        assert_eq!(lerp_position(a, b, 0.0), a);
        assert_eq!(lerp_position(a, b, 1.0), b);
        assert_eq!(lerp_position(a, b, 0.5), Vec3::new(5.0, 2.0, -1.0));
        // An alpha outside [0, 1) would extrapolate, which shows up as an entity that
        // overshoots and snaps back.
        assert_eq!(lerp_position(a, b, 2.0), b);
        assert_eq!(lerp_position(a, b, -1.0), a);
    }

    #[test]
    fn a_yaw_takes_the_short_way_round_the_wrap_point() {
        // The classic interpolation bug: a character turning past south whips around
        // through north because the lerp went the long way.
        use std::f32::consts::PI;
        let just_under = PI - 0.1;
        let just_over = -PI + 0.1;

        let halfway = lerp_angle(just_under, just_over, 0.5);
        // The short way is 0.2 radians across the wrap, so halfway is 0.1 past +pi.
        let expected = just_under + 0.1;
        assert!(
            (halfway - expected).abs() < 1e-4,
            "took the long way: {halfway} rather than {expected}"
        );

        // And the ordinary case is unchanged.
        assert!((lerp_angle(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(lerp_angle(0.4, 0.9, 0.0), 0.4);
    }

    #[test]
    fn a_moving_character_is_drawn_between_its_last_two_ticks() {
        // The property the whole milestone exists for. Two extractions with the same
        // simulation state but different alphas must place the character differently.
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);

        let player = simulation.player().entity;
        let world = simulation.world();
        let previous = world
            .previous_body(player)
            .expect("history exists")
            .position;
        let current = world.body(player).expect("the player exists").position;
        assert_ne!(
            previous, current,
            "a moving character must have moved between ticks"
        );

        // Interpolation covers the whole gap: the drawn position at alpha 0 is the previous
        // tick and at alpha 1 is the current one.
        assert_eq!(lerp_position(previous, current, 0.0), previous);
        assert_eq!(lerp_position(previous, current, 1.0), current);
        let middle = lerp_position(previous, current, 0.5);
        assert!(middle.z > current.z && middle.z < previous.z || middle.z < current.z);
    }

    #[test]
    fn a_still_character_is_drawn_in_exactly_one_place() {
        // Interpolation must not introduce jitter when nothing is moving.
        let mut simulation = arena_sim();
        simulation.step_many(30);
        let player = simulation.player().entity;
        let world = simulation.world();
        let previous = world.previous_body(player).expect("history").position;
        let current = world.body(player).expect("the player").position;
        // "Still" is now still to within an ulp rather than exactly. A grounded character
        // is pushed a skin width out of the floor every tick, and the arithmetic that does
        // it lands on one of two adjacent `f32` values — twenty nanometres apart, which is
        // four orders of magnitude below anything a pixel can show.
        assert!(
            (previous - current).length() < 1e-6,
            "{previous:?} {current:?}"
        );
        for alpha in [0.0, 0.25, 0.5, 0.99] {
            assert!((lerp_position(previous, current, alpha) - current).length() < 1e-6);
        }
    }

    #[test]
    fn the_camera_follows_the_interpolated_position_not_the_stepped_one() {
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);
        // Advance a partial tick so alpha is non-zero without a tick running.
        simulation.advance(std::time::Duration::from_micros(8_000));

        let alpha = simulation.timestep().alpha();
        assert!(alpha > 0.0, "a partial step should leave a remainder");

        let drawn = interpolated_player_position(&simulation);
        let stepped = simulation.player_position();
        assert_ne!(
            drawn, stepped,
            "mid-tick, the drawn position is ahead of the last simulated one"
        );
    }

    #[test]
    fn a_freshly_spawned_entity_does_not_slide_in_from_its_slots_previous_occupant() {
        // Without per-spawn history reset, a recycled slot would interpolate the new entity
        // in from wherever the old one stood.
        let mut simulation = arena_sim();
        let old = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        simulation.world_mut().despawn(old);
        let new = simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(50.0, 0.0, -50.0),
                100.0,
            ));

        let world = simulation.world();
        let previous = world.previous_body(new).expect("history").position;
        let current = world.body(new).expect("the dummy").position;
        assert_eq!(
            previous, current,
            "a new entity starts where it is, not where the last one was"
        );
    }

    #[test]
    fn interpolation_is_deterministic() {
        // Two identical runs must place the character identically, or a replay would render
        // differently from the run it recorded.
        let build = || {
            let mut simulation = arena_sim();
            for _ in 0..120 {
                simulation.push_command(PlayerCommand::Move {
                    forward: 1.0,
                    right: 0.4,
                    yaw: 0.3,
                    face_camera: false,
                });
                simulation.step_once();
            }
            simulation.advance(std::time::Duration::from_micros(5_000));
            interpolated_player_position(&simulation)
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn the_drawn_floor_can_be_moved_out_of_a_scene_that_brings_its_own() {
        // A scene whose geometry has a face at y = 0 must be able to move this plane away
        // from y = 0. Two coplanar surfaces are a depth-buffer tie, and a tie flickers.
        let mut extractor = extractor();
        let ground = extractor.ground();
        assert_eq!(
            extractor.render_world().transforms()[ground.index()].translation,
            Vec3::ZERO
        );

        extractor.set_ground_height(-15.0);
        let moved = extractor.render_world().transforms()[ground.index()];
        assert_eq!(moved.translation, Vec3::new(0.0, -15.0, 0.0));
        assert_eq!(
            moved.scale,
            Vec3::new(200.0, 1.0, 200.0),
            "moving the floor must not resize it"
        );

        // And extraction, which rewrites every entity it owns, leaves it where it was put.
        extractor.extract(&arena_sim());
        assert_eq!(
            extractor.render_world().transforms()[ground.index()].translation,
            Vec3::new(0.0, -15.0, 0.0)
        );
    }

    #[test]
    fn a_despawned_entity_stops_being_drawn() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let before = extractor.render_world().len();

        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        simulation.world_mut().despawn(dummy);
        extractor.extract(&simulation);

        assert_eq!(
            extractor.render_world().len(),
            before - parts_per_dummy(),
            "every part of the dummy stopped being drawn"
        );
        assert_eq!(extractor.stats().despawned, parts_per_dummy());
        assert_eq!(extractor.render_entity(dummy), None);
    }

    #[test]
    fn a_reused_gameplay_slot_gets_its_own_render_entity() {
        // The stale-handle case, in the one place both handle spaces meet: a new dummy in a
        // recycled slot must not inherit the previous occupant's render entity.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let old = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        let old_render = extractor.render_entity(old).expect("it was extracted");
        simulation.world_mut().despawn(old);
        let new = simulation
            .world_mut()
            .spawn(engine_game::world::EntityDesc::dummy(
                Vec3::new(5.0, 0.0, -5.0),
                100.0,
            ));
        assert_eq!(new.index(), old.index(), "the slot was reused");

        extractor.extract(&simulation);
        assert_eq!(extractor.render_entity(old), None, "the old id is stale");
        let new_render = extractor
            .render_entity(new)
            .expect("the new dummy is drawn");
        assert_ne!(new_render, old_render);
    }

    #[test]
    fn the_target_marker_follows_the_target_and_hides_when_there_is_none() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);

        let marker = extractor.marker;
        assert!(extractor.render_world().is_drawable(marker.index()));
        let target = simulation.player().target.expect("a dummy is targeted");
        let target_position = simulation
            .world()
            .position(target)
            .expect("the dummy exists");
        let marker_position = extractor.render_world().transforms()[marker.index()].translation;
        assert!((marker_position.x - target_position.x).abs() < 1e-4);
        assert!(marker_position.y > target_position.y, "it floats above");

        // Kill every dummy: nothing is targetable and the marker goes away.
        let dummies: Vec<_> = arena::dummies(simulation.world()).collect();
        for dummy in dummies {
            simulation.world_mut().damage(dummy, 1_000_000.0);
        }
        simulation.step_once();
        extractor.extract(&simulation);
        assert!(!extractor.render_world().is_drawable(marker.index()));
    }

    #[test]
    fn selecting_yourself_puts_no_marker_over_your_head() {
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        simulation.player_mut().target = Some(simulation.player().entity);
        extractor.extract(&simulation);
        assert!(
            !extractor
                .render_world()
                .is_drawable(extractor.marker.index())
        );
    }

    #[test]
    fn a_steady_scene_stops_allocating() {
        // Milestone 25: extraction is a copy into storage that already exists.
        let mut simulation = arena_sim();
        let mut extractor = extractor();
        for _ in 0..10 {
            extractor.extract(&simulation);
        }
        let mapped = extractor.mapped.capacity();
        let slots = extractor.render_world().slot_count();

        for _ in 0..1_000 {
            simulation.push_command(PlayerCommand::Move {
                forward: 1.0,
                right: 0.3,
                yaw: 0.2,
                face_camera: false,
            });
            simulation.step_once();
            extractor.extract(&simulation);
        }

        assert_eq!(extractor.mapped.capacity(), mapped);
        assert_eq!(extractor.render_world().slot_count(), slots);
        assert_eq!(extractor.stats().spawned, 0);
        assert_eq!(extractor.stats().despawned, 0);
    }

    #[test]
    fn a_thousand_dummies_extract_without_a_device() {
        let simulation = arena::training_arena(&ArenaConfig::with_group(1_000));
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let world = extractor.render_world();
        // A thousand group dummies, the featured one and the friendly one in the main hall,
        // each at its finest level, plus the floor and the marker. The player is in the
        // skinned list rather than here.
        assert_eq!(world.len(), 1_002 * parts_per_dummy() + 2);
        assert_eq!(world.skinned().len(), 4);
    }

    #[test]
    fn a_distant_dummy_is_drawn_at_a_coarser_level_and_costs_fewer_draws() {
        // The level-of-detail architecture, end to end through extraction: the same dummy
        // costs three render entities up close and two far away, and crossing the threshold
        // is what respawns them.
        let simulation = arena_sim();
        let mut extractor = RenderExtractor::new(bindings(), 200.0);
        extractor.set_view(ViewParams {
            camera_position: Vec3::new(0.0, 1.5, 0.0),
            delta_seconds: 0.016,
            lod_bias: 1.0,
        });

        let dummy = arena::dummies(simulation.world())
            .next()
            .expect("the arena has dummies");
        extractor.extract(&simulation);
        let near = extractor
            .render_parts(dummy)
            .expect("the dummy was extracted")
            .len();

        extractor.set_view(ViewParams {
            camera_position: Vec3::new(0.0, 1.5, 400.0),
            delta_seconds: 0.016,
            lod_bias: 1.0,
        });
        extractor.extract(&simulation);
        let far = extractor
            .render_parts(dummy)
            .expect("the dummy is still extracted")
            .len();

        assert!(near > far, "near {near} parts, far {far} parts");
        assert!(far > 0, "a distant dummy is still drawn");
    }

    #[test]
    fn a_character_animates_and_lands_in_the_skinned_list() {
        // Milestones 10 to 12 through the seam that joins them: gameplay state becomes an
        // animation state, a pose, a palette and a placed draw, with the simulation never
        // consulted about any of it.
        let mut simulation = arena_sim();
        simulation.push_command(PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        });
        simulation.step_many(30);

        let mut extractor = extractor();
        extractor.set_view(ViewParams {
            camera_position: Vec3::new(0.0, 2.0, 6.0),
            delta_seconds: 0.016,
            lod_bias: 1.0,
        });
        extractor.extract(&simulation);

        let visual = extractor.player_visual().expect("the scene set one up");
        assert!(
            matches!(
                visual.slot(),
                engine_assets::AnimationSlot::Walk | engine_assets::AnimationSlot::Run
            ),
            "a running character should not be idle: {:?}",
            visual.slot()
        );

        let world = extractor.render_world();
        assert_eq!(world.skinned().len(), 4);
        assert_eq!(world.skinned().palette().len(), 21);

        // The drawn orientation is gameplay's, not the animation's.
        let body = simulation.player_body().expect("the player exists");
        let drawn = visual.transform().rotation;
        let expected = engine_core::math::Quat::from_yaw_pitch_roll(body.facing_yaw, 0.0, 0.0);
        assert!(
            (drawn.rotate(Vec3::Z) - expected.rotate(Vec3::Z)).length() < 1e-3,
            "the model does not face where gameplay says"
        );
    }

    #[test]
    fn the_skinned_list_is_rebuilt_rather_than_accumulated() {
        // It is transient data, like the UI draw list. Extracting twice must not draw the
        // character twice.
        let simulation = arena_sim();
        let mut extractor = extractor();
        extractor.extract(&simulation);
        let first = extractor.render_world().skinned().len();
        extractor.extract(&simulation);
        assert_eq!(extractor.render_world().skinned().len(), first);
    }
}
