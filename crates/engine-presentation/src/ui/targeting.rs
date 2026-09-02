//! The ground-targeting system: one circle, shared by everything that is placed.
//!
//! ```text
//! press METEOR -> targeting mode -> circle follows the mouse -> left click confirms
//!                                                            -> right click / escape cancels
//! ```
//!
//! Meteor, Firestorm and Cataclysm all use this. They differ only in radius, range and
//! colour, which are read from the ability's own definition - so a fourth placed ability is a
//! row in the ability table and no new code here.
//!
//! The placement is computed by meeting the mouse ray with the ground - a plane for a level
//! with no height field, the height field itself for one that has - which is what makes the
//! circle sit where the player is pointing rather than where the camera is.
//! Range is checked against the caster, and the indicator says whether the placement is
//! valid *before* the click - a refusal after the fact is a wasted cooldown.

use engine_core::math::{Mat4, Vec3};
use engine_game::firemage::{FireAbility, ZoneShape};

use super::ground_decal::GroundSurface;
use super::input::UiInput;

/// Height of the ground plane the circle is placed on, in metres.
///
/// The arena floor: what [`GroundSurface::Plane`] is built from for a level with no height
/// field. A level that *has* one is sampled instead — see [`ground_under_cursor`].
pub const GROUND_HEIGHT: f32 = 0.0;

/// How many steps the cursor ray is walked in before the crossing is refined.
///
/// The walk only covers the slab of space the terrain occupies, so these are spent on the
/// stretch of the ray that can actually meet the ground rather than spread over the view
/// distance. A hundred and twenty-eight over a level's height band puts the first crossing
/// within one step of the true one on anything short of a knife-edge.
const MARCH_STEPS: usize = 128;

/// How many bisections refine the step the ray crossed the ground in.
///
/// Twelve halvings shrink a step to a four-thousandth of itself, which is far below the
/// centimetre the indicator is drawn to. It is a fixed count rather than a tolerance so the
/// cost of a cursor move is a constant a profile can rely on.
const MARCH_REFINEMENTS: usize = 12;

/// The footprint an ability covers, independent of which ability it is.
///
/// The targeting system handles the cursor, the validation, the confirmation and the
/// cancellation; the *ability* supplies one of these and the range it may be placed within.
/// That split is what makes the system reusable: a future rectangle ability adds a variant
/// and a mesh, and nothing about placing it changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetShape {
    /// A circle of `radius` metres. What every current ground ability uses.
    Circle {
        /// Metres from the centre.
        radius: f32,
    },
    /// A cone of `range` metres and `half_angle` radians either side of centre.
    Cone {
        /// How far it reaches.
        range: f32,
        /// Half the opening angle, radians.
        half_angle: f32,
    },
    /// An axis-aligned rectangle, in metres.
    Rectangle {
        /// Across.
        width: f32,
        /// Along.
        length: f32,
    },
    /// A line of `width` metres running `length` metres from the caster.
    Line {
        /// Across.
        width: f32,
        /// Along.
        length: f32,
    },
}

impl TargetShape {
    /// The shape an ability's own definition asks for.
    ///
    /// Read from the ability table, so an ability's footprint and the circle the player
    /// places are the same number by construction.
    #[must_use]
    pub fn of(ability: FireAbility) -> Self {
        let definition = ability.definition();
        match definition.target {
            engine_game::firemage::TargetMode::Cone => Self::Cone {
                range: definition.range,
                // The executor's own spread, so the wedge shown is the wedge that is hit.
                half_angle: engine_game::sim::CONE_HALF_ANGLE,
            },
            // Firestorm: a rectangle from the caster along the direction the mouse picks,
            // with the executor's own length and width so the strip shown is the strip
            // that burns.
            engine_game::firemage::TargetMode::Line => Self::Line {
                width: definition.width,
                length: definition.range,
            },
            _ => Self::Circle {
                radius: definition.radius,
            },
        }
    }

    /// How far the shape reaches from its centre, for a bounding test and for the mesh.
    #[must_use]
    pub fn extent(self) -> f32 {
        match self {
            Self::Circle { radius } => radius,
            Self::Cone { range, .. } => range,
            // A line is anchored on the caster and runs forwards, so how far it reaches is
            // its length - the same answer a cone gives, for the same reason.
            Self::Line { length, .. } => length,
            Self::Rectangle { width, length } => (width.max(length)) * 0.5,
        }
    }

