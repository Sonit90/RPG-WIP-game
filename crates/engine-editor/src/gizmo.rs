//! The transform gizmo: the handles an author drags a selection by.
//!
//! ```text
//!   MOVE (1)              TURN (2)                SCALE (3)
//!      +Y                   .-~~~-.                  +Y ■
//!       |                 /         \                 |
//!       O---- +X         |     O     |               ■---- ■ +X
//!      /                  \         /               /
//!    +Z                     `-...-'               ■ +Z      centre square: all three
//! ```
//!
//! # Why it lives in screen space
//!
//! This crate has no renderer and no device — it edits data and paints a `UiDrawList`, which
//! is what lets every verb be tested with no window open. So the gizmo is not meshes in the
//! world: it is the selection's origin and a few world-space points around it, projected with
//! the frame's view-projection and drawn as lines in the interface layer. That also gives it
//! the property a manipulator wants for free — it is drawn *over* the level, so a handle is
//! never buried inside the object it belongs to.
//!
//! # Dragging
//!
//! A move or scale handle is a segment on screen that stands for [`GizmoScreen::length`]
//! metres along one axis. The metres a mouse movement asks for is therefore the movement
//! projected onto that segment: `dot(movement, handle) / dot(handle, handle) * length`. That
//! is exactly what constrains a drag to one axis — movement across the handle contributes
//! nothing.
//!
//! The turn ring is the circle the selection's yaw sweeps, and a drag on it turns by the angle
//! the pointer sweeps around the gizmo's centre on screen. Rotation is yaw-only in this engine,
//! so there is one ring and not three.
//!
//! The scale handles run along the *object's own* axes, because scale is a property of the
//! object's own space: stretching a turned wall along its length means its X, whatever world
//! direction that happens to be.

use engine_core::math::{Mat4, Quat, Vec3};

/// One of the three axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// +X.
    X,
    /// +Y.
    Y,
    /// +Z.
    Z,
}

impl Axis {
    /// The three of them, in the order they are drawn and hit-tested.
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    /// The unit vector this axis moves along, in world space.
    #[must_use]
    pub fn direction(self) -> Vec3 {
        match self {
            Self::X => Vec3::new(1.0, 0.0, 0.0),
            Self::Y => Vec3::new(0.0, 1.0, 0.0),
            Self::Z => Vec3::new(0.0, 0.0, 1.0),
        }
    }

    /// Which slot of a `[_; 3]` this axis is.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    /// What the handle is painted: the colours every 3D package agrees on.
    #[must_use]
    pub fn color(self) -> [f32; 4] {
        match self {
            Self::X => [0.94, 0.26, 0.26, 1.0],
            Self::Y => [0.36, 0.90, 0.36, 1.0],
            Self::Z => [0.34, 0.56, 1.00, 1.0],
        }
    }

    /// The letter drawn at the tip.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }
}

/// What the gizmo currently does with a drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GizmoMode {
    /// Dragging a handle moves the selection along that world axis.
    #[default]
    Translate,
    /// Dragging the ring turns the selection about +Y.
    Rotate,
    /// Dragging a handle stretches the selection along its own axis; the centre, all three.
    Scale,
}

impl GizmoMode {
    /// What the mode badge says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Translate => "MOVE",
            Self::Rotate => "TURN",
            Self::Scale => "SCALE",
        }
    }
}

/// What part of the gizmo a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// One axis handle.
    Axis(Axis),
    /// The turn ring.
    Ring,
    /// The scale gizmo's centre: scale along all three at once.
    Uniform,
}

impl Handle {
    /// What the status line calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Axis(axis) => axis.label(),
            Self::Ring => "YAW",
            Self::Uniform => "XYZ",
        }
    }
}

/// One projected axis handle: where it starts and ends on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoHandle {
    /// Which axis it moves along.
    pub axis: Axis,
    /// The gizmo's origin on screen, in physical pixels.
    pub from: (f32, f32),
    /// The tip, in physical pixels.
    pub to: (f32, f32),
}

