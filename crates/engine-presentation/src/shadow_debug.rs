//! Drawing the bounds the shadow pass culls casters with.
//!
//! ```text
//! RenderWorld                          RenderWorld
//! -----------                          -----------
//! bounds[slot] x transform[slot]  --->  one translucent sphere per caster
//! shadow[slot].casts == false     --->  nothing
//! ```
//!
//! The renderer's three shadow debug views (F7) answer "what does the shadow look like".
//! This one answers the question underneath it: **what does the shadow pass think is there**.
//! When a caster is missing from a cascade, the two possible causes are that it was culled
//! and that it was drawn somewhere unexpected, and a sphere drawn at the bounds the culler
//! actually tested is what tells them apart — a sphere in the wrong place, or one far too
//! large, is a bug in extraction rather than in the shadow pass.
//!
//! It draws **the same numbers the culler reads**, out of the same arrays, rather than
//! recomputing bounds from the mesh. A debug view built from a second description of the
//! scene lies exactly when it is needed.
//!
//! Like [`crate::collision_debug`], it costs nothing when off: its entities are despawned,
//! not hidden, so the culler does not walk them.

use engine_core::math::{Sphere, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{
    InstanceStyle, MaterialHandle, MeshHandle, RenderEntity, RenderEntityDesc, RenderWorld,
    ShadowCasting, world_bounds,
};

use crate::collision_debug::{DebugBindings, MAX_DEBUG_ENTITIES};

/// Colour of a caster's bounding sphere.
///
/// Yellow, because the collision view owns blue and green and the two are routinely on at
/// the same time — a developer chasing a missing shadow through a wall is looking at both.
const CASTER_TINT: [f32; 3] = [1.00, 0.85, 0.25];

/// Colour of a bound that receives shadows but casts none.
const RECEIVER_TINT: [f32; 3] = [0.45, 0.55, 0.70];

/// How strongly a drawn bound glows.
///
/// Emissive rather than lit: the point of the sphere is its outline, and a sphere shaded by
/// the sun is dark on exactly the side the sun is casting from.
const GLOW: f32 = 0.35;

/// The shadow caster bounds, drawn.
#[derive(Debug)]
pub struct ShadowCasterDebug {
    bindings: DebugBindings,
    /// The render entities this view owns. Empty while it is disabled.
    entities: Vec<RenderEntity>,
    /// Bounds collected from the world before anything is spawned into it.
    ///
    /// A scratch buffer rather than a local, because it is filled every frame the view is on
    /// and this module is not allowed to allocate per frame any more than the rest of
    /// presentation is. It exists at all because spawning into the world while walking it is
    /// a borrow conflict *and* would make the view draw its own spheres.
    scratch: Vec<(Sphere, bool)>,
    enabled: bool,
    /// How many entities the last rebuild used.
    drawn: usize,
    /// How many casters the last rebuild found, before the entity cap.
    casters: usize,
}

impl ShadowCasterDebug {
    /// A disabled view.
    #[must_use]
    pub fn new(bindings: DebugBindings) -> Self {
        Self {
            bindings,
            entities: Vec::with_capacity(64),
            scratch: Vec::with_capacity(256),
            enabled: false,
            drawn: 0,
            casters: 0,
        }
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

    /// Casters the last rebuild found, whether or not it had room to draw them all.
    #[must_use]
    pub fn casters(&self) -> usize {
        self.casters
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
            self.scratch.clear();
            self.drawn = 0;
            self.casters = 0;
        }
    }

    /// Rebuilds the drawn spheres from the render world's own bounds.
    ///
    /// Called once a frame while the view is on. Entities are reused across frames, so the
    /// per-frame cost is writing transforms rather than spawning.
    pub fn rebuild(&mut self, world: &mut RenderWorld) {
        if !self.enabled {
            return;
        }

        self.scratch.clear();
        let transforms = world.transforms();
        let bounds = world.bounds();
        let shadow = world.shadow_casting();
        for slot in 0..world.slot_count() {
            if !world.is_drawable(slot) {
                continue;
            }
            let casting = shadow[slot];
            if !casting.casts && !casting.receives {
                continue;
            }
            self.scratch
                .push((world_bounds(bounds[slot], &transforms[slot]), casting.casts));
        }
        self.casters = self.scratch.iter().filter(|(_, casts)| *casts).count();

        let mut used = 0;
        for index in 0..self.scratch.len() {
            let (sphere, casts) = self.scratch[index];
            // A receiver's bound is drawn thinner than a caster's: it is context for where
            // the caster is, not the thing being looked at.
            let (colour, scale) = if casts {
                (CASTER_TINT, 1.0)
            } else {
                (RECEIVER_TINT, 0.35)
            };
            let transform = Transform::from_translation(sphere.center)
                .with_scale(Vec3::splat(sphere.radius.max(0.01) * scale));
            self.push(
                world,
                transform,
                // The sphere the culler tests is a sphere, so a sphere is what is drawn. A
                // box here would be a different shape from the one being debugged.
                self.bindings.sphere_mesh,
                self.bindings.sphere_bounds,
                self.bindings.shape_material,
                InstanceStyle::tinted(colour[0], colour[1], colour[2]).with_emissive(GLOW),
                &mut used,
            );
        }

        for entity in self.entities.iter().skip(used) {
            world.set_visible(*entity, false);
        }
        self.drawn = used;
    }

    /// Reuses or spawns one debug entity.
    #[allow(
        clippy::too_many_arguments,
        reason = "the same shape a drawn debug entity has in collision_debug::push"
    )]
    fn push(
        &mut self,
        world: &mut RenderWorld,
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
                world.set_transform(entity, transform, bounds);
                world.set_material(entity, material);
                world.set_style(entity, style);
                world.set_visible(entity, true);
            }
            None => {
                let entity = world.spawn(RenderEntityDesc {
                    transform,
                    mesh,
                    material,
                    bounds,
                    visible: true,
                    style,
                    // Never a caster and never a receiver. A debug view of what casts
                    // shadows that itself cast shadows would change the answer it is drawn
                    // to report, and every sphere would be drawn again in every cascade.
                    shadow: ShadowCasting::NONE,
                });
                self.entities.push(entity);
            }
        }
        *used += 1;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn bindings() -> DebugBindings {
        let unit = Sphere::new(Vec3::ZERO, 1.0);
        DebugBindings {
            box_mesh: MeshHandle::new(0, 1),
            box_bounds: unit,
            sphere_mesh: MeshHandle::new(1, 1),
            sphere_bounds: unit,
            shape_material: MaterialHandle::new(0, 1),
            capsule_material: MaterialHandle::new(1, 1),
            contact_material: MaterialHandle::new(2, 1),
        }
    }

    /// A world with one caster, one receiver-only object and one thing that is neither.
    fn world() -> RenderWorld {
        let mut world = RenderWorld::new();
        for (position, shadow) in [
            (Vec3::new(0.0, 1.0, 0.0), ShadowCasting::CASTS_AND_RECEIVES),
            (Vec3::new(4.0, 0.0, 0.0), ShadowCasting::RECEIVES_ONLY),
            (Vec3::new(8.0, 0.0, 0.0), ShadowCasting::NONE),
        ] {
            world.spawn(RenderEntityDesc {
                transform: Transform::from_translation(position),
                mesh: MeshHandle::new(0, 1),
                material: MaterialHandle::new(0, 1),
                bounds: Sphere::new(Vec3::ZERO, 1.5),
                visible: true,
                style: InstanceStyle::NEUTRAL,
                shadow,
            });
        }
        world
    }

    #[test]
    fn a_disabled_view_puts_nothing_in_the_world() {
        let mut world = world();
        let before = world.len();
        let mut debug = ShadowCasterDebug::new(bindings());
        debug.rebuild(&mut world);
        assert_eq!(world.len(), before, "a disabled view spawned something");
        assert_eq!(debug.drawn(), 0);
    }

    #[test]
    fn it_draws_the_casters_and_the_receivers_and_nothing_else() {
        let mut world = world();
        let before = world.len();
        let mut debug = ShadowCasterDebug::new(bindings());
        debug.set_enabled(&mut world, true);
        debug.rebuild(&mut world);

        // Two of the three take part in shadows; the third is excluded, and so are the
        // spheres the view just spawned — which is the property that stops it growing by
        // one sphere per frame forever.
        assert_eq!(debug.drawn(), 2);
        assert_eq!(debug.casters(), 1);
        assert_eq!(world.len(), before + 2);

        debug.rebuild(&mut world);
        assert_eq!(
            debug.drawn(),
            2,
            "a second frame drew the debug spheres too"
        );
        assert_eq!(
            world.len(),
            before + 2,
            "entities are reused, not respawned"
        );
    }

    #[test]
    fn a_sphere_is_drawn_where_the_culler_looks() {
        // The bound the shadow culler tests is the local sphere put through the transform,
        // so the drawn sphere has to be at the *world* centre and not at the local one.
        let mut world = RenderWorld::new();
        world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(Vec3::new(10.0, 2.0, -6.0))
                .with_scale(Vec3::splat(2.0)),
            mesh: MeshHandle::new(0, 1),
            material: MaterialHandle::new(0, 1),
            bounds: Sphere::new(Vec3::new(0.0, 1.0, 0.0), 1.5),
            visible: true,
            style: InstanceStyle::NEUTRAL,
            shadow: ShadowCasting::CASTS_AND_RECEIVES,
        });
        let expected = world_bounds(world.bounds()[0], &world.transforms()[0]);

        let mut debug = ShadowCasterDebug::new(bindings());
        debug.set_enabled(&mut world, true);
        debug.rebuild(&mut world);

        let drawn = world.transforms()[1];
        assert!(
            (drawn.translation - expected.center).length() < 1e-4,
            "{drawn:?}"
        );
        assert!((drawn.scale.x - expected.radius).abs() < 1e-4, "{drawn:?}");
    }

    #[test]
    fn turning_it_off_leaves_the_world_as_it_was() {
        let mut world = world();
        let before = world.len();
        let mut debug = ShadowCasterDebug::new(bindings());
        debug.set_enabled(&mut world, true);
        debug.rebuild(&mut world);
        debug.set_enabled(&mut world, false);
        assert_eq!(world.len(), before);
        assert_eq!(debug.drawn(), 0);
    }
}
