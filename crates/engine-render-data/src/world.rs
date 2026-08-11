//! The drawable world: what the game hands the renderer each frame.
//!
//! [`RenderWorld`] is the boundary between simulation and rendering. The game fills it with
//! transforms, mesh handles and material handles; the renderer reads it and records draws.
//! It contains no gameplay concept — a [`RenderEntity`] is a slot in five arrays, not a
//! character — and no rendering concept either: nothing here knows what a pipeline state or
//! a vertex buffer is.

use engine_core::math::Sphere;
use engine_core::transform::Transform;

/// An index and a generation, packed into eight bytes.
///
/// A stale handle is *detected* rather than dereferenced: the generation in the handle is
/// compared against the generation in the slot, and a freed-and-reused slot fails the
/// comparison instead of silently addressing someone else's object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    /// `pub(crate)` rather than private so [`declare_handle`] can be invoked from another
    /// module in this crate. Still not public: outside the crate a handle stays opaque.
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

impl Handle {
    /// The slot this handle refers to.
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation this handle was issued with.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Declares a distinct handle type, so one kind cannot be passed where another belongs.
///
/// `pub(crate)`-exported below so `skinning.rs` can declare its own handle with the same
/// generation semantics rather than writing a second, subtly different one.
macro_rules! declare_handle {
    ($name:ident, $what:literal) => {
        #[doc = concat!("A handle to ", $what, ".")]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(Handle);

        impl $name {
            /// A handle that names nothing.
            ///
            /// Generation 0 never matches a live slot, so this cannot be mistaken for a
            /// real handle. It exists for a world built only to measure culling, where the
            /// bounds matter and the geometry does not; `Renderer::render_world` rejects it.
            pub const INVALID: Self = Self(Handle {
                index: u32::MAX,
                generation: 0,
            });

            /// Wraps a raw index and generation.
            ///
            /// Public because the renderer issues these handles and lives in another crate
            /// now. Callers outside the renderer should treat handles as opaque: fabricating
            /// one names a slot that may not exist.
            #[must_use]
            pub fn new(index: u32, generation: u32) -> Self {
                Self(Handle { index, generation })
            }

            /// The underlying index and generation.
            #[must_use]
            pub fn raw(self) -> Handle {
                self.0
            }

            /// The slot this handle refers to.
            #[must_use]
            pub fn index(self) -> usize {
                self.0.index as usize
            }
        }
    };
}

pub(crate) use declare_handle;

declare_handle!(MeshHandle, "an uploaded mesh");
declare_handle!(MaterialHandle, "an uploaded material");
declare_handle!(RenderEntity, "one drawable in a `RenderWorld`");

/// Per-entity visual state that changes without touching the entity.
///
/// A material describes a *surface* — what a dummy is made of — and is shared by every
/// dummy in the arena. This describes what is happening to **one** of them right now: it is
/// selected, it was just hit, it is nearly dead. Phase 3 had nowhere to put that, so the
/// extractor expressed "this one died" by despawning the render entity and respawning it
/// with a different material handle, which churned a slot for a colour change.
///
/// Deliberately small: five floats, sixteen of a 96-byte instance record. It is not a
/// second material system and must not become one — no textures, no shader selection, no
/// parameters the lighting model does not already read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceStyle {
    /// Multiplied with the material's base colour. `[1, 1, 1, 1]` changes nothing.
    ///
    /// A multiply rather than a replacement, so a tint darkens or warms the surface the
    /// artist authored instead of discarding it.
    pub tint: [f32; 4],
    /// Added to the material's emissive term. Zero changes nothing.
    ///
    /// What a hit flash and a selection glow are made of: a surface that briefly ignores
    /// the lighting, without a second material or a second draw.
    pub emissive: f32,
}

impl Default for InstanceStyle {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

impl InstanceStyle {
    /// The style that changes nothing about how a material looks.
    pub const NEUTRAL: Self = Self {
        tint: [1.0, 1.0, 1.0, 1.0],
        emissive: 0.0,
    };

    /// A style that multiplies the surface by `tint`.
    #[must_use]
    pub fn tinted(red: f32, green: f32, blue: f32) -> Self {
        Self {
            tint: [red, green, blue, 1.0],
            ..Self::NEUTRAL
        }
    }

