//! Drawing the collision world, for a developer who needs to see why they are stuck.
//!
//! ```text
//! CollisionWorld (simulation)          RenderWorld (presentation)
//! -------------------------            --------------------------
//! Aabb, Obb                    --->    a box per shape
//! the character's capsule      --->    a cylinder and two caps
//! contacts from a live query   --->    a marker and an arrow per contact
//! grounded / airborne          --->    the capsule's colour
//! the camera's obstruction check --->   pivot, sweep, hit and normal, desired and actual eye
//! ```
//!
//! Two properties matter more than what it looks like:
//!
//! 1. **It draws the collision world, not a copy of it.** Every box here comes from the
//!    shape the simulation is actually testing against. A debug view built from a second
//!    description of the level is a debug view that lies exactly when it is needed.
//! 2. **It costs nothing when it is off.** The render entities are despawned, not hidden
//!    and skipped: an invisible entity still occupies a culler slot and is still walked
//!    every frame. `set_enabled(false)` leaves the render world exactly as it was before
//!    the debug view existed.
//!
//! It is developer-only by construction: `engine-app` builds it behind the `debug-overlay`
//! feature, so a `--no-default-features` build cannot reach this module at all.

use engine_collision::CharacterConfig;

use crate::camera::CameraProbe;
use engine_collision::shape::Shape;
use engine_collision::world::{CollisionWorld, QueryBuffers};
use engine_core::math::{Quat, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_game::sim::Simulation;
use engine_render_data::{
    InstanceStyle, MaterialHandle, MeshHandle, RenderEntity, RenderEntityDesc, RenderWorld,
};

/// The meshes and materials the debug view draws with.
///
/// Supplied by the application, because uploading a mesh needs a device and this crate does
/// not have one — the same arrangement [`crate::extract::VisualBindings`] uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugBindings {
    /// A unit cube, centred on its own origin.
    pub box_mesh: MeshHandle,
    /// Local bounds of [`DebugBindings::box_mesh`].
    pub box_bounds: Sphere,
    /// A unit-radius sphere, centred on its own origin.
    pub sphere_mesh: MeshHandle,
    /// Local bounds of [`DebugBindings::sphere_mesh`].
    pub sphere_bounds: Sphere,
    /// Material for static collision geometry.
    pub shape_material: MaterialHandle,
    /// Material for the character's capsule.
    pub capsule_material: MaterialHandle,
    /// Material for contact points and normals.
    pub contact_material: MaterialHandle,
}

/// How many debug entities the view is allowed.
///
/// The collision demo course is about thirty shapes; a thousand is far past anything a
/// developer can read on screen, and the cap is what stops a mistake in a level from
/// turning the debug key into a hang.
pub const MAX_DEBUG_ENTITIES: usize = 1_024;

/// Length of a drawn contact normal, in metres.
const NORMAL_LENGTH: f32 = 0.7;

/// Thickness of a drawn contact normal, in metres.
const NORMAL_THICKNESS: f32 = 0.05;

/// Size of the marker drawn at a contact point, in metres.
const CONTACT_SIZE: f32 = 0.12;

/// Colour of static collision geometry.
const SHAPE_TINT: [f32; 3] = [0.30, 0.75, 1.00];

/// Colour of the capsule while it is standing on something walkable.
const GROUNDED_TINT: [f32; 3] = [0.35, 1.00, 0.45];

/// Colour of the capsule while it is in the air.
const AIRBORNE_TINT: [f32; 3] = [1.00, 0.72, 0.25];

/// Colour of a contact on a walkable surface.
const CONTACT_GROUND_TINT: [f32; 3] = [0.40, 1.00, 0.60];

/// Colour of a contact on a wall or ceiling.
const CONTACT_WALL_TINT: [f32; 3] = [1.00, 0.35, 0.35];

/// Colour of a line-of-sight ray that reached its target.
const LOS_CLEAR_TINT: [f32; 3] = [0.40, 1.00, 0.55];

/// Colour of a line-of-sight ray that was stopped.
const LOS_BLOCKED_TINT: [f32; 3] = [1.00, 0.30, 0.30];

/// Thickness of the drawn line-of-sight ray, in metres.
const LOS_THICKNESS: f32 = 0.06;

/// Size of the marker drawn at each end of the ray, and at the blocking point.
const LOS_MARKER_SIZE: f32 = 0.22;

