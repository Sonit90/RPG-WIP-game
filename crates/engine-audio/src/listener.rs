//! Where the player is listening from, and what that does to a sound in the world.
//!
//! ```text
//!            world                       listener                    speakers
//!   sound at a position  ──▶  distance ──▶ attenuation  ──▶  gain
//!                            direction ──▶ pan           ──▶  left / right
//! ```
//!
//! # There is one camera and this is not it
//!
//! The listener holds a position and a basis, and `engine-app` writes the *gameplay*
//! camera's into it once a frame. This crate cannot see a camera — it depends on
//! `engine-core` — which is exactly what makes it impossible for audio to grow a second
//! camera that drifts from the one the player is looking through.
//!
//! # The model
//!
//! * **Attenuation** is inverse-distance with a reference radius, faded to nothing at a
//!   maximum. Inverse distance is how sound actually behaves; the reference radius stops a
//!   sound *on* the player from dividing by nothing, and the fade to zero at the maximum is
//!   what lets a voice be dropped without a click.
//! * **Panning** is constant power: a sound moved from centre to hard left keeps the same
//!   total energy, which is what stops the arena getting louder as the camera turns.
//! * **A sound behind you is not silent.** Two speakers cannot express behind at all, and
//!   pretending otherwise by attenuating the back would make a target the player turned away
//!   from vanish rather than move.

use engine_core::math::Vec3;

/// Where a sound ends up in the two output channels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Gain into the left channel, attenuation included.
    pub left: f32,
    /// Gain into the right channel.
    pub right: f32,
    /// How far the sound was, in metres.
    pub distance: f32,
}

impl Placement {
    /// A sound with no position: dead centre, no attenuation. What UI plays through.
    pub const CENTRED: Self = Self {
        left: std::f32::consts::FRAC_1_SQRT_2,
        right: std::f32::consts::FRAC_1_SQRT_2,
        distance: 0.0,
    };

    /// Silence, which is what a sound past its maximum distance is.
    pub const SILENT: Self = Self {
        left: 0.0,
        right: 0.0,
        distance: f32::INFINITY,
    };

    /// The louder of the two channels. What the voice limiter ranks by.
    #[must_use]
    pub fn peak(&self) -> f32 {
        self.left.max(self.right)
    }
}

/// The ear: a position and the basis it hears in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    /// Where it is, in world space.
    pub position: Vec3,
    /// Where it faces. Normalised on the way in.
    pub forward: Vec3,
    /// Which way is up for it.
    pub up: Vec3,
}

impl Default for Listener {
    /// At the origin, facing −Z, up +Y — the engine's own convention.
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            forward: Vec3::new(0.0, 0.0, -1.0),
            up: Vec3::new(0.0, 1.0, 0.0),
        }
    }
}

impl Listener {
    /// A listener at `position` facing `forward` with `up` overhead.
    ///
    /// A degenerate basis — a zero forward, an up parallel to it, a NaN anywhere — falls
    /// back to the default orientation rather than producing NaN gains that would reach the
    /// device as a click.
    #[must_use]
    pub fn new(position: Vec3, forward: Vec3, up: Vec3) -> Self {
        let mut listener = Self::default();
        if position.x.is_finite() && position.y.is_finite() && position.z.is_finite() {
            listener.position = position;
        }
        let forward_length = forward.length();
        let up_length = up.length();
        if forward_length > 1e-4 && up_length > 1e-4 {
            let forward = forward * (1.0 / forward_length);
            let up = up * (1.0 / up_length);
            // Parallel axes give a zero right vector, which would put every sound dead
            // centre. Keeping the default basis is a better answer than a silent one.
            if forward.cross(up).length() > 1e-4 {
                listener.forward = forward;
                listener.up = up;
            }
        }
        listener
    }

    /// Which way is right, from here.
    #[must_use]
    pub fn right(&self) -> Vec3 {
        self.forward.cross(self.up).normalize()
    }

    /// Where a sound at `position` lands, given the distances its catalogue row named.
    #[must_use]
    pub fn place(&self, position: Vec3, ref_distance: f32, max_distance: f32) -> Placement {
        let offset = position - self.position;
        let distance = offset.length();
        if !distance.is_finite() {
            return Placement::SILENT;
        }
        let attenuation = attenuate(distance, ref_distance, max_distance);
        if attenuation <= 0.0 {
            return Placement::SILENT;
        }

        // Inside the reference radius a sound has no direction worth naming — it is on you —
        // so it is centred rather than swinging between the speakers as you turn.
        let pan = if distance > 1e-3 {
            let direction = offset * (1.0 / distance);
            let sideways = direction.dot(self.right());
            // Held short of full so a sound hard to one side is still present in the other
            // ear. A pan of exactly 1 silences a speaker, which reads as a fault.
            (sideways * PAN_WIDTH).clamp(-1.0, 1.0)
        } else {
            0.0
        };

        // Constant power: the two gains are a quarter-circle apart, so their squares always
        // sum to one and moving a sound across the field does not change how loud it is.
        let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        Placement {
            left: angle.cos() * attenuation,
            right: angle.sin() * attenuation,
            distance,
        }
    }
}

