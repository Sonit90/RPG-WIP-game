//! The third-person camera controller.
//!
//! ```text
//!          eye
//!            \   distance
//!             \
//!              \  pitch
//!               v
//!             focus  = player feet + height
//!               |
//!             (P)
//! ```
//!
//! A *controller*, like `engine-showcase`'s free-fly camera: it reads intent and drives an
//! [`engine_render_data::Camera`] by setting a position and a rotation. The camera itself
//! stays a view and a projection and learns nothing about a player.
//!
//! It lives in presentation rather than in `engine-game` deliberately. Where the camera is
//! does not change what the simulation does — it only changes what movement means, and that
//! is carried in the command as a yaw. A future spectator, replay or cinematic camera is
//! another controller against the same seam and requires no gameplay change.
//!
//! ## Keeping out of walls
//!
//! ```text
//!   anchor (character's chest, unsmoothed)
//!     |  sweep 1: anchor -> focus      the smoothed focus may cut a corner; the pivot may not
//!   pivot
//!     |  sweep 2: pivot -> desired eye, as a sphere of `radius`
//!     |
//!     o  eye      <- stops `padding` short of whatever the sphere met
//!     X  wall
//!     .  desired eye
//! ```
//!
//! [`ThirdPersonCamera::avoid_obstructions`] asks the simulation's own collision world — the
//! same shapes the character collides with — how far back the eye can sit, and shortens the
//! orbit to that. Nothing is hidden, faded or drawn differently: the camera simply never
//! goes where the world is solid. Pulling in is immediate; easing back out once the way is
//! clear runs at [`CameraCollision::recovery_rate`]. The orbit the player chose with the
//! wheel, [`ThirdPersonCamera::distance`], is never changed by any of it.

use engine_collision::shape::Segment;
use engine_collision::{CollisionMask, CollisionWorld, Ray, StaticId};
use engine_core::math::{Quat, Vec3};
use engine_core::tuning;
use engine_render_data::error::RenderDataError;
use engine_render_data::{
    Camera, CameraFade, DEFAULT_FAR, DEFAULT_FOV_Y, DEFAULT_NEAR, Projection,
};

/// Radians of rotation per pixel of mouse movement, before the player's setting.
///
/// Half of the 0.0028 the camera shipped with: at that rate a hand-sized mouse movement
/// spun the camera further than a player could follow. Both axes use it, so a turn and a
/// tilt of the same size on the desk are the same size on screen.
///
/// The tuned rate, not the rate in use: what the camera actually turns at is this times
/// [`ThirdPersonCamera::sensitivity`], which the settings screen drives. A sensitivity of
/// one means exactly this, which is why the default settings file changes nothing.
pub const BASE_MOUSE_SENSITIVITY: f32 = 0.0014;

/// Lowest the camera may look from, in radians. Negative looks down at the character.
const MIN_PITCH: f32 = -1.2;

/// Highest the camera may look from. Short of straight up, where yaw and view coincide and
/// the image rolls as the mouse moves sideways.
const MAX_PITCH: f32 = 1.1;

/// Closest the camera may orbit, in metres.
const MIN_DISTANCE: f32 = 2.5;

/// Furthest the camera may orbit, in metres.
const MAX_DISTANCE: f32 = 25.0;

/// Metres of orbit distance per wheel notch.
const ZOOM_PER_NOTCH: f32 = 1.2;

/// Where the camera looks relative to the character's feet, in metres.
///
/// Roughly the chest: focusing on the feet puts the character in the top half of the screen
/// and the floor in the other half, which is where a third-person camera looks wrong
/// without being obviously misconfigured.
const FOCUS_HEIGHT: f32 = 1.4;

/// How quickly the focus catches up with the character, in units of e-folds per second.
///
/// The camera lags the character slightly, which is what makes movement read as momentum
/// rather than as the world sliding. Frame-rate independent: the smoothing is exponential
/// in elapsed time, not a fraction per frame — a per-frame fraction would follow six times
/// as fast at 360 FPS as at 60.
const FOLLOW_RATE: f32 = 12.0;

/// Below this many metres a held distance counts as back at the player's zoom.
const RECOVERED_EPSILON: f32 = 1e-3;