/// Colour of the camera sweep while nothing is in its way.
const CAMERA_CLEAR_TINT: [f32; 3] = [0.55, 0.85, 1.00];

/// Colour of the camera sweep, and its hit, while geometry is holding the camera in.
const CAMERA_BLOCKED_TINT: [f32; 3] = [1.00, 0.55, 0.15];

/// Thickness of the drawn camera sweep, in metres. Thinner than the line-of-sight ray, so
/// the two can be told apart where they cross.
const CAMERA_SWEEP_THICKNESS: f32 = 0.03;

/// Size of the markers on the camera sweep, in metres.
const CAMERA_MARKER_SIZE: f32 = 0.10;

/// The collision world, drawn.
#[derive(Debug)]
pub struct CollisionDebug {
    bindings: DebugBindings,
    /// The render entities this view owns. Empty while it is disabled.
    entities: Vec<RenderEntity>,
    /// Its own query buffers: the debug view asks the collision world what is touching the
    /// character, rather than asking the simulation to remember on its behalf.
    buffers: QueryBuffers,
    enabled: bool,
    /// How many entities the last rebuild used.
    drawn: usize,
    /// How many contacts the last rebuild found.
    contacts: usize,
    /// The camera's last obstruction check, handed in by whoever owns the camera.
    camera: Option<CameraProbe>,
}

impl CollisionDebug {
    /// A disabled debug view.
    #[must_use]
    pub fn new(bindings: DebugBindings) -> Self {
        Self {
            bindings,
            entities: Vec::with_capacity(64),
            buffers: QueryBuffers::with_capacity(16),
            enabled: false,
            drawn: 0,
            contacts: 0,
            camera: None,
        }
    }

    /// Hands the view the camera's last obstruction check, to be drawn on the next rebuild.
    ///
    /// Handed in rather than read, because the camera belongs to the application and this
    /// view only ever sees the simulation.
    pub fn set_camera_probe(&mut self, probe: Option<CameraProbe>) {
        self.camera = probe;
    }

    /// Whether the view is on.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Render entities the last rebuild used.
    #[must_use]
    pub fn drawn(&self) -> usize {
        self.drawn
    }

    /// Contacts the last rebuild found under the character.
    #[must_use]
    pub fn contacts(&self) -> usize {
        self.contacts
    }

