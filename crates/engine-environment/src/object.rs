//! One placed thing, and what of it a character runs into.
//!
//! ```text
//! EnvironmentObject
//!  ├ asset       which model to draw
//!  ├ transform   where, facing which way, how big
//!  └ collision   a primitive in the model's own space, or nothing at all
//! ```
//!
//! # Collision is not the visual mesh, and this is where that is enforced
//!
//! A pillar is 96 triangles of tapered prisms; what a character collides with is one box.
//! A fern is 40 triangles and collides with nothing. The two descriptions are separate
//! fields of separate types, so there is no path by which the visual mesh becomes the
//! collision mesh — a triangle-accurate static world is not something that can be reached by
//! accident from here, it would have to be written.
//!
//! The primitive is expressed in the **model's local space** and carried through the
//! object's transform when the collision world is built. That is what lets one asset be
//! placed at four scales and get four correctly-sized boxes from one authored number.

use engine_core::math::Vec3;
use engine_core::transform::Transform;

use crate::asset::EnvironmentAsset;

/// What a character runs into, in the model's local space.
///
/// Deliberately a tiny closed set. A convex hull, a capsule and a triangle mesh are all
/// things `engine-collision` could grow, and none of them is needed to stop a player walking
/// through a wall or a boulder — which is the whole job here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnvironmentCollision {
    /// Nothing. The object is decoration and the character walks through it.
    ///
    /// The default for grass, ferns, banners and anything below knee height: a collision
    /// shape a player can trip on without being able to see why is worse than no shape.
    None,
    /// An axis-aligned box in the model's local space.
    ///
    /// Becomes an AABB in the collision world when the object has no rotation, and an
    /// oriented box when it does — chosen at build time rather than authored, because an
    /// author should not have to know which is cheaper.
    Box {
        /// Centre of the box, in the model's local space. Usually half the model's height
        /// above its origin, because models stand on their origin.
        center: Vec3,
        /// Half-width, half-height and half-depth, in the model's local space.
        half_extents: Vec3,
    },
    /// Several boxes, held by the scene under this index.
    ///
    /// # Why an index and not the boxes
    ///
    /// One box is the overwhelmingly common case and an [`EnvironmentObject`] is 44 bytes
    /// because of it. A cliff overhang needs two boxes, a tunnel four and a room ten — and a
    /// variant big enough for a room would put two hundred bytes into every rock in the level
    /// to hold nothing.
    ///
    /// So the boxes live in the scene, in [`EnvironmentScene::collision_shapes`], and a
    /// placement names one. Two placements of the same cliff piece share an entry, and the
    /// object stays `Copy` and small.
    ///
    /// [`EnvironmentScene::collision_shapes`]: crate::EnvironmentScene::collision_shapes
    Compound(u16),
}

impl EnvironmentCollision {
    /// A box standing on the model's origin: a footprint and a height.
    ///
    /// The form nearly every piece of architecture and every boulder is authored in, because
    /// every model in this project stands on its own origin.
    #[must_use]
    pub fn standing(half_width: f32, height: f32, half_depth: f32) -> Self {
        Self::Box {
            center: Vec3::new(0.0, height * 0.5, 0.0),
            half_extents: Vec3::new(half_width, height * 0.5, half_depth),
        }
    }

    /// Whether this object blocks anything at all.
    ///
    /// A compound is taken at its word: an index the scene cannot resolve is a broken level
    /// rather than a decoration, and the build reports it where the resolution happens.
    #[must_use]
    pub fn blocks(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Which shape in the scene's table this names, if it names one.
    #[must_use]
    pub fn compound_index(self) -> Option<usize> {
        match self {
            Self::Compound(index) => Some(index as usize),
            _ => None,
        }
    }
}

/// One box of a compound collision shape, in the model's local space.
///
/// The same two numbers [`EnvironmentCollision::Box`] carries. A separate type because a
/// compound is a list of them and a list of two-field structs reads better than a list of
/// enum variants that can only ever be one of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionBox {
    /// Centre, in the model's local space.
    pub center: Vec3,
    /// Half-width, half-height and half-depth, in the model's local space.
    pub half_extents: Vec3,
}

impl CollisionBox {
    /// A box.
    #[must_use]
    pub fn new(center: Vec3, half_extents: Vec3) -> Self {
        Self {
            center,
            half_extents,
        }
    }