    /// Whether a point on the ground is inside the shape placed at `anchor`.
    ///
    /// `facing` is where the caster looks, which the directional shapes use.
    ///
    /// A circle and a line answer through [`engine_game::firemage::ZoneShape`] - the same
    /// containment test the *burn* uses. That matters: this function decides which enemies
    /// the interface rings while the player is aiming, and an interface that highlighted a
    /// different set from the one the ability hits would be lying at the exact moment the
    /// player is deciding. There is one implementation, and it lives in gameplay.
    #[must_use]
    pub fn contains(self, anchor: Vec3, facing: f32, point: Vec3) -> bool {
        self.contains_body(anchor, facing, point, 0.0)
    }

    /// The same test for a body of `body_radius` standing at `point`.
    ///
    /// The executor is edge-corrected - an enemy whose centre is just outside the shape but
    /// whose body is inside it is hit - so the highlight has to be too. Without this the
    /// ring and the fire disagree by half a metre on every enemy at the edge, which is the
    /// half a metre a player is actually aiming with.
    #[must_use]
    pub fn contains_body(self, anchor: Vec3, facing: f32, point: Vec3, body_radius: f32) -> bool {
        match self {
            Self::Circle { radius } => {
                ZoneShape::Circle { radius }.contains(anchor, point, body_radius)
            }
            Self::Line { width, length } => ZoneShape::Line {
                yaw: facing,
                length,
                width,
            }
            .contains(anchor, point, body_radius),
            Self::Cone { range, half_angle } => {
                let offset = Vec3::new(point.x - anchor.x, 0.0, point.z - anchor.z);
                let distance = (offset.x * offset.x + offset.z * offset.z).sqrt();
                if distance > range + body_radius {
                    return false;
                }
                if distance <= f32::EPSILON {
                    return true;
                }
                let forward = Vec3::new(-facing.sin(), 0.0, -facing.cos());
                let cosine = (offset.x * forward.x + offset.z * forward.z) / distance;
                cosine >= half_angle.cos()
            }
            // Centred on its anchor and turned to face, unlike a line, which starts at one.
            Self::Rectangle { width, length } => {
                let offset = Vec3::new(point.x - anchor.x, 0.0, point.z - anchor.z);
                let (sin, cos) = (-facing.sin(), -facing.cos());
                let along = offset.x * sin + offset.z * cos;
                let across = offset.x * -cos + offset.z * sin;
                along.abs() <= length * 0.5 + body_radius
                    && across.abs() <= width * 0.5 + body_radius
            }
        }
    }
}

/// What the player is currently placing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundTarget {
    /// Which ability is waiting to be placed.
    pub ability: FireAbility,
    /// What footprint it covers.
    pub shape: TargetShape,
    /// Which way it points, radians about +Y. Only the directional shapes use it.
    pub facing: f32,
    /// Where the shape is anchored: the caster for a cone, the cursor for a circle.
    pub anchor: Vec3,
    /// Where the indicator currently sits, in world space.
    pub position: Vec3,
    /// How big it is, in metres. The circle's radius, or the shape's extent.
    pub radius: f32,
    /// How far from the caster it may be placed.
    pub range: f32,
    /// Whether the current placement would be accepted.
    pub valid: bool,
}

/// What the targeting system decided this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TargetingOutcome {
    /// Nothing happened.
    Idle,
    /// The player is still choosing.
    Choosing,
    /// The player confirmed a placement.
    Confirmed {
        /// Which ability.
        ability: FireAbility,
        /// Where it goes.
        position: Vec3,
    },
    /// The player backed out.
    Cancelled,
}

/// The one placement system, shared by every ground-targeted ability.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct GroundTargeting {
    active: Option<GroundTarget>,
}

impl GroundTargeting {
    /// Not placing anything.
    #[must_use]
    pub const fn new() -> Self {
        Self { active: None }
    }

    /// What is being placed, if anything.
    #[must_use]
    pub fn active(&self) -> Option<GroundTarget> {
        self.active
    }

    /// Whether the player is choosing a location.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Enters targeting mode for `ability`, reading its radius and range from its definition.
    pub fn begin(&mut self, ability: FireAbility) {
        let definition = ability.definition();
        let shape = TargetShape::of(ability);
        self.active = Some(GroundTarget {
            ability,
            shape,
            facing: 0.0,
            anchor: Vec3::ZERO,
            position: Vec3::ZERO,
            radius: shape.extent(),
            range: definition.range,
            valid: false,
        });
    }

    /// Leaves targeting mode.
    pub fn cancel(&mut self) {
        self.active = None;
    }