/// How the camera keeps out of solid geometry.
///
/// The defaults come from [`engine_core::tuning::camera`]; the fields are public so a test
/// or a tool can try a different camera without editing the tuning file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraCollision {
    /// Radius of the sphere the eye is swept as, in metres.
    pub radius: f32,
    /// Extra gap kept between that sphere and the surface it stopped at, in metres.
    pub padding: f32,
    /// Closest the eye is pulled towards the pivot when there is room, in metres.
    pub min_distance: f32,
    /// How quickly the eye returns to the player's zoom once clear, in e-folds per second.
    pub recovery_rate: f32,
    /// What the camera treats as solid. The world layer: what the character itself
    /// collides with. Sight-only barriers and dummies are not walls to a camera.
    pub mask: CollisionMask,
}

impl CameraCollision {
    /// The tuned camera.
    pub const DEFAULT: Self = Self {
        radius: tuning::camera::COLLISION_RADIUS,
        padding: tuning::camera::COLLISION_PADDING,
        min_distance: tuning::camera::COLLISION_MIN_DISTANCE,
        recovery_rate: tuning::camera::COLLISION_RECOVERY_RATE,
        mask: CollisionMask::WORLD,
    };
}

impl Default for CameraCollision {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// What stopped the camera on the last frame it checked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraHit {
    /// Where the sweep met the surface.
    pub point: Vec3,
    /// Unit normal out of that surface, towards the camera's pivot.
    pub normal: Vec3,
    /// The collision object that was in the way.
    pub id: StaticId,
}

/// Everything the last obstruction check decided, kept for the developer view.
///
/// A record of the query that ran, not a fresh one: what it explains is the frame the
/// player just saw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraProbe {
    /// The character's chest, unsmoothed: where the camera is anchored.
    pub anchor: Vec3,
    /// Where the sweep towards the eye started. The smoothed focus, unless it was behind
    /// geometry as seen from the anchor.
    pub pivot: Vec3,
    /// Where the eye would be with nothing in the way.
    pub desired_eye: Vec3,
    /// Where the eye actually is.
    pub eye: Vec3,
    /// The orbit distance the geometry allowed this frame, before easing back out.
    pub limit: f32,
    /// What the sweep met, if anything.
    pub hit: Option<CameraHit>,
}

impl CameraProbe {
    /// Whether anything was in the way this frame.
    #[must_use]
    pub fn obstructed(&self) -> bool {
        self.hit.is_some()
    }
}

/// An orbiting follow camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPersonCamera {
    /// Rotation about +Y, radians. Zero looks down −Z.
    pub yaw: f32,
    /// Rotation about the camera's right, radians. Negative looks down.
    pub pitch: f32,
    /// How far behind the character the eye sits, metres.
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// How fast the mouse turns the camera, as a multiple of [`BASE_MOUSE_SENSITIVITY`].
    ///
    /// The player's setting, handed in rather than read: this crate does not know that a
    /// settings file exists, and the camera is not the authority on what the value is.
    sensitivity: f32,
    /// The smoothed point the camera looks at.
    focus: Vec3,
    /// How the camera keeps out of walls.
    pub collision: CameraCollision,
    /// The pivot the last obstruction check settled on, relative to the focus. Zero unless
    /// the focus had cut behind geometry.
    pivot_offset: Vec3,
    /// The orbit distance the camera is held at by geometry, or infinity when it is free to
    /// sit at [`ThirdPersonCamera::distance`].
    ///
    /// Infinity rather than a copy of the zoom, so the wheel still zooms instantly when
    /// nothing is in the way: only a camera that was pushed in eases back out.
    held_distance: f32,
    /// The last obstruction check, for the developer view.
    probe: Option<CameraProbe>,
    /// How much of the followed character is drawn, eased. See
    /// [`ThirdPersonCamera::character_opacity`].
    character_opacity: f32,
}

impl Default for ThirdPersonCamera {
    fn default() -> Self {
        Self::START
    }
}

impl ThirdPersonCamera {
    /// Behind and slightly above the character, looking down at it.
    pub const START: Self = Self {
        yaw: 0.0,
        pitch: -0.22,
        distance: 8.0,
        fov_y: DEFAULT_FOV_Y,
        sensitivity: 1.0,
        focus: Vec3::ZERO,
        collision: CameraCollision::DEFAULT,
        pivot_offset: Vec3::ZERO,
        held_distance: f32::INFINITY,
        probe: None,
        character_opacity: 1.0,
    };