    /// A box standing on the model's origin: a footprint and a height.
    #[must_use]
    pub fn standing(half_width: f32, height: f32, half_depth: f32) -> Self {
        Self::new(
            Vec3::new(0.0, height * 0.5, 0.0),
            Vec3::new(half_width, height * 0.5, half_depth),
        )
    }

    /// A box between two corners of the model's local space.
    ///
    /// The form a cliff or a cave piece is authored in: "the wall runs from here to here",
    /// which is how somebody reads it off the geometry they just built.
    #[must_use]
    pub fn between(low: Vec3, high: Vec3) -> Self {
        let min = low.min(high);
        let max = low.max(high);
        Self::new((min + max) * 0.5, (max - min) * 0.5)
    }

    /// Whether the box has volume. A flat one is a shape no query can ever touch.
    #[must_use]
    pub fn is_solid(&self) -> bool {
        self.half_extents.x > 0.0 && self.half_extents.y > 0.0 && self.half_extents.z > 0.0
    }
}

/// A named set of boxes: what one asset blocks, when one box is not enough to say it.
///
/// The whole reason this exists is the overhang. A cliff a player can walk under is a wall
/// and a slab with air between them, and a single box around both is a wall the player walks
/// into. The same is true of every cave piece: the cavity *is* the asset.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CollisionShape {
    /// The boxes, in the model's local space.
    pub boxes: Vec<CollisionBox>,
}

impl CollisionShape {
    /// A shape made of `boxes`, dropping any that have no volume.
    #[must_use]
    pub fn new(boxes: impl IntoIterator<Item = CollisionBox>) -> Self {
        Self {
            boxes: boxes.into_iter().filter(CollisionBox::is_solid).collect(),
        }
    }

    /// How many boxes it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    /// Whether it holds none, which means it blocks nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }
}

/// One placed environment object.
///
/// 44 bytes plus the discriminant: an asset id, a transform and a primitive. The shipping
/// arena is 141 of these, and a stress scene is a few thousand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvironmentObject {
    /// Which model to draw.
    pub asset: EnvironmentAsset,
    /// Where it stands, which way it faces, how large it is.
    pub transform: Transform,
    /// What of it blocks a character.
    pub collision: EnvironmentCollision,
}

impl EnvironmentObject {
    /// A decorative object at `position`: drawn, walked through.
    #[must_use]
    pub fn decoration(asset: EnvironmentAsset, position: Vec3) -> Self {
        Self {
            asset,
            transform: Transform::from_translation(position),
            collision: EnvironmentCollision::None,
        }
    }

    /// The same object, rotated `yaw` radians about +Y.
    ///
    /// Yaw only, because everything in the arena stands on the ground. A prop that needed
    /// pitch or roll would set [`EnvironmentObject::transform`] directly, and nothing does.
    #[must_use]
    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.transform.rotation = engine_core::math::Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0);
        self
    }

    /// The same object, uniformly scaled.
    ///
    /// Uniform on purpose: the bounding sphere a cull tests against is scaled by the
    /// transform's largest axis, so a non-uniform scale over-estimates the bound in two axes
    /// out of three and draws things that could have been skipped. The one exception in the
    /// library is the ground plane, which sets its scale directly.
    #[must_use]
    pub fn with_scale(mut self, scale: f32) -> Self {
        self.transform.scale = Vec3::splat(scale);
        self
    }

    /// The same object with a collision primitive.
    #[must_use]
    pub fn with_collision(mut self, collision: EnvironmentCollision) -> Self {
        self.collision = collision;
        self
    }

    /// Where the object stands.
    #[must_use]
    pub fn position(&self) -> Vec3 {
        self.transform.translation
    }

    /// The collision box in world space, as a centre, half-extents and a yaw.
    ///
    /// `None` for a decoration. The half-extents are scaled by the object's own scale, so an
    /// asset authored once and placed at 0.7 and at 1.6 gets two correctly-sized boxes
    /// without either number being written down twice.
    ///
    /// Returned as plain numbers rather than as an `engine_collision::Shape`, because this
    /// crate does not depend on `engine-collision` — the caller that does turns these into a
    /// shape, and that caller is `engine_game::arena`.
    #[must_use]
    pub fn world_collision(&self) -> Option<WorldBox> {
        let EnvironmentCollision::Box {
            center,
            half_extents,
        } = self.collision
        else {
            return None;
        };
        Some(self.place_box(CollisionBox::new(center, half_extents)))
    }

    /// One local-space box, carried through this object's transform.
    ///
    /// The half-extents are scaled by the object's own scale, so an asset authored once and
    /// placed at 0.7 and at 1.6 gets two correctly-sized boxes without either number being
    /// written down twice.
    #[must_use]
    pub fn place_box(&self, shape: CollisionBox) -> WorldBox {
        WorldBox {
            center: self.transform.transform_point(shape.center),
            half_extents: Vec3::new(
                shape.half_extents.x * self.transform.scale.x,
                shape.half_extents.y * self.transform.scale.y,
                shape.half_extents.z * self.transform.scale.z,
            ),
            rotation: self.transform.rotation,
        }
    }
}