    /// The same style with an emissive term.
    #[must_use]
    pub fn with_emissive(self, emissive: f32) -> Self {
        Self {
            emissive: emissive.max(0.0),
            ..self
        }
    }

    /// Whether this style leaves the material untouched.
    ///
    /// Lets a caller count how many entities are actually styled, which is what makes "the
    /// tint is free when nothing is tinted" checkable rather than assumed.
    #[must_use]
    pub fn is_neutral(self) -> bool {
        self == Self::NEUTRAL
    }
}

/// What a `RenderWorld` slot holds, in parallel arrays.
///
/// Structure-of-arrays because culling reads `bounds` and `visible` and nothing else:
/// streaming mesh handles and materials it does not look at would waste the cache line it
/// is reading through.
#[derive(Debug, Default, Clone)]
pub struct RenderWorld {
    transforms: Vec<Transform>,
    meshes: Vec<MeshHandle>,
    materials: Vec<MaterialHandle>,
    /// Per-entity visual state. Written every frame by extraction; costs no slot churn.
    styles: Vec<InstanceStyle>,
    /// World-space bounding spheres, refreshed whenever a transform changes.
    bounds: Vec<Sphere>,
    visible: Vec<bool>,
    generations: Vec<u32>,
    /// Slots whose entity was despawned, most recent first.
    free_slots: Vec<u32>,
    live: usize,
    /// This frame's skinned draws. Rebuilt every frame rather than slot-managed — see
    /// [`crate::skinning`] for why the two kinds of drawable are stored differently.
    skinned: crate::skinning::SkinnedDrawList,
}

/// What a caller must supply to add something drawable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderEntityDesc {
    /// Where it is.
    pub transform: Transform,
    /// What to draw.
    pub mesh: MeshHandle,
    /// How it looks.
    pub material: MaterialHandle,
    /// The mesh's local-space bounds, which the caller reads from the mesh.
    pub bounds: Sphere,
    /// Whether it should be drawn at all.
    pub visible: bool,
    /// Per-entity visual state. [`InstanceStyle::NEUTRAL`] for an ordinary object.
    pub style: InstanceStyle,
}

impl Default for RenderEntityDesc {
    fn default() -> Self {
        Self {
            transform: Transform::IDENTITY,
            mesh: MeshHandle::INVALID,
            material: MaterialHandle::INVALID,
            bounds: Sphere::new(engine_core::math::Vec3::ZERO, 1.0),
            visible: true,
            style: InstanceStyle::NEUTRAL,
        }
    }
}