impl GizmoHandle {
    /// How long the handle is on screen, in pixels.
    #[must_use]
    pub fn pixels(self) -> f32 {
        let (dx, dy) = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        (dx * dx + dy * dy).sqrt()
    }

    /// How far `pointer` is from the handle, in pixels, and where along it that is.
    ///
    /// `None` when the handle is degenerate: an axis pointing straight at the eye is a dot,
    /// and a dot is a handle nobody can aim at or drag meaningfully.
    fn distance(self, pointer: (f32, f32)) -> Option<(f32, f32)> {
        let (dx, dy) = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        let length_squared = dx * dx + dy * dy;
        if length_squared < 1.0 {
            return None;
        }
        let (px, py) = (pointer.0 - self.from.0, pointer.1 - self.from.1);
        let along = ((px * dx + py * dy) / length_squared).clamp(0.0, 1.0);
        let (ox, oy) = (px - dx * along, py - dy * along);
        Some(((ox * ox + oy * oy).sqrt(), along))
    }
}

/// How many straight pieces the turn ring is drawn and hit-tested as.
pub const RING_SEGMENTS: usize = 48;

/// The gizmo as this frame's pointer sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoScreen {
    /// What a drag on it means.
    pub mode: GizmoMode,
    /// Where the selection's origin is on screen, in physical pixels.
    pub origin: (f32, f32),
    /// How many metres one whole handle stands for.
    pub length: f32,
    /// The handles, in [`Axis::ALL`] order. An axis whose tip is behind the eye has none, and
    /// the turn gizmo has none at all.
    pub handles: [Option<GizmoHandle>; 3],
    /// The turn ring's points on screen, for the turn gizmo; `None` where a point is behind
    /// the eye. Empty (all `None`) for the other two.
    pub ring: [Option<(f32, f32)>; RING_SEGMENTS],
    /// Whether the eye is above the ring's plane, which is which way round a clockwise drag
    /// on screen turns the selection.
    pub from_above: bool,
}

/// How much of the distance to the eye the gizmo spans.
///
/// A constant *angle* rather than a constant size in metres: a gizmo written in metres is a
/// speck on an object two hundred metres away and swallows one that is two metres away, and
/// an author works at both distances in the same session.
const SCREEN_FRACTION: f32 = 0.16;

/// The shortest the gizmo may be, in metres.
///
/// The floor keeps it aimable when the camera is inside the object; the ceiling keeps it from
/// reaching across the whole level when the camera is high above a layout.
const MIN_LENGTH: f32 = 0.6;
/// The longest it may be. See [`MIN_LENGTH`].
const MAX_LENGTH: f32 = 60.0;

/// How many metres of axis the gizmo should span, seen from `eye`.
#[must_use]
pub fn length_for(origin: Vec3, eye: Vec3) -> f32 {
    let distance = (origin - eye).length();
    (distance * SCREEN_FRACTION).clamp(MIN_LENGTH, MAX_LENGTH)
}

/// How near the cursor must be to a handle to grab it, in logical pixels.
pub const GRAB_PIXELS: f32 = 11.0;

/// A point of the turn ring, `angle` radians round from -Z the way a positive yaw turns.
fn ring_point(origin: Vec3, length: f32, angle: f32) -> Vec3 {
    origin + Vec3::new(-angle.sin(), 0.0, -angle.cos()) * length
}

