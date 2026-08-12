//! The light a scene is lit by, and the colours its sky is made of.
//!
//! # Why this is not `engine_render_data::Lighting`
//!
//! It is the same five numbers, and it deliberately does not share the type. `Lighting`
//! lives in `engine-render-data`, which is the crate that describes *what the renderer is
//! handed*; this crate must not depend on it, for the same reason `engine-game` must not.
//! The conversion is a five-field copy done once when a scene is loaded, in the application,
//! which is the one place that legitimately knows both types.
//!
//! The two extra fields — the sky's zenith and horizon colours — have no counterpart in
//! `Lighting` at all. They are content: they tint the sky dome's texture and they set the
//! colour the back buffer is cleared to, so that the sky and the void behind it agree.

use engine_core::math::Vec3;

/// One directional light, an ambient term, and the sky's two colours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvironmentLighting {
    /// The direction the sun's light *travels*. Normalized on use.
    pub sun_direction: Vec3,
    /// Linear colour of the sun.
    pub sun_color: Vec3,
    /// Sun strength. 1.0 means a fully lit surface shows its base colour.
    pub sun_intensity: f32,
    /// Linear ambient colour: everything the single light does not account for.
    ///
    /// Tinted towards the sky, because in an outdoor scene the fill light *is* the sky. A
    /// neutral grey ambient is what makes a stylized scene look like a technical demo.
    pub ambient_color: Vec3,
    /// Ambient strength. What stops an unlit face from being black.
    pub ambient_intensity: f32,
    /// Linear colour straight overhead.
    pub sky_zenith: Vec3,
    /// Linear colour at the horizon.
    pub sky_horizon: Vec3,
}

impl EnvironmentLighting {
    /// Late afternoon: a low warm sun, a cool sky, long readable shading.
    ///
    /// The arena's preset, and the reasoning behind every number:
    ///
    /// * The sun is **low** (`y` about −0.55 against a horizontal length near 1) so a
    ///   vertical face and a horizontal one receive clearly different amounts of light. A sun
    ///   overhead flattens every silhouette in the scene, which is the opposite of the
    ///   readability the phase asks for.
    /// * The sun is **warm** and the ambient is **cool**. Two-tone lighting is what separates
    ///   a lit face from a shadowed one when there are no cast shadows to do it — the lit
    ///   side goes gold, the unlit side goes blue, and the eye reads form from the hue
    ///   difference rather than from brightness alone.
    /// * The ambient is **0.34**, which is high. There are no cast shadows and no bounce, so
    ///   an unlit face gets exactly this and nothing else; below about 0.25 the far side of
    ///   every rock reads as a hole.
    /// * The sun's intensity is **1.25** rather than 1.0 because the ambient is added on top
    ///   of it, and a fully lit surface should still reach its base colour.
    #[must_use]
    pub fn arena_afternoon() -> Self {
        Self {
            sun_direction: Vec3::new(-0.62, -0.55, -0.56),
            sun_color: Vec3::new(1.00, 0.91, 0.75),
            sun_intensity: 1.25,
            ambient_color: Vec3::new(0.46, 0.58, 0.80),
            ambient_intensity: 0.34,
            sky_zenith: Vec3::new(0.16, 0.31, 0.60),
            sky_horizon: Vec3::new(0.74, 0.66, 0.56),
        }
    }

    /// The colour the back buffer is cleared to.
    ///
    /// The horizon colour, darkened. The sky dome covers the whole view in the shipping
    /// arena, so this is only seen where the dome does not reach — but "only seen when
    /// something is wrong" is exactly the case where a colour that matches the sky hides the
    /// problem and one that clashes reveals it. Darkened rather than matched, so a gap in
    /// the dome reads as a gap.
    #[must_use]
    pub fn clear_color(&self) -> [f32; 4] {
        [
            self.sky_horizon.x * 0.45,
            self.sky_horizon.y * 0.45,
            self.sky_horizon.z * 0.50,
            1.0,
        ]
    }
}

impl Default for EnvironmentLighting {
    fn default() -> Self {
        Self::arena_afternoon()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arena_sun_is_low_warm_and_filled_by_a_cool_ambient() {
        let lighting = EnvironmentLighting::arena_afternoon();

        assert!(lighting.sun_direction.y < 0.0, "the sun points downward");
        let horizontal = lighting.sun_direction.x.hypot(lighting.sun_direction.z);
        assert!(
            horizontal > lighting.sun_direction.y.abs(),
            "a low sun travels further sideways than down: {:?}",
            lighting.sun_direction
        );

        assert!(
            lighting.sun_color.x > lighting.sun_color.z,
            "the sun is warm"
        );
        assert!(
            lighting.ambient_color.z > lighting.ambient_color.x,
            "the fill is cool"
        );
        assert!(
            lighting.ambient_intensity > 0.25,
            "with no shadows and no bounce, an unlit face gets only this"
        );
    }

    #[test]
    fn the_clear_colour_is_darker_than_the_sky_so_a_gap_in_the_dome_shows() {
        let lighting = EnvironmentLighting::arena_afternoon();
        let clear = lighting.clear_color();
        assert!(clear[0] < lighting.sky_horizon.x);
        assert!(clear[1] < lighting.sky_horizon.y);
        assert_eq!(clear[3], 1.0);
    }
}