    /// A camera already snapped to a character at `position`.
    #[must_use]
    pub fn following(position: Vec3) -> Self {
        let mut camera = Self::START;
        camera.snap_to(position);
        camera
    }

    /// How fast the mouse turns the camera, as a multiple of the tuned rate.
    #[must_use]
    pub fn sensitivity(&self) -> f32 {
        self.sensitivity
    }

    /// Sets the turn rate multiplier.
    ///
    /// Applies from the next mouse movement: there is no state to rebuild and nothing to
    /// restart, which is what makes dragging the settings slider change the camera while
    /// the slider is still being dragged. A value that is not a positive, finite number is
    /// ignored rather than freezing or inverting the camera.
    pub fn set_sensitivity(&mut self, sensitivity: f32) {
        if sensitivity.is_finite() && sensitivity > 0.0 {
            self.sensitivity = sensitivity;
        }
    }

    /// The point the camera is looking at.
    #[must_use]
    pub fn focus(&self) -> Vec3 {
        self.focus
    }

    /// The camera's orientation.
    #[must_use]
    pub fn rotation(&self) -> Quat {
        Quat::from_yaw_pitch_roll(self.yaw, self.pitch, 0.0)
    }

    /// Where the eye is, in world space.
    ///
    /// Behind the pivot along the view direction, which is what makes zooming and pitching
    /// orbit the character rather than move it across the screen. The pivot is the focus,
    /// and the distance is the zoom, unless [`ThirdPersonCamera::avoid_obstructions`] found
    /// geometry in the way.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.pivot() + self.backward() * self.actual_distance()
    }

    /// The point the eye orbits: the focus, unless the focus had cut behind geometry.
    #[must_use]
    pub fn pivot(&self) -> Vec3 {
        self.focus + self.pivot_offset
    }

    /// How far the eye actually is from the pivot: the zoom, or less while a wall is in the
    /// way or the camera is easing back out from one.
    #[must_use]
    pub fn actual_distance(&self) -> f32 {
        self.distance.min(self.held_distance)
    }

    /// What the last obstruction check found, for the developer view. `None` until
    /// [`ThirdPersonCamera::avoid_obstructions`] has run.
    #[must_use]
    pub fn probe(&self) -> Option<&CameraProbe> {
        self.probe.as_ref()
    }

    /// How much of the followed character should be drawn, from 1 (all of it) to 0.
    ///
    /// Fades as the eye closes on the character's body — which only happens when a wall has
    /// pulled the camera in — so the player looks *through* their own character rather than
    /// at the inside of its head. Updated by [`ThirdPersonCamera::avoid_obstructions`].
    #[must_use]
    pub fn character_opacity(&self) -> f32 {
        self.character_opacity
    }

    /// What the renderer should fade out of the camera's way this frame: see-through things
    /// right in front of the eye, or between the eye and the character.
    ///
    /// `None` until [`ThirdPersonCamera::avoid_obstructions`] has run, because until then
    /// the camera does not know where the character is.
    #[must_use]
    pub fn fade(&self) -> Option<CameraFade> {
        self.probe.map(|probe| CameraFade {
            target: probe.anchor,
            near_start: tuning::camera::NEAR_FADE_START,
            near_end: tuning::camera::NEAR_FADE_END,
            radius: tuning::camera::OCCLUDER_FADE_RADIUS,
            min_opacity: tuning::camera::OCCLUDER_MIN_OPACITY,
        })
    }

    /// Unit vector from the pivot towards the eye.
    fn backward(&self) -> Vec3 {
        self.rotation().rotate(Vec3::Z)
    }

    /// Applies a mouse movement, in pixels.
    ///
    /// Screen Y grows downward, so moving the mouse down lowers the pitch.
    pub fn rotate(&mut self, delta_x: f32, delta_y: f32) {
        let rate = BASE_MOUSE_SENSITIVITY * self.sensitivity;
        self.yaw = (self.yaw - delta_x * rate).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - delta_y * rate).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Turns the orbit by `radians` of yaw, independent of the mouse sensitivity.
    ///
    /// What a benchmark's spinning camera uses: a fixed angular rate, not a pixel count.
    pub fn spin(&mut self, radians: f32) {
        self.yaw = (self.yaw + radians).rem_euclid(std::f32::consts::TAU);
    }

    /// Applies wheel notches as an orbit distance change.
    ///
    /// Rolling away from the user pulls the camera in, which is what "zoom in" means.
    pub fn zoom(&mut self, notches: f32) {
        self.distance =
            (self.distance - notches * ZOOM_PER_NOTCH).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Puts the focus exactly on the character, with no smoothing.
    ///
    /// For the first frame and after a teleport: smoothing from wherever the camera was
    /// would fly it across the arena.
    pub fn snap_to(&mut self, position: Vec3) {
        self.focus = position + Vec3::new(0.0, FOCUS_HEIGHT, 0.0);
        // Whatever held the camera in belongs to where it was. The next obstruction check
        // places it against the walls where it is now.
        self.pivot_offset = Vec3::ZERO;
        self.held_distance = f32::INFINITY;
    }

    /// Moves the focus towards the character for one frame.
    ///
    /// Driven by the *render* delta rather than the simulation step, and that is correct:
    /// the camera is presentation, it is expected to move every frame, and interpolating it
    /// at the render rate is what stops a 60 Hz simulation from looking like 60 FPS.
    pub fn follow(&mut self, position: Vec3, seconds: f32) {
        let target = position + Vec3::new(0.0, FOCUS_HEIGHT, 0.0);
        if seconds <= 0.0 {
            self.focus = target;
            return;
        }
        // Exponential smoothing: the fraction covered depends on elapsed time, so the
        // camera behaves identically at 60 and at 1400 FPS.
        let blend = 1.0 - (-FOLLOW_RATE * seconds).exp();
        self.focus = self.focus.lerp(target, blend.clamp(0.0, 1.0));
    }

    /// Keeps the eye out of solid geometry for one frame.
    ///
    /// Called every frame after [`ThirdPersonCamera::follow`] (or
    /// [`ThirdPersonCamera::snap_to`]) with the same character position, and before
    /// [`ThirdPersonCamera::apply`]. `seconds` is the render delta; zero — the first frame,
    /// a teleport — places the camera at the allowed distance with no easing.
    ///
    /// Two sweeps of the camera's sphere through `world`, both read-only:
    ///
    /// 1. From the character's unsmoothed chest to the smoothed focus. The focus lags the
    ///    character, and a lag around a corner puts it on the far side of the wall the
    ///    character just walked past; the pivot stops where the sweep does instead.
    /// 2. From the pivot back along the view to where the eye wants to be. The eye stops
    ///    [`CameraCollision::padding`] short of whatever the sphere meets.
    ///
    /// When even [`CameraCollision::min_distance`] is not clear for the sphere, the eye still
    /// goes no further than the centre ray allows minus the padding, so it is never inside
    /// or behind a surface — only closer to it than the sphere would like.
    ///
    /// The rotation is never touched: yaw and pitch remain the player's, so a wall can shorten
    /// the orbit but can never turn or flip the view.
    pub fn avoid_obstructions(&mut self, world: &CollisionWorld, character: Vec3, seconds: f32) {
        let config = self.collision;
        let anchor = character + Vec3::new(0.0, FOCUS_HEIGHT, 0.0);
        let radius = config.radius.max(0.0);
        let padding = config.padding.max(0.0);

        // Sweep 1: may the pivot be where the focus is?
        let pivot = match Ray::between(anchor, self.focus) {
            Some((ray, length)) => match world.sweep_sphere(&ray, radius, length, config.mask) {
                Some(hit) => ray.at((hit.distance - padding).max(0.0)),
                None => self.focus,
            },
            None => self.focus,
        };

        // Sweep 2: how far back may the eye go?
        let backward = self.backward();
        let desired = self.distance;
        let mut limit = desired;
        let mut hit = None;
        if let Some(ray) = Ray::new(pivot, backward) {
            if let Some(found) = world.sweep_sphere(&ray, radius, desired + padding, config.mask) {
                limit = (found.distance - padding).clamp(0.0, desired);
                hit = Some(CameraHit {
                    point: found.point,
                    normal: found.normal,
                    id: found.id,
                });
            }
            if limit < config.min_distance {
                // Boxed in: the sphere has no room, but the eye need not sit inside the
                // character's head while its centre still has the surface in front of it.
                let centre = world
                    .raycast(&ray, desired + padding, config.mask)
                    .map_or(desired, |ray_hit| (ray_hit.hit.distance - padding).max(0.0));
                limit = limit.max(config.min_distance.min(centre).min(desired));
            }
        }

        // In at once, out gently. Never further out than the limit: easing only ever covers
        // ground the sweep has just said is clear.
        let current = self.actual_distance();
        let next = if limit <= current || seconds <= 0.0 {
            limit
        } else {
            let blend = 1.0 - (-config.recovery_rate.max(0.0) * seconds).exp();
            let eased = current + (limit - current) * blend.clamp(0.0, 1.0);
            if limit - eased <= RECOVERED_EPSILON {
                limit
            } else {
                eased
            }
        };
        self.held_distance = if next >= desired - RECOVERED_EPSILON {
            f32::INFINITY
        } else {
            next
        };
        self.pivot_offset = pivot - self.focus;

        // The character fades by how close the eye now is to its body.
        let target = character_opacity_at(self.eye(), character);
        self.character_opacity = if seconds <= 0.0 {
            target
        } else {
            let blend = 1.0 - (-tuning::camera::CHARACTER_FADE_RATE * seconds).exp();
            let eased =
                self.character_opacity + (target - self.character_opacity) * blend.clamp(0.0, 1.0);
            // Snapped once it is close. An exponential ease never arrives on its own: in f32 it
            // stalls one step short of 1.0, and a character at 0.99999994 is a character the
            // renderer still draws see-through — in two passes, every frame, for good.
            if (target - eased).abs() <= RECOVERED_EPSILON {
                target
            } else {
                eased
            }
        };

        self.probe = Some(CameraProbe {
            anchor,
            pivot,
            desired_eye: pivot + backward * desired,
            eye: self.eye(),
            limit,
            hit,
        });
    }

    /// Writes this controller's state into a renderer camera.
    ///
    /// # Errors
    ///
    /// Returns [`RenderDataError::InvalidConfig`] if the aspect ratio is degenerate, which
    /// a minimized window produces.
    pub fn apply(&self, camera: &mut Camera, aspect: f32) -> Result<(), RenderDataError> {
        camera.set_projection(Projection::Perspective {
            fov_y: self.fov_y,
            aspect,
            near: DEFAULT_NEAR,
            far: DEFAULT_FAR,
        })?;
        camera.set_position(self.eye());
        camera.set_rotation(self.rotation());
        Ok(())
    }
}

