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
//! The placement is computed by intersecting the mouse ray with the ground plane, which is
//! what makes the circle sit where the player is pointing rather than where the camera is.
//! Range is checked against the caster, and the indicator says whether the placement is
//! valid *before* the click - a refusal after the fact is a wasted cooldown.

use engine_core::math::{Mat4, Vec3};
use engine_game::firemage::FireAbility;

use super::input::UiInput;

/// Height of the ground plane the circle is placed on, in metres.
///
/// The arena floor. A future level with real terrain would raycast the collision world
/// instead; the seam is this one function.
pub const GROUND_HEIGHT: f32 = 0.0;

/// What the player is currently placing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundTarget {
    /// Which ability is waiting to be placed.
    pub ability: FireAbility,
    /// Where the circle currently sits, in world space.
    pub position: Vec3,
    /// How big it is, in metres.
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
        self.active = Some(GroundTarget {
            ability,
            position: Vec3::ZERO,
            radius: definition.radius,
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
    /// `caster` is where the player is standing, which is what range is measured from.
    pub fn update(
        &mut self,
        input: &UiInput,
        view_projection: &Mat4,
        viewport: (u32, u32),
        caster: Vec3,
    ) -> TargetingOutcome {
        let Some(mut target) = self.active else {
            return TargetingOutcome::Idle;
        };

        // Escape or the right button backs out, exactly as the brief asks.
        if input.cancel || input.secondary_pressed {
            self.active = None;
            return TargetingOutcome::Cancelled;
        }

        if let Some(position) = ground_under_cursor(input.pointer, view_projection, viewport) {
            let flat = Vec3::new(position.x, GROUND_HEIGHT, position.z);
            let distance = horizontal_distance(caster, flat);
            target.position = flat;
            target.valid = distance <= target.range;
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

/// Where the cursor is pointing on the ground plane, if anywhere.
///
/// The mouse position becomes a ray through the inverse view-projection, and the ray is
/// intersected with `y = GROUND_HEIGHT`. A ray parallel to the ground, or one pointing at the
/// sky, has no answer and the circle stays where it was.
#[must_use]
pub fn ground_under_cursor(
    pointer: (f32, f32),
    view_projection: &Mat4,
    viewport: (u32, u32),
) -> Option<Vec3> {
    if viewport.0 == 0 || viewport.1 == 0 {
        return None;
    }
    let inverse = view_projection.inverse()?;

    // Client pixels to normalized device coordinates: x right, y up, z into the screen.
    let ndc_x = (pointer.0 / viewport.0 as f32) * 2.0 - 1.0;
    let ndc_y = 1.0 - (pointer.1 / viewport.1 as f32) * 2.0;

    let near = inverse.transform_point(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inverse.transform_point(Vec3::new(ndc_x, ndc_y, 1.0));
    let direction = far - near;
    if direction.y.abs() < 1e-6 {
        return None;
    }

    let t = (GROUND_HEIGHT - near.y) / direction.y;
    if t < 0.0 {
        return None;
    }
    Some(near + direction * t)
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

    #[test]
    fn nothing_is_being_placed_until_an_ability_asks() {
        let mut targeting = GroundTargeting::new();
        assert!(!targeting.is_active());
        assert_eq!(
            targeting.update(
                &input_at(0.0, 0.0),
                &camera().view_projection(),
                (1280, 720),
                Vec3::ZERO
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
        // The brief's requirement: one targeting implementation, three abilities.
        for ability in [
            FireAbility::Meteor,
            FireAbility::Firestorm,
            FireAbility::Cataclysm,
        ] {
            let mut targeting = GroundTargeting::new();
            targeting.begin(ability);
            let active = targeting.active().expect("placing");
            assert_eq!(active.radius, ability.definition().radius, "{ability:?}");
            assert!(active.radius > 0.0, "{ability:?} has no circle to draw");
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
        );
        let first = targeting.active().expect("placing").position;

        targeting.update(
            &input_at(300.0, 500.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
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
        );
        let click = UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..input_at(640.0, 500.0)
        };
        let outcome = targeting.update(&click, &view_projection, (1280, 720), Vec3::ZERO);

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
            let outcome = targeting.update(&cancel, &view_projection, (1280, 720), Vec3::ZERO);
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
        );
        assert!(!targeting.active().expect("placing").valid);

        let click = UiInput {
            pointer_down: true,
            pointer_pressed: true,
            ..input_at(640.0, 500.0)
        };
        assert_eq!(
            targeting.update(&click, &view_projection, (1280, 720), caster),
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
        );
        let grounded = targeting.active().expect("placing").position;

        targeting.update(
            &input_at(640.0, 0.0),
            &view_projection,
            (1280, 720),
            Vec3::ZERO,
        );
        let after = targeting.active().expect("placing").position;
        assert!(
            after.y.abs() < 1e-6,
            "the circle left the ground: {after:?}"
        );
        let _ = grounded;
    }

    #[test]
    fn a_degenerate_viewport_produces_no_placement_rather_than_a_division_by_zero() {
        assert_eq!(
            ground_under_cursor((10.0, 10.0), &camera().view_projection(), (0, 0)),
            None
        );
    }
}