impl RenderWorld {
    /// An empty world.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty world with room for `capacity` entities.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            transforms: Vec::with_capacity(capacity),
            meshes: Vec::with_capacity(capacity),
            materials: Vec::with_capacity(capacity),
            styles: Vec::with_capacity(capacity),
            bounds: Vec::with_capacity(capacity),
            visible: Vec::with_capacity(capacity),
            generations: Vec::with_capacity(capacity),
            free_slots: Vec::new(),
            live: 0,
            skinned: crate::skinning::SkinnedDrawList::new(),
        }
    }

    /// Adds an entity, reusing a freed slot when one is available.
    pub fn spawn(&mut self, desc: RenderEntityDesc) -> RenderEntity {
        self.live += 1;
        let world_bounds = world_bounds(desc.bounds, &desc.transform);

        if let Some(index) = self.free_slots.pop() {
            let slot = index as usize;
            self.transforms[slot] = desc.transform;
            self.meshes[slot] = desc.mesh;
            self.materials[slot] = desc.material;
            self.styles[slot] = desc.style;
            self.bounds[slot] = world_bounds;
            self.visible[slot] = desc.visible;
            return RenderEntity::new(index, self.generations[slot]);
        }

        let index = self.transforms.len() as u32;
        self.transforms.push(desc.transform);
        self.meshes.push(desc.mesh);
        self.materials.push(desc.material);
        self.styles.push(desc.style);
        self.bounds.push(world_bounds);
        self.visible.push(desc.visible);
        // Generation 1, not 0: a default-constructed handle has generation 0 and must not
        // accidentally address a live slot.
        self.generations.push(1);
        RenderEntity::new(index, 1)
    }

    /// Removes an entity. A stale handle is ignored and reported.
    pub fn despawn(&mut self, entity: RenderEntity) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let slot = entity.index();
        // Bumping the generation is what makes every outstanding handle to this slot stale.
        self.generations[slot] = self.generations[slot].wrapping_add(1).max(1);
        self.visible[slot] = false;
        self.free_slots.push(entity.raw().index());
        self.live -= 1;
        true
    }

    /// Whether the handle still refers to a live entity.
    #[must_use]
    pub fn contains(&self, entity: RenderEntity) -> bool {
        let slot = entity.index();
        slot < self.generations.len()
            && self.generations[slot] == entity.raw().generation()
            && !self.free_slots.contains(&entity.raw().index())
    }

    /// Moves an entity, refreshing its world-space bounds.
    ///
    /// Returns `false` for a stale handle rather than panicking: a game that despawns and
    /// then updates in the same frame is doing something ordinary.
    pub fn set_transform(
        &mut self,
        entity: RenderEntity,
        transform: Transform,
        local: Sphere,
    ) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let slot = entity.index();
        self.bounds[slot] = world_bounds(local, &transform);
        self.transforms[slot] = transform;
        true
    }

    /// Replaces an entity's per-entity visual state.
    ///
    /// The point of the whole mechanism: a dummy that is hit, selected or nearly dead keeps
    /// its render entity, its slot and its place in the culler's ordering. Nothing is
    /// despawned and nothing is respawned.
    pub fn set_style(&mut self, entity: RenderEntity, style: InstanceStyle) -> bool {
        if !self.contains(entity) {
            return false;
        }
        self.styles[entity.index()] = style;
        true
    }

    /// Replaces an entity's material.
    ///
    /// Also no longer a respawn. A material change does move the entity into a different
    /// batch, which is a per-frame grouping decision and costs nothing across frames — the
    /// slot, the handle and every outstanding reference stay valid.
    pub fn set_material(&mut self, entity: RenderEntity, material: MaterialHandle) -> bool {
        if !self.contains(entity) {
            return false;
        }
        self.materials[entity.index()] = material;
        true
    }

    /// Shows or hides an entity.
    pub fn set_visible(&mut self, entity: RenderEntity, visible: bool) -> bool {
        if !self.contains(entity) {
            return false;
        }
        self.visible[entity.index()] = visible;
        true
    }

    /// Live entities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether the world holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Slots ever allocated, live or free. The length of the parallel arrays.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.transforms.len()
    }

    /// The transforms, indexed by slot.
    #[must_use]
    pub fn transforms(&self) -> &[Transform] {
        &self.transforms
    }

    /// The mesh handles, indexed by slot.
    #[must_use]
    pub fn meshes(&self) -> &[MeshHandle] {
        &self.meshes
    }

    /// The material handles, indexed by slot.
    #[must_use]
    pub fn materials(&self) -> &[MaterialHandle] {
        &self.materials
    }

    /// The per-entity visual states, indexed by slot.
    #[must_use]
    pub fn styles(&self) -> &[InstanceStyle] {
        &self.styles
    }

    /// The world-space bounding spheres, indexed by slot.
    #[must_use]
    pub fn bounds(&self) -> &[Sphere] {
        &self.bounds
    }

    /// Per-slot visibility flags.
    #[must_use]
    pub fn visible_flags(&self) -> &[bool] {
        &self.visible
    }

    /// The handle addressing slot `slot`.
    ///
    /// The inverse of [`RenderEntity::index`]: a consumer walking the parallel arrays by
    /// slot — the culler does — needs to name the entity it found without the caller
    /// having kept a handle for every one. Rebuilding it from the slot's live generation is
    /// what makes the returned handle valid rather than merely plausible.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is past the end. Slots come from iterating the world's own arrays,
    /// so an out-of-range one is an engine bug rather than bad input.
    #[must_use]
    pub fn entity_at(&self, slot: usize) -> RenderEntity {
        RenderEntity::new(slot as u32, self.generations[slot])
    }

    /// Whether a slot holds something that should be drawn.
    ///
    /// A freed slot keeps its arrays populated — clearing them would be work for data
    /// nobody reads — so it is `visible = false` that makes it invisible.
    #[must_use]
    pub fn is_drawable(&self, slot: usize) -> bool {
        self.visible.get(slot).copied().unwrap_or(false)
    }

    /// This frame's skinned draws.
    #[must_use]
    pub fn skinned(&self) -> &crate::skinning::SkinnedDrawList {
        &self.skinned
    }

    /// This frame's skinned draws, for the extractor to refill.
    #[must_use]
    pub fn skinned_mut(&mut self) -> &mut crate::skinning::SkinnedDrawList {
        &mut self.skinned
    }

    /// Removes every entity, keeping the allocated capacity.
    pub fn clear(&mut self) {
        self.skinned.clear();
        self.transforms.clear();
        self.meshes.clear();
        self.materials.clear();
        self.styles.clear();
        self.bounds.clear();
        self.visible.clear();
        self.generations.clear();
        self.free_slots.clear();
        self.live = 0;
    }
}