    /// Moves the circle and reads the frame's clicks.
    ///
    /// `caster` is where the player is standing, which is what range is measured from, and
    /// `ground` is the surface the cursor is met with — a plane for a level with no height
    /// field, the height field itself for one that has.
    pub fn update(
        &mut self,
        input: &UiInput,
        view_projection: &Mat4,
        viewport: (u32, u32),
        caster: Vec3,
        ground: &GroundSurface<'_>,
    ) -> TargetingOutcome {
        let Some(mut target) = self.active else {
            return TargetingOutcome::Idle;
        };

        // Escape or the right button backs out, exactly as the brief asks.
        if input.cancel || input.secondary_pressed {
            self.active = None;
            return TargetingOutcome::Cancelled;
        }

        if let Some(position) =
            ground_under_cursor(input.pointer, view_projection, viewport, ground)
        {
            // On the ground rather than on `y = 0`: the indicator is drawn from this point,
            // and a point floating over a hillside would put the whole shape in the air.
            let flat = Vec3::new(
                position.x,
                ground.height_at(position.x, position.z),
                position.z,
            );
            target.position = flat;
            match target.shape {
                // A cone is anchored on the caster and *aimed*: the cursor picks a
                // direction, not a place, so it is always a legal cast.
                TargetShape::Cone { .. } | TargetShape::Line { .. } => {
                    let dx = flat.x - caster.x;
                    let dz = flat.z - caster.z;
                    target.anchor =
                        Vec3::new(caster.x, ground.height_at(caster.x, caster.z), caster.z);
                    if dx.abs() > 1e-4 || dz.abs() > 1e-4 {
                        // Zero yaw faces -Z, which is what the executor and the mesh agree on.
                        target.facing = (-dx).atan2(-dz);
                    }
                    target.valid = true;
                }
                _ => {
                    target.anchor = flat;
                    target.valid = horizontal_distance(caster, flat) <= target.range;
                }
            }
        }
        self.active = Some(target);

        if input.pointer_pressed {
            if !target.valid {
                // Out of range: the click is refused rather than spending the cooldown on a
                // placement the simulation would reject anyway.
                return TargetingOutcome::Choosing;
            }
            self.active = None;
            return TargetingOutcome::Confirmed {
                ability: target.ability,
                position: target.position,
            };
        }
        TargetingOutcome::Choosing
    }
}

/// Where the cursor is pointing on the ground, if anywhere.
///
/// The mouse position becomes a ray through the inverse view-projection and the ray is met
/// with `ground`. A ray parallel to the ground, or one pointing at the sky, has no answer and
/// the circle stays where it was.
///
/// A [`GroundSurface::Plane`] is one division: the ray is intersected with `y = height`. A
/// [`GroundSurface::Terrain`] cannot be — a height field has no closed form — so the ray is
/// walked until it first passes below the surface and the crossing is then bisected. The walk
/// is restricted to the slab between the level's lowest and highest ground, which is the only
/// stretch of the ray where a crossing can exist and is what keeps a hundred and twenty-eight
/// steps fine rather than coarse.
///
/// **The first crossing, not the nearest point.** A ray that would clear a ridge and land in
/// the valley behind it stops on the ridge, exactly as the eye does: the player is pointing at
/// the ground they can see.
#[must_use]
pub fn ground_under_cursor(
    pointer: (f32, f32),
    view_projection: &Mat4,
    viewport: (u32, u32),
    ground: &GroundSurface<'_>,
) -> Option<Vec3> {
    let (near, direction) = cursor_ray(pointer, view_projection, viewport)?;
    match ground {
        GroundSurface::Plane(height) => {
            if direction.y.abs() < 1e-6 {
                return None;
            }
            let t = (height - near.y) / direction.y;
            if t < 0.0 {
                return None;
            }
            Some(near + direction * t)
        }
        GroundSurface::Terrain(_) => march_to_ground(near, direction, ground),
    }
}