/// How far towards a hard pan a sound at the very edge of the field goes.
///
/// Not 1.0: a fully panned sound leaves one speaker empty, and on headphones that reads as a
/// channel having failed rather than as a fireball to the left.
const PAN_WIDTH: f32 = 0.85;

/// How loud a sound `distance` away is.
///
/// Inverse distance beyond `ref_distance`, multiplied by a linear fade over the last
/// quarter of the range so a voice reaches exactly zero at `max_distance` instead of being
/// cut off while still audible.
#[must_use]
pub fn attenuate(distance: f32, ref_distance: f32, max_distance: f32) -> f32 {
    let reference = ref_distance.max(0.01);
    let maximum = max_distance.max(reference + 0.01);
    if !distance.is_finite() || distance >= maximum {
        return 0.0;
    }
    if distance <= reference {
        return 1.0;
    }
    let inverse = reference / distance;
    // The last quarter of the range is where the fade lives; before it, attenuation is pure
    // inverse distance and sounds like it.
    let fade_start = reference + (maximum - reference) * 0.75;
    if distance <= fade_start {
        return inverse;
    }
    let fade = 1.0 - (distance - fade_start) / (maximum - fade_start);
    inverse * fade.clamp(0.0, 1.0)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn listener() -> Listener {
        Listener::new(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
    }

    #[test]
    fn a_sound_gets_quieter_with_distance() {
        let listener = listener();
        let mut previous = f32::INFINITY;
        for metres in [0.0, 1.0, 2.5, 5.0, 10.0, 20.0, 40.0, 54.0] {
            let placement = listener.place(Vec3::new(0.0, 0.0, -metres), 2.5, 55.0);
            let level = placement.peak();
            assert!(level <= previous + 1e-6, "{metres} m got louder");
            previous = level;
        }
        assert_eq!(
            listener.place(Vec3::new(0.0, 0.0, -80.0), 2.5, 55.0).peak(),
            0.0,
            "past the maximum is silence"
        );
    }

    #[test]
    fn inside_the_reference_radius_nothing_changes() {
        let listener = listener();
        let near = listener.place(Vec3::new(0.0, 0.0, -1.0), 2.5, 55.0);
        let nearer = listener.place(Vec3::new(0.0, 0.0, -0.1), 2.5, 55.0);
        assert!((near.peak() - nearer.peak()).abs() < 1e-6);
    }

    #[test]
    fn left_is_left_and_right_is_right() {
        let listener = listener();
        // Facing −Z with +Y up, `right` is −X … which is the left-handed basis the rest of
        // the engine uses. What matters is that the two sides differ and are mirrored.
        let right = listener.place(listener.right() * 5.0, 2.5, 55.0);
        let left = listener.place(listener.right() * -5.0, 2.5, 55.0);
        assert!(right.right > right.left, "a sound to the right leans right");
        assert!(left.left > left.right, "a sound to the left leans left");
        assert!(
            (right.right - left.left).abs() < 1e-5,
            "the two are mirrors"
        );
    }

    #[test]
    fn turning_the_listener_moves_the_stereo_image() {
        let sound = Vec3::new(5.0, 0.0, 0.0);
        let facing = Listener::new(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let turned = Listener::new(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        let before = facing.place(sound, 2.5, 55.0);
        let after = turned.place(sound, 2.5, 55.0);
        assert!(
            (before.left - after.right).abs() < 1e-5 && (before.right - after.left).abs() < 1e-5,
            "turning around swaps the ears: {before:?} vs {after:?}"
        );
    }

    #[test]
    fn panning_holds_its_power() {
        let listener = listener();
        for position in [
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -5.0),
            Vec3::new(3.0, 0.0, -4.0),
        ] {
            let placement = listener.place(position, 2.5, 55.0);
            let attenuation = attenuate(5.0, 2.5, 55.0);
            let power = placement.left.powi(2) + placement.right.powi(2);
            assert!(
                (power - attenuation.powi(2)).abs() < 1e-4,
                "{position:?} changed the energy: {power}"
            );
        }
    }

    #[test]
    fn a_sound_hard_to_one_side_is_still_present_in_the_other_ear() {
        let listener = listener();
        let placement = listener.place(listener.right() * 8.0, 2.5, 55.0);
        assert!(placement.left > 0.05 * placement.right, "{placement:?}");
    }

    #[test]
    fn a_degenerate_listener_falls_back_instead_of_producing_nans() {
        let broken = Listener::new(Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(broken.forward, Listener::default().forward);

        let parallel = Listener::new(
            Vec3::new(f32::NAN, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert_eq!(parallel.position, Vec3::ZERO);
        let placement = parallel.place(Vec3::new(1.0, 0.0, 0.0), 2.5, 55.0);
        assert!(placement.left.is_finite() && placement.right.is_finite());
    }

    #[test]
    fn a_sound_at_a_nonsense_position_is_silent_rather_than_loud() {
        let listener = listener();
        let placement = listener.place(Vec3::new(f32::NAN, 0.0, 0.0), 2.5, 55.0);
        assert_eq!(placement.peak(), 0.0);
    }
}
