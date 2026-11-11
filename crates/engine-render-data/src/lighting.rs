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

/// What kind of local light a [`LocalLight`] is.
///
/// Local as opposed to the sun: it has a position, it attenuates, and it lights a bounded
/// part of the world. The directional light in [`Lighting`] has none of those properties,
/// which is why the two are different types rather than one type with a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalLightKind {
    /// Light in every direction from a point. Shadowed with six views, one per cube face.
    Point,
    /// A cone. Shadowed with one perspective view down the cone's axis.
    Spot,
}

/// A positioned light: a torch, a brazier, a lantern, a spell effect.
///
/// Rebuilt every frame like the skinned draw list rather than slot-managed like a
/// [`RenderEntity`](crate::world::RenderEntity): a light follows something that moves, and
/// there are tens of them rather than thousands.
///
/// # Shadows are opt-in
///
/// `casts_shadow` is false by default and must be set deliberately. A shadow-casting local
/// light costs a whole shadow view — a viewport of the atlas, a cull and a depth pass, six
/// times over for a point light — so "every torch in the dungeon casts shadows" is not a
/// setting the renderer can honour. `priority` and distance decide which of the ones that
/// asked actually get a map; see the shadow renderer's local-light budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalLight {
    /// Which kind of light this is.
    pub kind: LocalLightKind,
    /// World-space position.
    pub position: Vec3,
    /// The direction the cone points, for a spot light. Ignored by a point light.
    pub direction: Vec3,
    /// Linear colour.
    pub color: Vec3,
    /// Strength at the light's own position, before attenuation.
    pub intensity: f32,
    /// How far the light reaches, in metres. Attenuation is smoothly zero at this distance,
    /// which is what lets the renderer reject a light that cannot touch the view.
    pub range: f32,
    /// Cosine of the half-angle at which a spot light is at full strength.
    pub inner_cos: f32,
    /// Cosine of the half-angle at which a spot light has fallen to nothing.
    ///
    /// The cone's actual edge. `inner_cos` above it gives the soft rim; the two being equal
    /// gives a hard-edged cone.
    pub outer_cos: f32,
    /// Whether this light asks for a shadow map.
    pub casts_shadow: bool,
    /// Which lights win when there are more applicants than shadow maps.
    ///
    /// Higher is more important. A boss's brazier outranks a corridor torch whatever the
    /// distances are; among equal priorities the nearer light wins.
    pub priority: i16,
}

impl Default for LocalLight {
    fn default() -> Self {
        Self {
            kind: LocalLightKind::Point,
            position: Vec3::ZERO,
            direction: Vec3::new(0.0, -1.0, 0.0),
            color: Vec3::ONE,
            intensity: 1.0,
            range: 10.0,
            inner_cos: 1.0,
            outer_cos: 1.0,
            casts_shadow: false,
            priority: 0,
        }
    }
}

impl LocalLight {
    /// A point light of `range` metres.
    #[must_use]
    pub fn point(position: Vec3, color: Vec3, intensity: f32, range: f32) -> Self {
        Self {
            kind: LocalLightKind::Point,
            position,
            color,
            intensity,
            range,
            ..Self::default()
        }
    }

    /// A spot light pointing along `direction`, with its cone given as half-angles in
    /// radians.
    ///
    /// The angles are stored as cosines because that is what the shader compares against a
    /// dot product; converting here means one `cos` per light per frame on the CPU instead
    /// of an `acos` per pixel.
    #[must_use]
    pub fn spot(
        position: Vec3,
        direction: Vec3,
        color: Vec3,
        intensity: f32,
        range: f32,
        inner_radians: f32,
        outer_radians: f32,
    ) -> Self {
        // Ordered rather than trusted: an inner angle wider than the outer one inverts the
        // falloff and lights the outside of the cone.
        let inner = inner_radians.min(outer_radians);
        let outer = outer_radians.max(inner_radians);
        Self {
            kind: LocalLightKind::Spot,
            position,
            direction,
            color,
            intensity,
            range,
            inner_cos: inner.cos(),
            outer_cos: outer.cos(),
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

    /// The cone's outer half-angle in radians, for a spot light.
    ///
    /// What the shadow projection's field of view is built from: the perspective frustum has
    /// to cover the whole cone or the shadow map ends before the light does.
    #[must_use]
    pub fn outer_angle(&self) -> f32 {
        self.outer_cos.clamp(-1.0, 1.0).acos()
    }

    /// Whether the light can affect anything at all.
    ///
    /// A light with no range, no intensity or a black colour is not an error — a script
    /// fading a torch out passes through this state every time — it simply has nothing to
    /// contribute and is dropped before it costs a shadow map.
    #[must_use]
    pub fn is_significant(&self) -> bool {
        self.range > 0.0
            && self.intensity > 0.0
            && self.color.max_element() > 0.0
            && self.position.length_squared().is_finite()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spot_light_keeps_its_cone_ordered() {
        // Authored the wrong way round: the inner angle must still end up inside the outer
        // one, or the falloff lights the outside of the cone and nothing inside it.
        let light = LocalLight::spot(
            Vec3::ZERO,
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::ONE,
            1.0,
            10.0,
            0.8,
            0.4,
        );
        assert!(light.inner_cos >= light.outer_cos);
        assert!((light.outer_angle() - 0.8).abs() < 1e-4);
    }

    #[test]
    fn a_local_light_does_not_cast_shadows_unless_it_is_asked_to() {
        // The budget is small and the cost is per light, so this default is the one that
        // keeps a dungeon full of torches affordable.
        let light = LocalLight::point(Vec3::ZERO, Vec3::ONE, 1.0, 8.0);
        assert!(!light.casts_shadow);
        assert!(light.casting_shadows(3).casts_shadow);
    }

    #[test]
    fn a_light_with_nothing_to_contribute_is_not_significant() {
        let mut light = LocalLight::point(Vec3::ZERO, Vec3::ONE, 1.0, 8.0);
        assert!(light.is_significant());
        light.intensity = 0.0;
        assert!(!light.is_significant());
    }

    #[test]
    fn the_default_light_leaves_unlit_faces_visible() {
        // Ambient exists so a single directional light does not render half of every object
        // as pure black — there is no bounce lighting to fill it in.
        let lighting = Lighting::default();
        assert!(lighting.ambient_intensity > 0.0);
        assert!(lighting.direction.y < 0.0, "the sun points downward");
    }
}