/// Where the handles are this frame, or `None` if the origin itself is not on screen.
///
/// `rotation` is the selection's own rotation, which the scale handles run along; the move
/// handles always run along the world's axes. The origin being off screen is the case the
/// striped silhouette answers — see [`crate::ui`]. There is nothing to grab when the thing
/// being manipulated is behind the camera, and a handle projected from a point behind the eye
/// points the wrong way.
#[must_use]
pub fn project(
    mode: GizmoMode,
    origin: Vec3,
    length: f32,
    rotation: Quat,
    eye: Vec3,
    view_projection: &Mat4,
    viewport: (u32, u32),
) -> Option<GizmoScreen> {
    let screen = engine_presentation::nameplate::project(origin, view_projection, viewport)?;
    let mut handles = [None, None, None];
    let mut ring = [None; RING_SEGMENTS];
    match mode {
        GizmoMode::Translate | GizmoMode::Scale => {
            for (slot, axis) in Axis::ALL.into_iter().enumerate() {
                let direction = if mode == GizmoMode::Scale {
                    rotation.rotate(axis.direction())
                } else {
                    axis.direction()
                };
                let tip = origin + direction * length;
                handles[slot] =
                    engine_presentation::nameplate::project(tip, view_projection, viewport).map(
                        |to| GizmoHandle {
                            axis,
                            from: screen,
                            to,
                        },
                    );
            }
        }
        GizmoMode::Rotate => {
            for (slot, point) in ring.iter_mut().enumerate() {
                let angle = slot as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                *point = engine_presentation::nameplate::project(
                    ring_point(origin, length, angle),
                    view_projection,
                    viewport,
                );
            }
        }
    }
    Some(GizmoScreen {
        mode,
        origin: screen,
        length,
        handles,
        ring,
        from_above: eye.y >= origin.y,
    })
}

impl GizmoScreen {
    /// Which part of the gizmo is under `pointer`, if any. The nearest wins.
    ///
    /// `tolerance` is in physical pixels, so the caller scales it — a handle that is eleven
    /// logical pixels wide has to be eleven logical pixels wide on a 150% display too.
    #[must_use]
    pub fn hit(&self, pointer: (f32, f32), tolerance: f32) -> Option<Handle> {
        match self.mode {
            GizmoMode::Rotate => self.hit_ring(pointer, tolerance),
            GizmoMode::Translate | GizmoMode::Scale => {
                if self.mode == GizmoMode::Scale {
                    let (dx, dy) = (pointer.0 - self.origin.0, pointer.1 - self.origin.1);
                    if (dx * dx + dy * dy).sqrt() <= tolerance * 1.2 {
                        return Some(Handle::Uniform);
                    }
                }
                self.hit_axis(pointer, tolerance).map(Handle::Axis)
            }
        }
    }

    /// The nearest axis handle within `tolerance`.
    fn hit_axis(&self, pointer: (f32, f32), tolerance: f32) -> Option<Axis> {
        let mut best: Option<(Axis, f32)> = None;
        for handle in self.handles.into_iter().flatten() {
            let Some((distance, along)) = handle.distance(pointer) else {
                continue;
            };
            // The very centre belongs to no axis in particular: all three handles meet there,
            // so a click on it would pick whichever happened to be tested first.
            if along < 0.12 || distance > tolerance {
                continue;
            }
            if best.is_none_or(|(_, nearest)| distance < nearest) {
                best = Some((handle.axis, distance));
            }
        }
        best.map(|(axis, _)| axis)
    }

    /// The ring, if the pointer is within `tolerance` of any piece of it.
    fn hit_ring(&self, pointer: (f32, f32), tolerance: f32) -> Option<Handle> {
        self.ring_segments()
            .any(|(from, to)| {
                let segment = GizmoHandle {
                    axis: Axis::Y,
                    from,
                    to,
                };
                segment
                    .distance(pointer)
                    .is_some_and(|(distance, _)| distance <= tolerance)
            })
            .then_some(Handle::Ring)
    }

