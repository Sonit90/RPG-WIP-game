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

use crate::scene::EnvironmentScene;

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

/// A light that belongs to a place rather than to the sky.
///
/// A brazier, a torch, a shaft of light down a stairwell. Scene data, exactly like the sun
/// above it — and, exactly like the sun, it reaches the renderer as a copy made by the
/// application rather than by sharing a type with it: this crate must not depend on
/// `engine-render-data`, for the same reason `engine-game` must not.
///
/// # Shadows are opt-in and rationed
///
/// `casts_shadow` is a *request*. The renderer has a small budget of shadow maps for local
/// lights and hands them out by priority and then by distance, so a dungeon can ask for
/// twenty and get four. Asking costs nothing when it is refused; see §21 of the shadow plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLight {
    /// Where it is.
    pub position: Vec3,
    /// Which way the cone points. Ignored unless `cone_radians` is set.
    pub direction: Vec3,
    /// Linear colour.
    pub color: Vec3,
    /// Strength at the light itself, before attenuation.
    pub intensity: f32,
    /// How far it reaches, in metres.
    pub range: f32,
    /// The cone's inner and outer half-angles in radians, or `None` for a point light.
    pub cone_radians: Option<(f32, f32)>,
    /// Whether it asks for a shadow map.
    pub casts_shadow: bool,
    /// Which lights win when there are more applicants than shadow maps. Higher is better.
    pub priority: i16,
}

impl Default for SceneLight {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            direction: Vec3::new(0.0, -1.0, 0.0),
            color: Vec3::ONE,
            intensity: 1.0,
            range: 10.0,
            cone_radians: None,
            casts_shadow: false,
            priority: 0,
        }
    }
}

impl SceneLight {
    /// A warm point light: a brazier, a fire, a lamp.
    #[must_use]
    pub fn brazier(position: Vec3, range: f32) -> Self {
        Self {
            position,
            // Firelight: strongly warm, because the sun in these scenes is warm-neutral and
            // the fill is blue. A neutral point light reads as a bug rather than as a fire.
            color: Vec3::new(1.0, 0.62, 0.28),
            intensity: 3.0,
            range,
            ..Self::default()
        }
    }

    /// A cone of light pointing along `direction`.
    #[must_use]
    pub fn spot(position: Vec3, direction: Vec3, range: f32, outer_radians: f32) -> Self {
        Self {
            position,
            direction,
            intensity: 4.0,
            range,
            // The inner angle at two thirds of the outer one gives a rim soft enough not to
            // read as a stencil and hard enough to still be a beam.
            cone_radians: Some((outer_radians * 0.66, outer_radians)),
            ..Self::default()
        }
    }

    /// The same light, asking for a shadow map at the given priority.
    #[must_use]
    pub fn casting_shadows(mut self, priority: i16) -> Self {
        self.casts_shadow = true;
        self.priority = priority;
        self
    }

    /// The same light with its colour and strength replaced.
    #[must_use]
    pub fn with_color(mut self, color: Vec3, intensity: f32) -> Self {
        self.color = color;
        self.intensity = intensity;
        self
    }
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

/// Adds `count` extra point lights to a scene, spread over its floor.
///
/// A **stress control**, in the same family as the environment scatter density and the
/// effect stress driver: the arena has four brazier lights, and "does the lighting scale?"
/// is a question four of them cannot answer. Placed by the same hash the scatter uses, so a
/// given count always produces the same scene and two runs of a benchmark compare like with
/// like.
///
/// The first `casting` of them ask for a shadow map; the rest only light. The split is the
/// point: shading many lights and shadowing many lights are two different costs with two
/// different structures behind them, and a control that could not separate them would
/// measure their sum and attribute it to whichever was being changed.
///
/// ```text
/// count = 0                  the scene as authored
/// count = 8,  casting = 0    eight more torches, none of them shadowing
/// count = 8,  casting = 8    the same eight, all asking for maps
/// count = 32, casting = 0    the renderer's per-frame light maximum
/// ```
pub fn push_stress_lights(scene: &mut EnvironmentScene, count: usize, casting: usize) {
    if count == 0 {
        return;
    }
    // Inside the boundary and clear of the middle, where the fight happens: a stress light
    // standing on the player is a light whose cost is one pixel's worth, which measures
    // nothing.
    let outer = (scene.half_extent - 6.0).max(4.0);
    let inner = outer * 0.35;
    for index in 0..count {
        // A golden-angle spiral: successive lights land far apart in both angle and radius,
        // so no count produces a ring of lights all in one cluster.
        let fraction = (index as f32 + 0.5) / count as f32;
        let angle = index as f32 * 2.399_963_2;
        let radius = inner + (outer - inner) * fraction.sqrt();
        let position = Vec3::new(
            radius * angle.cos(),
            1.45 + crate::hash_range(index as u32, 97, -0.3, 0.6),
            radius * angle.sin(),
        );
        // The brazier's own range, so a stress light costs what a real one costs.
        let light = SceneLight::brazier(position, 14.0);
        scene.push_light(if index < casting {
            light.casting_shadows(0)
        } else {
            light
        });
    }
}

#[cfg(test)]
mod stress_light_tests {
    use super::*;

    fn scene() -> EnvironmentScene {
        let mut scene = EnvironmentScene::with_capacity("test", 8);
        scene.half_extent = 40.0;
        scene
    }

    #[test]
    fn no_stress_lights_leaves_the_scene_alone() {
        let mut scene = scene();
        push_stress_lights(&mut scene, 0, 0);
        assert!(scene.lights.is_empty());
    }

    #[test]
    fn every_stress_light_stands_inside_the_scene() {
        // A stress light outside the boundary lights the outside of a wall, which measures
        // the cost of a light nobody can see.
        let mut scene = scene();
        push_stress_lights(&mut scene, 32, 0);
        assert_eq!(scene.lights.len(), 32);
        for light in &scene.lights {
            let radius = light.position.x.hypot(light.position.z);
            assert!(
                radius < scene.half_extent,
                "a stress light stands {radius} m out, past the {} m boundary",
                scene.half_extent
            );
            assert!(light.position.y > 0.0, "a stress light is under the floor");
        }
    }

    #[test]
    fn stress_lights_cast_only_when_asked_to() {
        // The two costs stay separable: "what does shading many lights cost" and "what does
        // shadowing many lights cost" are different questions with different structures
        // behind them.
        let mut quiet = scene();
        push_stress_lights(&mut quiet, 16, 0);
        assert!(quiet.lights.iter().all(|light| !light.casts_shadow));

        let mut casting = scene();
        push_stress_lights(&mut casting, 16, 6);
        assert_eq!(
            casting.lights.iter().filter(|l| l.casts_shadow).count(),
            6,
            "the first six were asked to cast"
        );
    }

    #[test]
    fn the_same_count_always_produces_the_same_scene() {
        let (mut first, mut second) = (scene(), scene());
        push_stress_lights(&mut first, 12, 4);
        push_stress_lights(&mut second, 12, 4);
        assert_eq!(first.lights, second.lights);
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