/// What an asset blocks when nobody says otherwise.
///
/// Every placement carries its own primitive, and a layout function is free to author one —
/// the arena's kerb is a knee-high wall segment and the dungeon's is a two-and-a-half metre
/// one, from the same asset. This is the answer for the two authors that cannot make that
/// judgement: the Blender layout importer and the in-game editor, where an object is placed
/// by pointing at the ground and has to block something sensible immediately.
///
/// The numbers are the ones the arena and the dungeon already author by hand, with the
/// blocking reading chosen where they disagree — an editor that placed a wall you could walk
/// through would be an editor that lies about the level.
#[must_use]
pub fn default_collision(asset: EnvironmentAsset) -> EnvironmentCollision {
    use crate::asset::{
        ARCH, BRAZIER, PILLAR, PINE, ROCK_MEDIUM, ROCK_SPIRE, SUPPLY_CRATE, WALL_SEGMENT,
    };
    match asset {
        // Full height, not the arena's kerb: a wall an editor places is a wall meant to stop
        // somebody.
        a if a == WALL_SEGMENT => EnvironmentCollision::standing(4.0, 2.4, 0.45),
        a if a == PILLAR => EnvironmentCollision::standing(0.62, 4.6, 0.62),
        // The two legs, as one box. An arch is a doorway and the doorway must stay open, so
        // the box spans the jambs and not the span between them.
        a if a == ARCH => EnvironmentCollision::standing(0.7, 5.5, 0.7),
        a if a == BRAZIER => EnvironmentCollision::standing(0.45, 1.35, 0.45),
        a if a == SUPPLY_CRATE => EnvironmentCollision::standing(0.46, 0.92, 0.46),
        a if a == ROCK_MEDIUM => EnvironmentCollision::standing(0.85, 1.15, 0.78),
        a if a == ROCK_SPIRE => EnvironmentCollision::standing(0.66, 3.6, 0.62),
        // The trunk, not the canopy. A character brushing through the lower branches of a
        // conifer is correct; being stopped four metres from the trunk is not.
        a if a == PINE => EnvironmentCollision::standing(0.28, 5.0, 0.28),
        // Ground, ground cover, banners, distant scenery and the sky: nothing. A collision
        // shape a player can trip on without being able to see why is worse than none.
        _ => EnvironmentCollision::None,
    }
}

/// A collision box placed in the world: centre, half-extents, orientation.
///
/// The hand-off type between this crate and whoever owns a collision world. Plain math.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldBox {
    /// Centre in world space.
    pub center: Vec3,
    /// Half-extents along the box's own axes, already scaled.
    pub half_extents: Vec3,
    /// Orientation. Identity for most of the arena, which is what lets the consumer pick a
    /// cheap axis-aligned box instead of an oriented one.
    pub rotation: engine_core::math::Quat,
}

impl WorldBox {
    /// How far the orientation is from the identity, as the length of its vector part.
    ///
    /// Zero for an unrotated box. The consumer compares this against a small epsilon to
    /// decide between an AABB and an OBB; a quarter-degree of yaw is not worth an oriented
    /// box's narrow-phase cost.
    #[must_use]
    pub fn rotation_magnitude(&self) -> f32 {
        let rotation = self.rotation;
        (rotation.x * rotation.x + rotation.y * rotation.y + rotation.z * rotation.z).sqrt()
    }