    /// The ring's pieces whose two ends both project.
    pub fn ring_segments(&self) -> impl Iterator<Item = ((f32, f32), (f32, f32))> + '_ {
        (0..RING_SEGMENTS).filter_map(|slot| {
            let from = self.ring[slot]?;
            let to = self.ring[(slot + 1) % RING_SEGMENTS]?;
            Some((from, to))
        })
    }

    /// The handle for one axis, if it has one this frame.
    #[must_use]
    pub fn handle(&self, axis: Axis) -> Option<GizmoHandle> {
        self.handles
            .into_iter()
            .flatten()
            .find(|handle| handle.axis == axis)
    }

    /// How many metres along `axis` a movement from `from` to `to` on screen asks for.
    ///
    /// Zero when the handle is degenerate — an axis pointing at the eye has no direction on
    /// screen to measure a movement against, and guessing one is how a gizmo teleports an
    /// object when the camera happens to line up with it.
    #[must_use]
    pub fn metres(&self, axis: Axis, from: (f32, f32), to: (f32, f32)) -> f32 {
        let Some(handle) = self.handle(axis) else {
            return 0.0;
        };
        let (hx, hy) = (handle.to.0 - handle.from.0, handle.to.1 - handle.from.1);
        let length_squared = hx * hx + hy * hy;
        if length_squared < 1.0 {
            return 0.0;
        }
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        (dx * hx + dy * hy) / length_squared * self.length
    }

    /// The pointer's angle about the gizmo's centre on screen, radians, counter-clockwise
    /// with up as up — or `None` too near the centre to have a direction.
    #[must_use]
    pub fn pointer_angle(&self, pointer: (f32, f32)) -> Option<f32> {
        let (dx, dy) = (pointer.0 - self.origin.0, self.origin.1 - pointer.1);
        (dx * dx + dy * dy > 4.0).then(|| dy.atan2(dx))
    }

    /// How far the selection turns for the pointer sweeping `swept` radians on screen.
    ///
    /// Seen from above, counter-clockwise on screen is a positive yaw; seen from below the
    /// same sweep is the other way round, and a ring that turned the object against the hand
    /// is a ring nobody can use from inside a cave.
    #[must_use]
    pub fn yaw_for(&self, swept: f32) -> f32 {
        if self.from_above { swept } else { -swept }
    }
}