    /// Turns the view on or off, despawning everything it owns when off.
    pub fn set_enabled(&mut self, world: &mut RenderWorld, enabled: bool) {
        if enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            for entity in self.entities.drain(..) {
                world.despawn(entity);
            }
            self.drawn = 0;
            self.contacts = 0;
        }
    }

    /// Rebuilds the drawn geometry from the simulation's collision world.
    ///
    /// Called once a frame while the view is on. Entities are reused across frames — the
    /// pool grows to the scene's size and then stops — so the per-frame cost is writing
    /// transforms, not spawning.
    pub fn rebuild(&mut self, render: &mut RenderWorld, simulation: &Simulation) {
        if !self.enabled {
            return;
        }
        let mut used = 0;
        self.draw_statics(render, simulation.collision_world(), &mut used);
        self.draw_character(render, simulation, &mut used);
        self.draw_line_of_sight(render, simulation, &mut used);
        self.draw_camera(render, &mut used);

        // Anything the pool still holds beyond what this frame needed is parked out of
        // sight rather than despawned: a frame with one contact fewer than the last must
        // not churn a slot.
        for index in used..self.entities.len() {
            render.set_visible(self.entities[index], false);
        }
        self.drawn = used;
    }

    /// Draws every static shape that has a finite size.
    fn draw_statics(&mut self, render: &mut RenderWorld, world: &CollisionWorld, used: &mut usize) {
        for id in world.statics() {
            let Some(shape) = world.shape(id) else {
                continue;
            };
            let transform = match shape {
                // A plane is infinite. Drawing it as a very large thin box would put a
                // ceiling over the level from the wrong side and hide everything under it.
                Shape::Plane(_) => continue,
                Shape::Aabb(bounds) => Transform::from_translation(bounds.center())
                    .with_scale(bounds.half_extents() * 2.0),
                Shape::Obb(obb) => Transform::from_translation(obb.center)
                    .with_rotation(obb.rotation)
                    .with_scale(obb.half_extents * 2.0),
                // The terrain is already the picture: it is drawn as itself, out of the same
                // heights this shape was built from, so a wireframe box around the whole
                // level would say nothing except that a terrain exists.
                Shape::Heightfield(_) => continue,
            };
            self.push(
                render,
                transform,
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.shape_material,
                tint(SHAPE_TINT, 0.10),
                used,
            );
        }
    }

    /// Draws the character's capsule and whatever it is touching.
    fn draw_character(
        &mut self,
        render: &mut RenderWorld,
        simulation: &Simulation,
        used: &mut usize,
    ) {
        let Some(body) = simulation.player_body() else {
            return;
        };
        let config: CharacterConfig = simulation.config().character;
        let capsule = config.capsule_at(body.position);
        let colour = if body.grounded {
            GROUNDED_TINT
        } else {
            AIRBORNE_TINT
        };
        let style = tint(colour, 0.25);

        // The cylinder between the caps, then the caps themselves. Three primitives rather
        // than a capsule mesh, because the renderer has a cube and a sphere and a capsule
        // mesh would exist only for this.
        let spine = capsule.segment.direction().length();
        if spine > 1e-4 {
            self.push(
                render,
                Transform::from_translation((capsule.segment.start + capsule.segment.end) * 0.5)
                    .with_scale(Vec3::new(capsule.radius * 2.0, spine, capsule.radius * 2.0)),
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.capsule_material,
                style,
                used,
            );
        }
        for centre in [capsule.segment.start, capsule.segment.end] {
            self.push(
                render,
                Transform::from_translation(centre).with_scale(Vec3::splat(capsule.radius)),
                self.bindings.sphere_mesh,
                self.bindings.sphere_bounds,
                self.bindings.capsule_material,
                style,
                used,
            );
        }

        // What the character is touching, asked of the collision world directly. The
        // simulation is not made to remember contacts for a view that is off most of the
        // time.
        //
        // Asked with a slightly *fatter* capsule than the one that moves. Resolution leaves
        // the character a skin width clear of every surface, so a character standing
        // perfectly still is touching nothing at all — and a debug view that draws no
        // contact under the feet of a character that is visibly standing on the floor is
        // a debug view nobody will trust. Two skin widths is the smallest inflation that
        // shows a resting contact, and it is a view-only number: nothing here feeds back
        // into movement.
        let probe = engine_collision::shape::Capsule::new(
            capsule.segment,
            capsule.radius + config.skin_width * 2.0,
        );
        simulation
            .collision_world()
            .overlap_capsule(&probe, config.mask, &mut self.buffers);
        self.contacts = self.buffers.contacts().len();

        let contacts: Vec<(Vec3, Vec3, bool)> = self
            .buffers
            .contacts()
            .iter()
            .map(|hit| {
                (
                    hit.contact.point,
                    hit.contact.normal,
                    config.is_walkable(hit.contact.normal),
                )
            })
            .collect();
        for (point, normal, walkable) in contacts {
            let colour = if walkable {
                CONTACT_GROUND_TINT
            } else {
                CONTACT_WALL_TINT
            };
            self.push(
                render,
                Transform::from_translation(point).with_scale(Vec3::splat(CONTACT_SIZE)),
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.contact_material,
                tint(colour, 0.9),
                used,
            );
            self.push(
                render,
                Transform::from_translation(point + normal * (NORMAL_LENGTH * 0.5))
                    .with_rotation(rotation_from_up(normal))
                    .with_scale(Vec3::new(NORMAL_THICKNESS, NORMAL_LENGTH, NORMAL_THICKNESS)),
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.contact_material,
                tint(colour, 0.9),
                used,
            );
        }
    }

    /// Draws the last line-of-sight check a targeted ability ran.
    ///
    /// The recorded query, not a fresh one: the point of the picture is to explain a
    /// refusal that already happened, and re-asking would answer about the world as it is
    /// now rather than as it was when the ability was refused.
    ///
    /// ```text
    /// caster o=================o target      clear
    /// caster o======x  | wall |   target     blocked, x is where it stopped
    /// ```
    fn draw_line_of_sight(
        &mut self,
        render: &mut RenderWorld,
        simulation: &Simulation,
        used: &mut usize,
    ) {
        let Some(record) = simulation.last_line_of_sight() else {
            return;
        };
        let Some(check) = record.check else {
            return;
        };
        let colour = if check.visible {
            LOS_CLEAR_TINT
        } else {
            LOS_BLOCKED_TINT
        };
        let style = tint(colour, 0.9);

        // The ray itself, drawn only as far as it actually got: a bar that runs through the
        // wall to the target would be a picture of the answer the check did not give.
        let end = check.blocked_at.unwrap_or(check.to);
        let along = end - check.from;
        let length = along.length();
        if length > 1e-3 {
            self.push(
                render,
                Transform::from_translation((check.from + end) * 0.5)
                    .with_rotation(rotation_from_up(along))
                    .with_scale(Vec3::new(LOS_THICKNESS, length, LOS_THICKNESS)),
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.contact_material,
                style,
                used,
            );
        }

        // Both combat points, so it is obvious the ray leaves a chest rather than a foot,
        // and the point at which it stopped.
        for point in [Some(check.from), Some(check.to), check.blocked_at]
            .into_iter()
            .flatten()
        {
            self.push(
                render,
                Transform::from_translation(point).with_scale(Vec3::splat(LOS_MARKER_SIZE)),
                self.bindings.sphere_mesh,
                self.bindings.sphere_bounds,
                self.bindings.contact_material,
                style,
                used,
            );
        }

        // The blocking shape itself, lit up among the ordinary blue boxes.
        if let Some(id) = check.blocker
            && let Some(shape) = simulation.collision_world().shape(id)
        {
            let transform = match shape {
                Shape::Plane(_) => return,
                Shape::Aabb(bounds) => Transform::from_translation(bounds.center())
                    .with_scale(bounds.half_extents() * 2.0),
                Shape::Obb(obb) => Transform::from_translation(obb.center)
                    .with_rotation(obb.rotation)
                    .with_scale(obb.half_extents * 2.0),
                // The terrain is already the picture: it is drawn as itself, out of the same
                // heights this shape was built from, so a highlighted box around the whole
                // level would say nothing except that a terrain exists.
                Shape::Heightfield(_) => return,
            };
            self.push(
                render,
                transform,
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.shape_material,
                tint(LOS_BLOCKED_TINT, 0.35),
                used,
            );
        }
    }

    /// Draws the camera's last obstruction check.
    ///
    /// ```text
    /// anchor o--pivot o==========o eye   x hit -> normal    . desired eye
    /// ```
    ///
    /// The sweep is drawn from the pivot to the *desired* eye, so a blocked sweep visibly
    /// runs into the wall that stopped it; the hit and its normal are drawn where it did.
    /// The marker at the actual eye sits inside the near plane while playing and so only
    /// shows from a free camera, which is where it is useful.
    fn draw_camera(&mut self, render: &mut RenderWorld, used: &mut usize) {
        let Some(probe) = self.camera else {
            return;
        };
        let colour = if probe.obstructed() {
            CAMERA_BLOCKED_TINT
        } else {
            CAMERA_CLEAR_TINT
        };
        let style = tint(colour, 0.9);

        for (from, to) in [
            (probe.anchor, probe.pivot),
            (probe.pivot, probe.desired_eye),
        ] {
            let along = to - from;
            let length = along.length();
            if length > 1e-3 {
                self.push(
                    render,
                    Transform::from_translation((from + to) * 0.5)
                        .with_rotation(rotation_from_up(along))
                        .with_scale(Vec3::new(
                            CAMERA_SWEEP_THICKNESS,
                            length,
                            CAMERA_SWEEP_THICKNESS,
                        )),
                    self.bindings.box_mesh,
                    self.bindings.box_bounds,
                    self.bindings.contact_material,
                    style,
                    used,
                );
            }
        }
        for (point, mesh, bounds) in [
            (
                probe.anchor,
                self.bindings.box_mesh,
                self.bindings.box_bounds,
            ),
            (
                probe.pivot,
                self.bindings.sphere_mesh,
                self.bindings.sphere_bounds,
            ),
            (
                probe.desired_eye,
                self.bindings.box_mesh,
                self.bindings.box_bounds,
            ),
            (
                probe.eye,
                self.bindings.sphere_mesh,
                self.bindings.sphere_bounds,
            ),
        ] {
            self.push(
                render,
                Transform::from_translation(point).with_scale(Vec3::splat(CAMERA_MARKER_SIZE)),
                mesh,
                bounds,
                self.bindings.contact_material,
                style,
                used,
            );
        }
        if let Some(hit) = probe.hit {
            let blocked = tint(CAMERA_BLOCKED_TINT, 0.9);
            self.push(
                render,
                Transform::from_translation(hit.point).with_scale(Vec3::splat(CONTACT_SIZE)),
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.contact_material,
                blocked,
                used,
            );
            self.push(
                render,
                Transform::from_translation(hit.point + hit.normal * (NORMAL_LENGTH * 0.5))
                    .with_rotation(rotation_from_up(hit.normal))
                    .with_scale(Vec3::new(NORMAL_THICKNESS, NORMAL_LENGTH, NORMAL_THICKNESS)),
                self.bindings.box_mesh,
                self.bindings.box_bounds,
                self.bindings.contact_material,
                blocked,
                used,
            );
        }
    }

    /// Writes one drawn thing into the pool, spawning a slot only if the pool is short.
    #[allow(
        clippy::too_many_arguments,
        reason = "a draw call's parameters; grouping them into a struct would be the same \
                  arguments with a name in front"
    )]
    fn push(
        &mut self,
        render: &mut RenderWorld,
        transform: Transform,
        mesh: MeshHandle,
        bounds: Sphere,
        material: MaterialHandle,
        style: InstanceStyle,
        used: &mut usize,
    ) {
        if *used >= MAX_DEBUG_ENTITIES {
            return;
        }
        match self.entities.get(*used).copied() {
            Some(entity) => {
                render.set_transform(entity, transform, bounds);
                render.set_material(entity, material);
                render.set_style(entity, style);
                render.set_visible(entity, true);
            }
            None => {
                let entity = render.spawn(RenderEntityDesc {
                    transform,
                    mesh,
                    material,
                    bounds,
                    visible: true,
                    style,
                    // Developer geometry drawn over the world. Shadowing it would be
                    // shadowing the debug view rather than the game, and casting from it
                    // would put the collision hull's shadow on the floor beside the object's.
                    shadow: engine_render_data::ShadowCasting::NONE,
                });
                self.entities.push(entity);
            }
        }
        *used += 1;
    }
}