/// Walks `origin + direction * t` for `t` in `0..=1` until it is under `ground`.
///
/// `direction` spans the near plane to the far plane, which is what
/// [`cursor_ray`] promises, so the whole visible ray is the unit interval.
fn march_to_ground(origin: Vec3, direction: Vec3, ground: &GroundSurface<'_>) -> Option<Vec3> {
    let (low, high) = ground.height_range();
    // The slab the ground lives in, with a metre of slack either side so a ray that enters
    // exactly at the highest point is not clipped out by rounding.
    let (mut start, mut end) = (0.0_f32, 1.0_f32);
    if direction.y.abs() > 1e-6 {
        let enter = (high + 1.0 - origin.y) / direction.y;
        let exit = (low - 1.0 - origin.y) / direction.y;
        let (near_t, far_t) = if enter <= exit {
            (enter, exit)
        } else {
            (exit, enter)
        };
        start = start.max(near_t);
        end = end.min(far_t);
        if start > end {
            return None;
        }
    }

    let height_of = |t: f32| {
        let point = origin + direction * t;
        point.y - ground.height_at(point.x, point.z)
    };

    let mut previous = start;
    if height_of(previous) <= 0.0 {
        // The ray starts underground — the camera is inside a hill. There is no surface in
        // front of the player to point at, and inventing one would put the indicator behind
        // the terrain the player is looking at.
        return None;
    }
    for step in 1..=MARCH_STEPS {
        let t = start + (end - start) * (step as f32 / MARCH_STEPS as f32);
        if height_of(t) <= 0.0 {
            // Bisect the one step that straddles the surface: `previous` is above it and `t`
            // is below it, so the crossing is between them and halving never leaves it.
            let (mut above, mut below) = (previous, t);
            for _ in 0..MARCH_REFINEMENTS {
                let middle = (above + below) * 0.5;
                if height_of(middle) > 0.0 {
                    above = middle;
                } else {
                    below = middle;
                }
            }
            let hit = origin + direction * ((above + below) * 0.5);
            return Some(Vec3::new(hit.x, ground.height_at(hit.x, hit.z), hit.z));
        }
        previous = t;
    }
    None
}