    /// The axis-aligned extents, for the unrotated case.
    #[must_use]
    pub fn min_max(&self) -> (Vec3, Vec3) {
        (
            self.center - self.half_extents,
            self.center + self.half_extents,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{FERN, PILLAR};
    use engine_core::math::Quat;

    #[test]
    fn the_default_table_blocks_what_a_character_should_not_walk_through() {
        use crate::asset::{GROUND_PLAIN, LIBRARY, PINE, ROCK_SMALL, SKY_DOME, WALL_SEGMENT};

        assert!(default_collision(WALL_SEGMENT).blocks(), "a wall stops you");
        assert!(default_collision(PILLAR).blocks());
        assert!(default_collision(PINE).blocks(), "the trunk does");
        assert!(!default_collision(FERN).blocks(), "ground cover does not");
        assert!(
            !default_collision(ROCK_SMALL).blocks(),
            "nor a knee-high rock"
        );
        assert!(!default_collision(GROUND_PLAIN).blocks());
        assert!(!default_collision(SKY_DOME).blocks());

        // Every box in the table stands on the model's origin, because every model in the
        // library does. A box centred below the floor is a box a character falls into.
        for asset in LIBRARY {
            if let EnvironmentCollision::Box {
                center,
                half_extents,
            } = default_collision(asset)
            {
                assert!(
                    (center.y - half_extents.y).abs() < 1e-6,
                    "{asset} does not stand on its origin"
                );
            }
        }
    }

    #[test]
    fn a_decoration_blocks_nothing_and_has_no_world_box() {
        let fern = EnvironmentObject::decoration(FERN, Vec3::new(3.0, 0.0, -4.0));
        assert!(!fern.collision.blocks());
        assert_eq!(fern.world_collision(), None);
        assert_eq!(fern.position(), Vec3::new(3.0, 0.0, -4.0));
    }

    #[test]
    fn a_standing_box_sits_on_the_models_origin() {
        let collision = EnvironmentCollision::standing(0.4, 3.0, 0.4);
        let EnvironmentCollision::Box {
            center,
            half_extents,
        } = collision
        else {
            panic!("a standing box is a box");
        };
        assert_eq!(center, Vec3::new(0.0, 1.5, 0.0));
        assert_eq!(half_extents, Vec3::new(0.4, 1.5, 0.4));
        assert!(collision.blocks());
    }

    #[test]
    fn scaling_a_placement_scales_the_box_it_produces() {
        // The property the whole local-space arrangement exists for: one authored number,
        // any number of correctly-sized placements.
        let pillar = EnvironmentObject::decoration(PILLAR, Vec3::new(10.0, 0.0, 0.0))
            .with_collision(EnvironmentCollision::standing(0.5, 4.0, 0.5))
            .with_scale(2.0);
        let Some(world) = pillar.world_collision() else {
            panic!("a pillar blocks");
        };
        assert_eq!(world.half_extents, Vec3::new(1.0, 4.0, 1.0));
        assert_eq!(world.center, Vec3::new(10.0, 4.0, 0.0));
        let (min, max) = world.min_max();
        assert_eq!(min.y, 0.0, "the box still stands on the floor");
        assert_eq!(max.y, 8.0);
    }

    #[test]
    fn a_yawed_placement_reports_its_rotation_and_an_unyawed_one_reports_none() {
        let straight = EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
            .with_collision(EnvironmentCollision::standing(1.0, 2.0, 1.0));
        let Some(world) = straight.world_collision() else {
            panic!("it blocks");
        };
        assert!(world.rotation_magnitude() < 1e-6, "an unrotated box");

        let turned = straight.with_yaw(std::f32::consts::FRAC_PI_4);
        let Some(world) = turned.world_collision() else {
            panic!("it blocks");
        };
        assert!(world.rotation_magnitude() > 0.3, "a rotated box");
        assert_eq!(
            turned.transform.rotation,
            Quat::from_yaw_pitch_roll(std::f32::consts::FRAC_PI_4, 0.0, 0.0)
        );
    }

    #[test]
    fn a_yawed_box_carries_its_offset_centre_round_with_it() {
        // A box whose centre is not on the model's origin has to be rotated about the
        // origin, not about itself, or a wall segment turned to face along Z ends up beside
        // the wall it is meant to be.
        let object = EnvironmentObject::decoration(PILLAR, Vec3::ZERO)
            .with_collision(EnvironmentCollision::Box {
                center: Vec3::new(0.0, 1.0, -2.0),
                half_extents: Vec3::new(1.0, 1.0, 0.5),
            })
            .with_yaw(std::f32::consts::FRAC_PI_2);
        let Some(world) = object.world_collision() else {
            panic!("it blocks");
        };
        // A quarter turn about +Y sends -Z to -X.
        assert!((world.center.x + 2.0).abs() < 1e-4, "{world:?}");
        assert!(world.center.z.abs() < 1e-4, "{world:?}");
        assert!((world.center.y - 1.0).abs() < 1e-4, "{world:?}");
    }
}