/// How much of a character standing at `character` should be drawn from an eye at `eye`.
///
/// Measured to the body — the character's collision capsule, the same shape the camera's
/// focus height is chosen against — rather than to its origin, so a camera looking down on
/// the head and one level with the chest fade at the same gap.
#[must_use]
pub fn character_opacity_at(eye: Vec3, character: Vec3) -> f32 {
    let radius = tuning::character::RADIUS;
    let spine = Segment::new(
        character + Vec3::new(0.0, radius, 0.0),
        character + Vec3::new(0.0, tuning::character::HEIGHT - radius, 0.0),
    );
    let gap = (eye - spine.closest_point(eye)).length() - radius;
    let hidden = tuning::camera::CHARACTER_FADE_HIDDEN;
    let whole = tuning::camera::CHARACTER_FADE_WHOLE;
    let t = ((gap - hidden) / (whole - hidden)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-3;

    fn camera() -> Camera {
        Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection")
    }

    #[test]
    fn the_eye_sits_behind_and_above_the_character() {
        let controller = ThirdPersonCamera::following(Vec3::ZERO);
        let eye = controller.eye();
        assert!(eye.z > 0.0, "the camera is behind a character facing -Z");
        assert!(eye.y > 0.0, "and above it");
        assert!((eye.x).abs() < EPSILON);
        assert!(
            (eye - controller.focus()).length() - controller.distance < EPSILON,
            "the eye is exactly `distance` from the focus"
        );
    }

    #[test]
    fn the_camera_looks_at_the_character() {
        let controller = ThirdPersonCamera::following(Vec3::new(3.0, 0.0, -4.0));
        let mut view = camera();
        controller
            .apply(&mut view, 16.0 / 9.0)
            .expect("a valid aspect");

        // The focus must land in front of the camera, on its axis.
        let seen = view.view().transform_point(controller.focus());
        assert!(seen.x.abs() < EPSILON && seen.y.abs() < EPSILON, "{seen:?}");
        assert!(seen.z < 0.0, "the focus is behind the camera: {seen:?}");
    }

    #[test]
    fn yaw_orbits_the_character_rather_than_moving_it() {
        let mut controller = ThirdPersonCamera::following(Vec3::new(10.0, 0.0, 10.0));
        let focus = controller.focus();
        let before = controller.eye();
        controller.rotate(200.0, 0.0);

        assert_eq!(controller.focus(), focus, "the focus moved");
        assert!(
            ((controller.eye() - focus).length() - (before - focus).length()).abs() < EPSILON,
            "the orbit radius changed"
        );
        assert!((controller.eye() - before).length() > 0.5, "nothing moved");
    }

    #[test]
    fn the_mouse_sensitivity_is_half_what_it_used_to_be() {
        /// What the camera turned at before it was halved.
        const PREVIOUS: f32 = 0.0028;
        assert!(
            (BASE_MOUSE_SENSITIVITY - PREVIOUS * 0.5).abs() < f32::EPSILON,
            "{BASE_MOUSE_SENSITIVITY} is not half of {PREVIOUS}"
        );
    }

    #[test]
    fn the_sensitivity_setting_scales_the_tuned_rate() {
        // A setting of one must leave the camera exactly as it was tuned, or every existing
        // measurement of how the camera feels stops meaning anything. Driven with a negative
        // delta so yaw grows from zero and never wraps around.
        let mut tuned = ThirdPersonCamera::START;
        assert_eq!(tuned.sensitivity(), 1.0);
        tuned.rotate(-100.0, 0.0);
        assert!(
            (tuned.yaw - 100.0 * BASE_MOUSE_SENSITIVITY).abs() < EPSILON,
            "{}",
            tuned.yaw
        );

        let mut doubled = ThirdPersonCamera::START;
        doubled.set_sensitivity(2.0);
        doubled.rotate(-100.0, 0.0);
        assert!(
            (doubled.yaw - 2.0 * tuned.yaw).abs() < EPSILON,
            "twice the sensitivity turned {} rather than {}",
            doubled.yaw,
            2.0 * tuned.yaw
        );

        // Vertically too: one setting drives both axes.
        let mut halved = ThirdPersonCamera::START;
        halved.set_sensitivity(0.5);
        halved.rotate(0.0, -100.0);
        let expected = ThirdPersonCamera::START.pitch + 100.0 * BASE_MOUSE_SENSITIVITY * 0.5;
        assert!(
            (halved.pitch - expected).abs() < EPSILON,
            "{}",
            halved.pitch
        );
    }

    #[test]
    fn a_new_sensitivity_applies_to_the_very_next_movement() {
        // What "the setting applies without restarting" means in code: no rebuild, no
        // restart, the next mouse movement already turns at the new rate.
        let mut controller = ThirdPersonCamera::START;
        controller.rotate(-50.0, 0.0);
        let first = controller.yaw;

        controller.set_sensitivity(4.0);
        controller.rotate(-50.0, 0.0);
        let second = controller.yaw - first;
        assert!(
            (second - 4.0 * first).abs() < EPSILON,
            "the second turn was {second}, not four times {first}"
        );
    }

    #[test]
    fn a_meaningless_sensitivity_is_refused_rather_than_freezing_the_camera() {
        let mut controller = ThirdPersonCamera::START;
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            controller.set_sensitivity(bad);
            assert_eq!(controller.sensitivity(), 1.0, "{bad} was accepted");
        }
    }

    #[test]
    fn both_axes_turn_at_the_reduced_sensitivity() {
        // The rate itself, not only the constant: a rotation that scaled one axis
        // differently would pass the constant's test and still turn at the old speed.
        let mut controller = ThirdPersonCamera::START;
        controller.rotate(100.0, 100.0);
        assert!(
            (controller.yaw - (ThirdPersonCamera::START.yaw - 100.0 * 0.0014))
                .rem_euclid(std::f32::consts::TAU)
                .abs()
                < EPSILON,
            "yaw turned at {}",
            controller.yaw
        );
        assert!(
            (controller.pitch - (ThirdPersonCamera::START.pitch - 100.0 * 0.0014)).abs() < EPSILON,
            "pitch turned at {}",
            controller.pitch
        );
    }

    #[test]
    fn pitch_stops_short_of_straight_up_and_straight_down() {
        let mut controller = ThirdPersonCamera::START;
        controller.rotate(0.0, -100_000.0);
        assert!((controller.pitch - MAX_PITCH).abs() < EPSILON);
        controller.rotate(0.0, 200_000.0);
        assert!((controller.pitch - MIN_PITCH).abs() < EPSILON);
    }

    #[test]
    fn yaw_stays_bounded_across_many_turns() {
        let mut controller = ThirdPersonCamera::START;
        for _ in 0..1_000 {
            controller.rotate(500.0, 0.0);
        }
        assert!(controller.yaw.is_finite());
        assert!((0.0..std::f32::consts::TAU).contains(&controller.yaw));
    }

    #[test]
    fn the_wheel_zooms_between_its_limits() {
        let mut controller = ThirdPersonCamera::START;
        controller.zoom(100.0);
        assert!((controller.distance - MIN_DISTANCE).abs() < EPSILON);
        controller.zoom(-1_000.0);
        assert!((controller.distance - MAX_DISTANCE).abs() < EPSILON);
    }

    #[test]
    fn following_catches_up_without_ever_overshooting() {
        let mut controller = ThirdPersonCamera::following(Vec3::ZERO);
        let target = Vec3::new(0.0, 0.0, -20.0);
        let mut previous = controller.focus().z;

        for _ in 0..600 {
            controller.follow(target, 1.0 / 60.0);
            let z = controller.focus().z;
            assert!(z <= previous + EPSILON, "the focus moved backwards");
            assert!(z >= target.z - EPSILON, "the focus overshot: {z}");
            previous = z;
        }
        assert!((controller.focus().z - target.z).abs() < 0.01, "{previous}");
    }

    #[test]
    fn following_is_frame_rate_independent() {
        // The property exponential smoothing exists for: the same elapsed time must produce
        // the same camera at 60 and at 480 FPS, or the camera lags differently on a faster
        // machine.
        let target = Vec3::new(0.0, 0.0, -10.0);
        let mut slow = ThirdPersonCamera::following(Vec3::ZERO);
        let mut fast = ThirdPersonCamera::following(Vec3::ZERO);

        for _ in 0..6 {
            slow.follow(target, 1.0 / 60.0);
        }
        for _ in 0..48 {
            fast.follow(target, 1.0 / 480.0);
        }

        assert!(
            (slow.focus().z - fast.focus().z).abs() < 0.02,
            "{:?} vs {:?}",
            slow.focus(),
            fast.focus()
        );
    }

    #[test]
    fn snapping_skips_the_smoothing() {
        let mut controller = ThirdPersonCamera::following(Vec3::ZERO);
        controller.snap_to(Vec3::new(100.0, 0.0, 100.0));
        assert!((controller.focus().x - 100.0).abs() < EPSILON);
        assert!((controller.focus().z - 100.0).abs() < EPSILON);
    }

    #[test]
    fn a_degenerate_viewport_is_rejected_rather_than_producing_infinities() {
        let mut view = camera();
        assert!(ThirdPersonCamera::START.apply(&mut view, 0.0).is_err());
    }

    #[test]
    fn a_zero_delta_frame_places_the_focus_exactly() {
        // The first frame has a zero delta, and a camera that refused to move on it would
        // start the game looking at the origin from wherever it was constructed.
        let mut controller = ThirdPersonCamera::START;
        controller.follow(Vec3::new(5.0, 0.0, 5.0), 0.0);
        assert!((controller.focus().x - 5.0).abs() < EPSILON);
    }
}