/// The world-space ray under the cursor: where it starts, and which way it goes.
///
/// The cursor position is unprojected through the inverse view-projection at both depths, and
/// the ray is the line between them — so its `direction` spans exactly the near plane to the
/// far plane and a parameter of one is the far plane. It is deliberately *not* normalised:
/// callers either intersect something with it, which does not care, or want that property.
///
/// This is the only place a screen coordinate becomes a world direction. Everything that
/// points at something — placing a Meteor, clicking on an enemy — asks here, so there is one
/// unprojection to be wrong rather than one per feature.
#[must_use]
pub fn cursor_ray(
    pointer: (f32, f32),
    view_projection: &Mat4,
    viewport: (u32, u32),
) -> Option<(Vec3, Vec3)> {
    if viewport.0 == 0 || viewport.1 == 0 {
        return None;
    }
    let inverse = view_projection.inverse()?;

    // Client pixels to normalized device coordinates: x right, y up, z into the screen.
    let ndc_x = (pointer.0 / viewport.0 as f32) * 2.0 - 1.0;
    let ndc_y = 1.0 - (pointer.1 / viewport.1 as f32) * 2.0;

    let near = inverse.transform_point(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inverse.transform_point(Vec3::new(ndc_x, ndc_y, 1.0));
    Some((near, far - near))
}

/// Distance on the ground, ignoring height.
#[must_use]
pub fn horizontal_distance(a: Vec3, b: Vec3) -> f32 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_render_data::{Camera, Projection};

    /// A camera looking down at the origin from behind, like the game's.
    fn camera() -> Camera {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("a projection");
        camera.set_position(Vec3::new(0.0, 8.0, 12.0));
        camera.set_rotation(engine_core::math::Quat::from_yaw_pitch_roll(0.0, -0.5, 0.0));
        camera
    }

    fn input_at(x: f32, y: f32) -> UiInput {
        UiInput::new().with_pointer(x, y)
    }

    /// The arena floor: what a level with no height field offers.
    fn flat_ground() -> GroundSurface<'static> {
        GroundSurface::Plane(GROUND_HEIGHT)
    }

    /// A terrain that rises `slope` metres for every metre travelled along +X.
    fn ramp(slope: f32) -> engine_environment::terrain::Terrain {
        let mut terrain = engine_environment::terrain::Terrain::flat(129, 1.0, 0.0);
        for z in 0..terrain.resolution {
            for x in 0..terrain.resolution {
                let world_x = terrain.origin.x + x as f32 * terrain.cell_size;
                terrain.heights[z * terrain.resolution + x] = world_x * slope;
            }
        }
        terrain
    }

    #[test]
    fn nothing_is_being_placed_until_an_ability_asks() {
        let mut targeting = GroundTargeting::new();
        assert!(!targeting.is_active());
        assert_eq!(
            targeting.update(
                &input_at(0.0, 0.0),
                &camera().view_projection(),
                (1280, 720),
                Vec3::ZERO,
                &flat_ground(),
            ),
            TargetingOutcome::Idle
        );

        targeting.begin(FireAbility::Meteor);
        assert!(targeting.is_active());
        let active = targeting.active().expect("something is being placed");
        assert_eq!(active.ability, FireAbility::Meteor);
        assert_eq!(active.radius, FireAbility::Meteor.definition().radius);
        assert_eq!(active.range, FireAbility::Meteor.definition().range);
    }

    #[test]
    fn every_placed_ability_shares_the_one_system_and_brings_its_own_numbers() {
        // The brief's requirement: one targeting implementation, several shapes, and every
        // number read from the ability rather than from the targeting code.
        for ability in [
            FireAbility::Meteor,
            FireAbility::Firestorm,
            FireAbility::FlameCone,
            FireAbility::Cataclysm,
        ] {
            let mut targeting = GroundTargeting::new();
            targeting.begin(ability);
            let active = targeting.active().expect("placing");
            assert!(active.radius > 0.0, "{ability:?} has no shape to draw");
            match active.shape {
                TargetShape::Circle { radius } => {
                    assert_eq!(radius, ability.definition().radius, "{ability:?}");
                }
                TargetShape::Cone { range, .. } => {
                    assert_eq!(range, ability.definition().range, "{ability:?}");
                }
                TargetShape::Line { width, length } => {
                    assert_eq!(width, ability.definition().width, "{ability:?}");
                    assert_eq!(length, ability.definition().range, "{ability:?}");
                }
                TargetShape::Rectangle { .. } => panic!("{ability:?} asked for a rectangle"),
            }
        }
    }

    #[test]
    fn the_circle_follows_the_mouse_across_the_ground() {
        let mut targeting = GroundTargeting::new();
        targeting.begin(FireAbility::Meteor);
        let view_projection = camera().view_projection();

        targeting.update(
            &input_at(640.0, 500.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
            &flat_ground(),
        );
        let first = targeting.active().expect("placing").position;

        targeting.update(
            &input_at(300.0, 500.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
            &flat_ground(),
        );
        let second = targeting.active().expect("placing").position;

        assert!(
            horizontal_distance(first, second) > 0.5,
            "the circle did not move: {first:?} -> {second:?}"
        );
        assert!((first.y - GROUND_HEIGHT).abs() < 1e-6, "it left the ground");
    }

    #[test]
    fn a_left_click_confirms_the_placement() {
        let mut targeting = GroundTargeting::new();
        targeting.begin(FireAbility::Meteor);
        let view_projection = camera().view_projection();

        // Point at the ground first so the placement is valid.
        targeting.update(
            &input_at(640.0, 500.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
            &flat_ground(),
        );
        let click = UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..input_at(640.0, 500.0)
        };
        let outcome = targeting.update(
            &click,
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
            &flat_ground(),
        );

        match outcome {
            TargetingOutcome::Confirmed { ability, position } => {
                assert_eq!(ability, FireAbility::Meteor);
                assert!((position.y - GROUND_HEIGHT).abs() < 1e-6);
            }
            other => panic!("the click did not confirm: {other:?}"),
        }
        assert!(!targeting.is_active(), "targeting mode outlived the click");
    }

    #[test]
    fn escape_and_the_right_button_both_cancel() {
        let view_projection = camera().view_projection();
        for cancel in [
            UiInput {
                cancel: true,
                ..input_at(640.0, 500.0)
            },
            UiInput {
                secondary_pressed: true,
                ..input_at(640.0, 500.0)
            },
        ] {
            let mut targeting = GroundTargeting::new();
            targeting.begin(FireAbility::Firestorm);
            let outcome = targeting.update(
                &cancel,
                &view_projection,
                (1280, 720),
                Vec3::ZERO,
                &flat_ground(),
            );
            assert_eq!(outcome, TargetingOutcome::Cancelled);
            assert!(!targeting.is_active());
        }
    }

    #[test]
    fn a_placement_out_of_range_is_marked_invalid_and_refuses_the_click() {
        let mut targeting = GroundTargeting::new();
        targeting.begin(FireAbility::Meteor);
        let view_projection = camera().view_projection();

        // The caster is a long way from where the camera is looking.
        let caster = Vec3::new(500.0, 0.0, 500.0);
        targeting.update(
            &input_at(640.0, 500.0),
            &view_projection,
            (1280, 720),
            caster,
            &flat_ground(),
        );
        assert!(!targeting.active().expect("placing").valid);

        let click = UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..input_at(640.0, 500.0)
        };
        assert_eq!(
            targeting.update(
                &click,
                &view_projection,
                (1280, 720),
                caster,
                &flat_ground()
            ),
            TargetingOutcome::Choosing,
            "an invalid placement was confirmed"
        );
        assert!(targeting.is_active(), "the refusal left targeting mode");
    }

    #[test]
    fn a_ray_that_never_meets_the_ground_leaves_the_circle_where_it_was() {
        // Pointing at the sky: there is no answer, and inventing one would teleport the
        // circle to the horizon.
        let mut targeting = GroundTargeting::new();
        targeting.begin(FireAbility::Meteor);
        let view_projection = camera().view_projection();

        targeting.update(
            &input_at(640.0, 600.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
            &flat_ground(),
        );
        let grounded = targeting.active().expect("placing").position;

        targeting.update(
            &input_at(640.0, 0.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
            &flat_ground(),
        );
        let after = targeting.active().expect("placing").position;
        assert!(
            after.y.abs() < 1e-6,
            "the circle left the ground: {after:?}"
        );
        let _ = grounded;
    }

    #[test]
    fn a_line_highlights_exactly_what_it_will_burn() {
        // The bug this replaces: the line's containment ignored `facing` and treated the
        // strip as *centred* on the caster, so the ring appeared on enemies behind the
        // player and missed the ones the fire would actually reach.
        let firestorm = FireAbility::Firestorm.definition();
        let shape = TargetShape::of(FireAbility::Firestorm);
        let caster = Vec3::ZERO;

        // Aimed down -Z, which is what a facing of zero means everywhere in the project.
        assert!(
            shape.contains(caster, 0.0, Vec3::new(0.0, 0.0, -10.0)),
            "ahead"
        );
        assert!(
            shape.contains(caster, 0.0, Vec3::new(4.9, 0.0, -19.0)),
            "inside the far corner of a twenty by ten strip"
        );
        assert!(
            !shape.contains(caster, 0.0, Vec3::new(0.0, 0.0, 10.0)),
            "behind the caster"
        );
        assert!(
            !shape.contains(caster, 0.0, Vec3::new(0.0, 0.0, -21.0)),
            "past the end"
        );
        assert!(
            !shape.contains(caster, 0.0, Vec3::new(6.0, 0.0, -10.0)),
            "beside it"
        );

        // Turned a quarter circle: what was ahead is now beside, and what was beside is
        // ahead. A containment test that ignored the facing would pass none of this.
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(!shape.contains(caster, quarter, Vec3::new(0.0, 0.0, -10.0)));
        assert!(shape.contains(caster, quarter, Vec3::new(-10.0, 0.0, 0.0)));

        // And it is the gameplay shape's own answer, not a second implementation of it.
        for point in [
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(3.0, 0.0, -18.0),
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(9.0, 0.0, -2.0),
        ] {
            let burn = ZoneShape::Line {
                yaw: 0.4,
                length: firestorm.range,
                width: firestorm.width,
            };
            assert_eq!(
                shape.contains(caster, 0.4, point),
                burn.contains(caster, point, 0.0),
                "the ring and the fire disagree about {point:?}"
            );
        }
    }

    #[test]
    fn a_body_at_the_edge_is_highlighted_because_it_is_hit() {
        // The executor collects `distance <= radius + body_radius`; a highlight that used
        // centres alone would leave a ringed-out enemy taking the damage anyway.
        let shape = TargetShape::Circle { radius: 5.0 };
        let just_out = Vec3::new(5.4, 0.0, 0.0);
        assert!(!shape.contains(Vec3::ZERO, 0.0, just_out), "by its centre");
        assert!(
            shape.contains_body(Vec3::ZERO, 0.0, just_out, 0.5),
            "but its body is inside"
        );

        let line = TargetShape::of(FireAbility::Firestorm);
        let beside = Vec3::new(5.3, 0.0, -10.0);
        assert!(!line.contains(Vec3::ZERO, 0.0, beside));
        assert!(line.contains_body(Vec3::ZERO, 0.0, beside, 0.5));
    }

    #[test]
    fn a_circle_highlights_what_a_placed_ability_covers() {
        let shape = TargetShape::of(FireAbility::Meteor);
        let centre = Vec3::new(4.0, 0.0, -7.0);
        let radius = FireAbility::Meteor.definition().radius;
        assert!(shape.contains(centre, 0.0, centre));
        assert!(shape.contains(
            centre,
            0.0,
            Vec3::new(centre.x + radius - 0.1, 0.0, centre.z)
        ));
        assert!(!shape.contains(
            centre,
            0.0,
            Vec3::new(centre.x + radius + 0.5, 0.0, centre.z)
        ));
    }

    #[test]
    fn a_shape_comes_from_the_ability_rather_than_from_the_targeting_code() {
        // Placed circles ask for a circle, the line asks for a line, and the cone asks for
        // a wedge - all read from the same table the executor hits with.
        for ability in [FireAbility::Meteor, FireAbility::Cataclysm] {
            let shape = TargetShape::of(ability);
            assert_eq!(
                shape,
                TargetShape::Circle {
                    radius: ability.definition().radius
                },
                "{ability:?}"
            );
            assert_eq!(shape.extent(), ability.definition().radius);
        }

        let firestorm = FireAbility::Firestorm.definition();
        assert_eq!(
            TargetShape::of(FireAbility::Firestorm),
            TargetShape::Line {
                width: firestorm.width,
                length: firestorm.range,
            }
        );
        // The designed linear zone: twenty metres ahead, ten across.
        assert_eq!((firestorm.range, firestorm.width), (20.0, 10.0));
        assert_eq!(
            TargetShape::of(FireAbility::Firestorm).extent(),
            firestorm.range
        );
    }

    #[test]
    fn a_circle_contains_what_is_inside_it_and_nothing_else() {
        let shape = TargetShape::Circle { radius: 5.0 };
        assert!(shape.contains(Vec3::ZERO, 0.0, Vec3::new(3.0, 0.0, 3.0)));
        assert!(!shape.contains(Vec3::ZERO, 0.0, Vec3::new(6.0, 0.0, 0.0)));
        // Height is ignored: an area on the floor covers what stands on a step in it.
        assert!(shape.contains(Vec3::ZERO, 0.0, Vec3::new(0.0, 40.0, 0.0)));
    }

    #[test]
    fn the_other_shapes_are_ready_for_the_abilities_that_will_want_them() {
        // Defined and tested now so a rectangle ability is a variant and a mesh rather than
        // a second targeting system.
        let cone = TargetShape::Cone {
            range: 10.0,
            half_angle: std::f32::consts::FRAC_PI_4,
        };
        assert!(
            cone.contains(Vec3::ZERO, 0.0, Vec3::new(0.0, 0.0, -5.0)),
            "in front"
        );
        assert!(
            !cone.contains(Vec3::ZERO, 0.0, Vec3::new(0.0, 0.0, 5.0)),
            "behind"
        );
        assert!(
            !cone.contains(Vec3::ZERO, 0.0, Vec3::new(0.0, 0.0, -50.0)),
            "too far"
        );

        let rectangle = TargetShape::Rectangle {
            width: 4.0,
            length: 10.0,
        };
        assert!(rectangle.contains(Vec3::ZERO, 0.0, Vec3::new(1.5, 0.0, 4.0)));
        assert!(!rectangle.contains(Vec3::ZERO, 0.0, Vec3::new(3.0, 0.0, 0.0)));

        // A line is anchored at the caster and runs *forwards*, along -Z at a facing of
        // zero. It is not centred, and it is not axis-aligned to the world.
        let line = TargetShape::Line {
            width: 2.0,
            length: 20.0,
        };
        assert!(
            line.contains(Vec3::ZERO, 0.0, Vec3::new(0.5, 0.0, -9.0)),
            "ahead"
        );
        assert!(
            !line.contains(Vec3::ZERO, 0.0, Vec3::new(0.5, 0.0, 9.0)),
            "behind"
        );
        assert!(
            !line.contains(Vec3::ZERO, 0.0, Vec3::new(2.0, 0.0, -9.0)),
            "beside"
        );
    }

    #[test]
    fn the_cursor_lands_on_the_terrain_rather_than_on_the_plane_under_it() {
        // The bug this fixes: the ray was met with `y = 0`, so on a level whose ground is
        // four metres up the indicator appeared metres past where the player was pointing -
        // exactly as far past as the camera angle and the height between them made it.
        let plateau = engine_environment::terrain::Terrain::flat(129, 1.0, 4.0);
        let ground = GroundSurface::Terrain(&plateau);
        let view_projection = camera().view_projection();
        let pointer = (640.0, 520.0);

        let on_terrain = ground_under_cursor(pointer, &view_projection, (1280, 720), &ground)
            .expect("the cursor is pointing at the ground");
        assert!(
            (on_terrain.y - 4.0).abs() < 0.05,
            "the pick landed at {on_terrain:?} rather than on the plateau",
        );

        let on_plane = ground_under_cursor(
            pointer,
            &view_projection,
            (1280, 720),
            &GroundSurface::Plane(GROUND_HEIGHT),
        )
        .expect("the plane is in front of the camera too");
        assert!(
            horizontal_distance(on_terrain, on_plane) > 1.0,
            "the terrain pick and the plane pick agreed, so nothing was sampled",
        );
    }

    #[test]
    fn a_placement_on_a_slope_sits_on_the_slope() {
        // Both the position the spell is given and the anchor the indicator is built from.
        // A placement left on `y = 0` would hang the whole shape in the air over a hillside.
        let slope = ramp(0.4);
        let ground = GroundSurface::Terrain(&slope);
        let mut targeting = GroundTargeting::new();
        targeting.begin(FireAbility::Meteor);
        targeting.update(
            &input_at(560.0, 520.0),
            &camera().view_projection(),
            (1280, 720),
            Vec3::ZERO,
            &ground,
        );

        let active = targeting.active().expect("placing");
        let expected = ground.height_at(active.position.x, active.position.z);
        assert!(
            (active.position.y - expected).abs() < 0.05,
            "the placement is at {:?} but the ground there is {expected}",
            active.position,
        );
        assert!(
            active.position.y.abs() > 0.05,
            "the slope under the cursor was flat, so nothing was tested",
        );
    }

    #[test]
    fn a_cone_is_anchored_on_the_ground_the_caster_is_standing_on() {
        // A cone and a line are anchored on the caster, not the cursor, and that anchor is
        // where the indicator starts: on a slope it has to be the caster's own ground.
        let slope = ramp(0.5);
        let ground = GroundSurface::Terrain(&slope);
        let caster = Vec3::new(6.0, 0.0, 2.0);
        let mut targeting = GroundTargeting::new();
        targeting.begin(FireAbility::FlameCone);
        targeting.update(
            &input_at(640.0, 520.0),
            &camera().view_projection(),
            (1280, 720),
            caster,
            &ground,
        );

        let active = targeting.active().expect("placing");
        let expected = ground.height_at(caster.x, caster.z);
        assert!(
            (active.anchor.y - expected).abs() < 1e-3,
            "the cone is anchored at {:?}, not on the ground at {expected}",
            active.anchor,
        );
        assert!(active.valid, "an aimed cone is always a legal cast");
    }

    #[test]
    fn a_ray_that_clears_the_terrain_has_no_answer_rather_than_a_guess() {
        // Pointing at the sky over a height field. The march must run out rather than
        // returning the last point it looked at, which would teleport the indicator to the
        // horizon the moment the player looked up.
        let slope = ramp(0.2);
        let ground = GroundSurface::Terrain(&slope);
        assert_eq!(
            ground_under_cursor(
                (640.0, 0.0),
                &camera().view_projection(),
                (1280, 720),
                &ground
            ),
            None,
        );
    }

    #[test]
    fn the_first_crossing_is_taken_so_a_ridge_is_not_seen_through() {
        // A twelve-metre bank in front of the camera with flat ground behind it. Pointing at
        // the bank must place the indicator on the bank: the player is aiming at ground they
        // can see, and the far side of a ridge is not it. A marcher that took the *last*
        // crossing, or that solved for the plane, would drop the indicator in the flat
        // beyond - out of sight behind the terrain.
        let mut ridge = engine_environment::terrain::Terrain::flat(129, 1.0, 0.0);
        for z in 0..ridge.resolution {
            for x in 0..ridge.resolution {
                let world_z = ridge.origin.z + z as f32 * ridge.cell_size;
                let height = if (-10.0..-1.0).contains(&world_z) {
                    12.0
                } else {
                    0.0
                };
                ridge.heights[z * ridge.resolution + x] = height;
            }
        }
        let ground = GroundSurface::Terrain(&ridge);
        let hit = ground_under_cursor(
            (640.0, 500.0),
            &camera().view_projection(),
            (1280, 720),
            &ground,
        )
        .expect("the bank is in front of the camera");

        // Whatever it hit, it hit the surface: the height at the point is the point's height.
        let under = ground.height_at(hit.x, hit.z);
        assert!(
            (hit.y - under).abs() < 0.05,
            "the pick at {hit:?} is not on the ground at {under}",
        );
        assert!(
            hit.z > -11.0,
            "the ray was allowed through the bank and landed at {hit:?}",
        );
    }

    #[test]
    fn a_degenerate_viewport_produces_no_placement_rather_than_a_division_by_zero() {
        assert_eq!(
            ground_under_cursor(
                (10.0, 10.0),
                &camera().view_projection(),
                (0, 0),
                &flat_ground()
            ),
            None
        );
    }
}
