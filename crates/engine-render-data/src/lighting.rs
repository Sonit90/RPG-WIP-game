//! Scene lighting.
//!
//! Game-authored: a day/night cycle, an indoor area or a boss arena sets these, so they
//! belong on the data side of the seam. What the renderer does with them — packing them
//! into a constant buffer whose layout matches an HLSL `cbuffer` — stays in the renderer.

use engine_core::math::Vec3;

/// One directional light and an ambient term.
///
/// Directional because a sun is the only light a third-person outdoor scene cannot do
/// without, and because it needs no attenuation, no position and no per-object light list —
/// point and spot lights need all three, and clustered shading is out of scope for Phase 2,
/// which is what makes many of them affordable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lighting {
    /// The direction the light *travels*, normalized on use.
    ///
    /// Authored the way it is described — "the sun points down and to the left" — and
    /// negated once, in the shader, rather than at every call site.
    pub direction: Vec3,
    /// Linear light colour.
    pub color: Vec3,
    /// Light strength. Not in physical units; there is no exposure control to calibrate it
    /// against, so 1.0 means "a fully lit surface shows its base colour".
    pub intensity: f32,
    /// Linear ambient colour, standing in for everything that is not the sun.
    pub ambient_color: Vec3,
    /// Ambient strength. This is what stops an unlit face from being pure black, which is
    /// what a single light with no bounce would otherwise produce.
    pub ambient_intensity: f32,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            // Down, forward and slightly to the side: a face pointing up is fully lit and
            // the two visible side faces of a cube get different amounts, which makes a
            // wrong normal obvious at a glance.
            direction: Vec3::new(-0.4, -1.0, -0.6),
            color: Vec3::ONE,
            intensity: 1.0,
            ambient_color: Vec3::ONE,
            ambient_intensity: 0.15,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_light_leaves_unlit_faces_visible() {
        // Ambient exists so a single directional light does not render half of every object
        // as pure black — there is no bounce lighting to fill it in.
        let lighting = Lighting::default();
        assert!(lighting.ambient_intensity > 0.0);
        assert!(lighting.direction.y < 0.0, "the sun points downward");
    }
}