/// Places a local-space bounding sphere in world space.
///
/// The radius is scaled by the *largest* axis: a non-uniformly scaled sphere is an
/// ellipsoid, and keeping it a sphere means over-estimating. Over-estimating draws something
/// that could have been culled; under-estimating culls something that is on screen.
#[must_use]
pub fn world_bounds(local: Sphere, transform: &Transform) -> Sphere {
    Sphere::new(
        transform.transform_point(local.center),
        local.radius * transform.max_scale(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::math::{Quat, Vec3};

    fn desc(mesh: u32, material: u32) -> RenderEntityDesc {
        RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh: MeshHandle::new(mesh, 1),
            material: MaterialHandle::new(material, 1),
            bounds: Sphere::new(Vec3::ZERO, 1.0),
            visible: true,
            style: InstanceStyle::NEUTRAL,
        }
    }

    #[test]
    fn a_spawned_entity_is_live_and_drawable() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.contains(entity));
        assert!(world.is_drawable(entity.index()));
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn a_despawned_entity_stops_being_drawn() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.despawn(entity));
        assert!(!world.contains(entity));
        assert!(!world.is_drawable(entity.index()));
        assert!(world.is_empty());
    }

    #[test]
    fn a_stale_handle_is_detected_rather_than_dereferenced() {
        // The whole reason handles carry a generation: without it, the second spawn's slot
        // would answer to the first entity's handle.
        let mut world = RenderWorld::new();
        let first = world.spawn(desc(0, 0));
        world.despawn(first);
        let second = world.spawn(desc(1, 1));

        assert_eq!(first.index(), second.index(), "the slot must be reused");
        assert!(!world.contains(first));
        assert!(world.contains(second));
        assert!(!world.set_visible(first, true));
    }

    #[test]
    fn despawning_twice_is_reported_not_ignored() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.despawn(entity));
        assert!(!world.despawn(entity));
        assert_eq!(world.len(), 0);
    }

    #[test]
    fn slots_are_reused_rather_than_growing() {
        let mut world = RenderWorld::new();
        let handles: Vec<_> = (0..4).map(|i| world.spawn(desc(i, i))).collect();
        for handle in &handles {
            world.despawn(*handle);
        }
        for i in 0..4 {
            world.spawn(desc(i, i));
        }
        assert_eq!(world.slot_count(), 4, "freed slots must be reused");
        assert_eq!(world.len(), 4);
    }

    #[test]
    fn bounds_follow_the_transform_into_world_space() {
        let mut world = RenderWorld::new();
        let local = Sphere::new(Vec3::ZERO, 1.0);
        let entity = world.spawn(RenderEntityDesc {
            transform: Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
            bounds: local,
            ..desc(0, 0)
        });
        assert_eq!(world.bounds()[entity.index()].center.x, 10.0);

        let moved = Transform::from_translation(Vec3::new(0.0, 5.0, 0.0))
            .with_scale(Vec3::new(2.0, 3.0, 1.0));
        assert!(world.set_transform(entity, moved, local));

        let bounds = world.bounds()[entity.index()];
        assert_eq!(bounds.center, Vec3::new(0.0, 5.0, 0.0));
        // The largest axis, because a sphere scaled non-uniformly is an ellipsoid and
        // under-estimating would cull something that is on screen.
        assert_eq!(bounds.radius, 3.0);
    }

    #[test]
    fn a_rotated_offset_bound_ends_up_where_the_geometry_is() {
        let local = Sphere::new(Vec3::new(0.0, 0.0, -2.0), 0.5);
        let transform =
            Transform::from_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2));
        let bounds = world_bounds(local, &transform);
        // A quarter turn about +Y sends -Z to -X.
        assert!((bounds.center.x + 2.0).abs() < 1e-4, "{bounds:?}");
        assert!(bounds.center.z.abs() < 1e-4, "{bounds:?}");
    }

    #[test]
    fn hiding_an_entity_keeps_it_alive() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.set_visible(entity, false));
        assert!(world.contains(entity), "hidden is not despawned");
        assert!(!world.is_drawable(entity.index()));
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn styling_an_entity_keeps_its_handle_its_slot_and_its_material() {
        // Milestone 7's whole point: a visual change is a store, not a respawn.
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        let slots = world.slot_count();

        assert!(world.set_style(entity, InstanceStyle::tinted(0.5, 0.5, 0.5)));
        assert!(world.contains(entity), "styling is not despawning");
        assert_eq!(world.slot_count(), slots, "no slot was consumed");
        assert_eq!(world.len(), 1);
        assert_eq!(world.styles()[entity.index()].tint, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(world.materials()[entity.index()], MaterialHandle::new(0, 1));
    }

    #[test]
    fn changing_a_material_is_no_longer_a_respawn() {
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        let slots = world.slot_count();

        assert!(world.set_material(entity, MaterialHandle::new(7, 1)));
        assert!(world.contains(entity));
        assert_eq!(world.slot_count(), slots);
        assert_eq!(world.materials()[entity.index()], MaterialHandle::new(7, 1));
    }

    #[test]
    fn a_stale_handle_cannot_style_the_entity_that_replaced_it() {
        let mut world = RenderWorld::new();
        let first = world.spawn(desc(0, 0));
        world.despawn(first);
        let second = world.spawn(desc(1, 1));

        assert!(!world.set_style(first, InstanceStyle::tinted(0.0, 0.0, 0.0)));
        assert!(!world.set_material(first, MaterialHandle::new(9, 1)));
        assert!(world.styles()[second.index()].is_neutral());
    }

    #[test]
    fn a_spawned_entity_starts_unstyled() {
        // A default of zero would render everything black; neutral is a multiply by one.
        let mut world = RenderWorld::new();
        let entity = world.spawn(desc(0, 0));
        assert!(world.styles()[entity.index()].is_neutral());
        assert_eq!(InstanceStyle::default(), InstanceStyle::NEUTRAL);
        assert_eq!(InstanceStyle::NEUTRAL.tint, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn a_negative_emissive_is_clamped_rather_than_darkening_the_surface() {
        // Emissive is added to the shader's output; a negative one would subtract light.
        let style = InstanceStyle::NEUTRAL.with_emissive(-5.0);
        assert_eq!(style.emissive, 0.0);
    }

    #[test]
    fn clearing_empties_the_world() {
        let mut world = RenderWorld::with_capacity(8);
        let entity = world.spawn(desc(0, 0));
        world.clear();
        assert!(world.is_empty());
        assert!(!world.contains(entity));
        assert_eq!(world.slot_count(), 0);
    }

    #[test]
    fn a_slot_names_the_entity_that_lives_in_it() {
        // The culler walks slots and has to name what it found. A handle rebuilt with the
        // wrong generation would be indistinguishable from a stale one.
        let mut world = RenderWorld::new();
        let first = world.spawn(desc(0, 0));
        let second = world.spawn(desc(1, 1));

        assert_eq!(world.entity_at(first.index()), first);
        assert_eq!(world.entity_at(second.index()), second);

        // After a despawn and respawn the slot is reused with a new generation, and
        // `entity_at` must report the live one rather than the handle that is now stale.
        world.despawn(first);
        let reused = world.spawn(desc(2, 2));
        assert_eq!(reused.index(), first.index());
        assert_eq!(world.entity_at(reused.index()), reused);
        assert_ne!(world.entity_at(reused.index()), first);
    }

    #[test]
    fn an_invalid_handle_names_no_live_slot() {
        // Generation 0 exists so a world built only to measure culling can hold entities
        // whose geometry does not exist, without those handles ever matching a real slot.
        assert_eq!(MeshHandle::INVALID.raw().generation(), 0);
        assert_eq!(MaterialHandle::INVALID.raw().generation(), 0);
    }
}