/// A colour and a glow, as an instance style.
fn tint(colour: [f32; 3], emissive: f32) -> InstanceStyle {
    InstanceStyle::tinted(colour[0], colour[1], colour[2]).with_emissive(emissive)
}

/// The rotation that takes `+Y` to `direction`.
///
/// Used to point a stretched box along a contact normal. A normal that is already `+Y`, or
/// exactly `-Y`, has no unique axis to rotate about, so those two are answered directly
/// rather than by normalizing a zero-length cross product.
fn rotation_from_up(direction: Vec3) -> Quat {
    let length = direction.length();
    if length <= 1e-6 {
        return Quat::IDENTITY;
    }
    let unit = direction / length;
    let dot = unit.dot(Vec3::Y).clamp(-1.0, 1.0);
    if dot > 1.0 - 1e-6 {
        return Quat::IDENTITY;
    }
    if dot < -1.0 + 1e-6 {
        return Quat::from_axis_angle(Vec3::X, std::f32::consts::PI);
    }
    Quat::from_axis_angle(Vec3::Y.cross(unit), dot.acos())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::demo;

    fn bindings() -> DebugBindings {
        let unit = Sphere::new(Vec3::ZERO, 0.87);
        DebugBindings {
            box_mesh: MeshHandle::new(0, 1),
            box_bounds: unit,
            sphere_mesh: MeshHandle::new(1, 1),
            sphere_bounds: Sphere::new(Vec3::ZERO, 1.0),
            shape_material: MaterialHandle::new(0, 1),
            capsule_material: MaterialHandle::new(1, 1),
            contact_material: MaterialHandle::new(2, 1),
        }
    }

    #[test]
    fn a_disabled_view_draws_nothing_and_owns_nothing() {
        let (simulation, _) = demo::collision_demo();
        let mut render = RenderWorld::new();
        let mut debug = CollisionDebug::new(bindings());

        debug.rebuild(&mut render, &simulation);
        assert!(!debug.enabled());
        assert_eq!(render.len(), 0, "an off view is not a hidden view");
        assert_eq!(debug.drawn(), 0);
    }

    #[test]
    fn enabling_it_draws_the_geometry_and_the_capsule() {
        let (mut simulation, _) = demo::collision_demo();
        simulation.step_many(10);
        let mut render = RenderWorld::new();
        let mut debug = CollisionDebug::new(bindings());

        debug.set_enabled(&mut render, true);
        debug.rebuild(&mut render, &simulation);

        // Every finite shape, plus three primitives for the capsule, plus two per contact.
        let shapes = simulation
            .collision_world()
            .statics()
            .filter(|id| simulation.collision_world().bounds(*id).is_some())
            .count();
        assert!(debug.drawn() >= shapes + 3, "{} drawn", debug.drawn());
        assert_eq!(render.len(), debug.drawn());
    }

    #[test]
    fn a_character_standing_on_the_floor_has_a_contact_drawn_under_it() {
        let (mut simulation, _) = demo::collision_demo();
        simulation.step_many(30);
        let mut render = RenderWorld::new();
        let mut debug = CollisionDebug::new(bindings());
        debug.set_enabled(&mut render, true);
        debug.rebuild(&mut render, &simulation);

        assert!(
            simulation
                .player_body()
                .expect("the player exists")
                .grounded
        );
        assert!(debug.contacts() >= 1, "nothing was reported under its feet");
    }

    #[test]
    fn turning_it_off_gives_the_render_world_back() {
        let (mut simulation, _) = demo::collision_demo();
        simulation.step_many(10);
        let mut render = RenderWorld::new();
        let mut debug = CollisionDebug::new(bindings());

        debug.set_enabled(&mut render, true);
        debug.rebuild(&mut render, &simulation);
        assert!(!render.is_empty());

        debug.set_enabled(&mut render, false);
        assert_eq!(render.len(), 0, "the entities were hidden, not released");
        assert_eq!(debug.drawn(), 0);
    }

    #[test]
    fn a_steady_scene_stops_spawning_render_entities() {
        let (mut simulation, _) = demo::collision_demo();
        let mut render = RenderWorld::new();
        let mut debug = CollisionDebug::new(bindings());
        debug.set_enabled(&mut render, true);
        for _ in 0..10 {
            simulation.step_once();
            debug.rebuild(&mut render, &simulation);
        }
        let slots = render.slot_count();

        for _ in 0..300 {
            simulation.step_once();
            debug.rebuild(&mut render, &simulation);
        }
        assert_eq!(render.slot_count(), slots, "the debug view churned slots");
    }

    #[test]
    fn a_camera_probe_adds_its_sweep_and_hit_to_the_view() {
        let (mut simulation, _) = demo::collision_demo();
        simulation.step_many(10);
        let mut render = RenderWorld::new();
        let mut debug = CollisionDebug::new(bindings());
        debug.set_enabled(&mut render, true);
        debug.rebuild(&mut render, &simulation);
        let without = debug.drawn();

        let mut camera = crate::camera::ThirdPersonCamera::following(simulation.player_position());
        camera.avoid_obstructions(
            simulation.collision_world(),
            simulation.player_position(),
            0.0,
        );
        let probe = camera.probe().copied().expect("the check ran");
        debug.set_camera_probe(Some(probe));
        debug.rebuild(&mut render, &simulation);
        let lines = 2;
        let markers = 4;
        let hit = if probe.obstructed() { 2 } else { 0 };
        assert!(
            debug.drawn() > without + markers + hit
                && debug.drawn() <= without + lines + markers + hit,
            "{} drawn, {without} without the camera",
            debug.drawn()
        );

        debug.set_camera_probe(None);
        debug.rebuild(&mut render, &simulation);
        assert_eq!(debug.drawn(), without);
    }

    #[test]
    fn a_normal_pointing_up_needs_no_rotation_and_one_pointing_down_needs_half_a_turn() {
        assert_eq!(rotation_from_up(Vec3::Y), Quat::IDENTITY);
        assert_eq!(rotation_from_up(Vec3::ZERO), Quat::IDENTITY);

        let flipped = rotation_from_up(-Vec3::Y).rotate(Vec3::Y);
        assert!((flipped + Vec3::Y).length() < 1e-5, "{flipped:?}");

        let sideways = rotation_from_up(Vec3::X).rotate(Vec3::Y);
        assert!((sideways - Vec3::X).length() < 1e-5, "{sideways:?}");
    }
}