/// `angle` brought into `(-PI, PI]`: the short way round.
#[must_use]
pub fn wrap_angle(angle: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let wrapped = (angle + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
    if wrapped <= -std::f32::consts::PI {
        wrapped + tau
    } else {
        wrapped
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_render_data::{Camera, Projection};

    const VIEWPORT: (u32, u32) = (1920, 1080);

    /// A camera two metres up, twelve metres back, looking at the origin.
    fn looking_at_origin() -> Mat4 {
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        camera.set_position(Vec3::new(0.0, 2.0, 12.0));
        camera
            .look_at(
                Vec3::new(0.0, 2.0, 12.0),
                Vec3::new(0.0, 2.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            )
            .expect("a camera that looks somewhere");
        camera.view_projection()
    }

    fn gizmo_in(mode: GizmoMode) -> GizmoScreen {
        project(
            mode,
            Vec3::new(0.0, 2.0, 0.0),
            2.0,
            Quat::IDENTITY,
            Vec3::new(0.0, 2.0, 12.0),
            &looking_at_origin(),
            VIEWPORT,
        )
        .expect("a gizmo in front of the camera")
    }

    fn gizmo() -> GizmoScreen {
        gizmo_in(GizmoMode::Translate)
    }

    #[test]
    fn the_gizmo_grows_with_the_distance_to_the_eye_and_stops_at_both_ends() {
        let near = length_for(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0));
        let far = length_for(Vec3::ZERO, Vec3::new(0.0, 0.0, 100.0));
        assert!(near < far, "{near} should be shorter than {far}");
        assert_eq!(length_for(Vec3::ZERO, Vec3::new(0.0, 0.0, 0.1)), MIN_LENGTH);
        assert_eq!(
            length_for(Vec3::ZERO, Vec3::new(0.0, 0.0, 100_000.0)),
            MAX_LENGTH
        );
    }

    #[test]
    fn the_cursor_on_a_handle_finds_that_handle_and_the_centre_finds_none() {
        let gizmo = gizmo();
        let x = gizmo.handle(Axis::X).expect("an X handle");
        let midpoint = ((x.from.0 + x.to.0) * 0.5, (x.from.1 + x.to.1) * 0.5);
        assert_eq!(gizmo.hit(midpoint, 12.0), Some(Handle::Axis(Axis::X)));
        assert_eq!(gizmo.hit(gizmo.origin, 12.0), None);
    }

    #[test]
    fn a_cursor_far_from_every_handle_grabs_nothing() {
        let gizmo = gizmo();
        assert_eq!(gizmo.hit((10.0, 10.0), 12.0), None);
    }

    #[test]
    fn dragging_along_a_handle_asks_for_metres_and_across_it_asks_for_none() {
        let gizmo = gizmo();
        let x = gizmo.handle(Axis::X).expect("an X handle");
        let along = gizmo.metres(Axis::X, x.from, x.to);
        assert!(
            (along - gizmo.length).abs() < 1e-3,
            "dragging the whole handle should ask for its whole length, asked {along}"
        );
        // The X handle runs across the screen, so a movement straight down it is across it.
        let across = gizmo.metres(Axis::X, x.from, (x.from.0, x.from.1 + 200.0));
        assert!(
            across.abs() < 1e-3,
            "a movement across the axis moved {across}"
        );
    }

    #[test]
    fn every_axis_answers_for_its_own_direction() {
        assert_eq!(Axis::X.direction(), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(Axis::Y.direction(), Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(Axis::Z.direction(), Vec3::new(0.0, 0.0, 1.0));
    }

    #[test]
    fn the_scale_gizmo_has_a_centre_that_means_all_three() {
        let gizmo = gizmo_in(GizmoMode::Scale);
        assert_eq!(gizmo.hit(gizmo.origin, 12.0), Some(Handle::Uniform));
        let x = gizmo.handle(Axis::X).expect("an X handle");
        assert_eq!(gizmo.hit(x.to, 12.0), Some(Handle::Axis(Axis::X)));
    }

    #[test]
    fn the_scale_handles_follow_the_objects_own_axes() {
        let turned = project(
            GizmoMode::Scale,
            Vec3::new(0.0, 2.0, 0.0),
            2.0,
            Quat::from_yaw_pitch_roll(std::f32::consts::FRAC_PI_2, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 12.0),
            &looking_at_origin(),
            VIEWPORT,
        )
        .expect("a gizmo");
        let x = turned.handle(Axis::X).expect("an X handle");
        // A quarter turn puts the object's X along the world's Z, which from this camera is
        // nearly straight at the eye: a short handle, not the long horizontal one.
        assert!(x.pixels() < gizmo().handle(Axis::X).expect("world X").pixels() * 0.5);
    }

    #[test]
    fn the_turn_gizmo_is_a_ring_and_its_centre_is_not_on_it() {
        // From above: seen edge-on, a ring is a line through its own centre.
        let eye = Vec3::new(0.0, 12.0, 8.0);
        let mut camera =
            Camera::new(Projection::perspective(16.0 / 9.0)).expect("a valid projection");
        camera.set_position(eye);
        camera
            .look_at(eye, Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 1.0, 0.0))
            .expect("a camera that looks somewhere");
        let gizmo = project(
            GizmoMode::Rotate,
            Vec3::new(0.0, 2.0, 0.0),
            2.0,
            Quat::IDENTITY,
            eye,
            &camera.view_projection(),
            VIEWPORT,
        )
        .expect("a gizmo in front of the camera");
        assert!(gizmo.handles.iter().all(Option::is_none));
        let (from, _) = gizmo.ring_segments().next().expect("a ring on screen");
        assert_eq!(gizmo.hit(from, 12.0), Some(Handle::Ring));
        assert_eq!(gizmo.hit(gizmo.origin, 12.0), None);
    }

    #[test]
    fn a_sweep_seen_from_below_turns_the_other_way() {
        let mut gizmo = gizmo_in(GizmoMode::Rotate);
        assert!(gizmo.yaw_for(0.5) > 0.0);
        gizmo.from_above = false;
        assert!(gizmo.yaw_for(0.5) < 0.0);
    }

    #[test]
    fn an_angle_is_wrapped_the_short_way_round() {
        use std::f32::consts::PI;
        assert!((wrap_angle(1.5 * PI) + 0.5 * PI).abs() < 1e-5);
        assert!((wrap_angle(-1.5 * PI) - 0.5 * PI).abs() < 1e-5);
        assert!((wrap_angle(0.25) - 0.25).abs() < 1e-6);
    }
}
