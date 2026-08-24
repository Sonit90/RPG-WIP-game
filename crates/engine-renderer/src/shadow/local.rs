//! Local-light shadows: which lights get a map, and what they render with.
//!
//! Pure CPU, like [`super::cascade`]. Two jobs:
//!
//! * **Selection.** More lights ask for shadows than there are atlas tiles, always. §21 asks
//!   for a mechanism rather than a rule of thumb, and this is it: reject the lights that
//!   cannot matter (out of view, out of range, not asking), score the rest with
//!   [`shadow_importance`], and spend the budget greedily on the highest scores. It **skips
//!   rather than stops**: a point light that does not fit is passed over and a later spot
//!   light that does fit still gets its view.
//! * **Projection.** A spot light is one perspective view down its cone. A point light is
//!   six, one per cube face, which is why it costs six times as much and why it is separately
//!   switchable in the settings.

use engine_core::math::{Frustum, Mat4, Sphere, Vec3};
use engine_render_data::{LocalLight, LocalLightKind};

use super::settings::ShadowSettings;

/// The six directions a point light's faces look along, in cube-map order.
///
/// `+X, -X, +Y, -Y, +Z, -Z`, which is the order §23 names and the order the shader's face
/// selection produces from the major axis of the light-to-surface vector.
pub const CUBE_FACE_DIRECTIONS: [Vec3; 6] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(-1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, -1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(0.0, 0.0, -1.0),
];

/// Up vectors for the six faces, chosen so no face looks along its own up vector.
const CUBE_FACE_UPS: [Vec3; 6] = [
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
];

/// How near a local shadow view's near plane sits, in metres.
///
/// Local shadow maps are perspective, so the near plane is where the depth precision goes.
/// Ten centimetres is inside any lamp housing and far enough out to leave usable precision
/// across a ten-metre range.
pub const LOCAL_SHADOW_NEAR: f32 = 0.1;

/// One shadow view a local light needs rendered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalShadowView {
    /// Which light in the frame's light list this belongs to.
    pub light: usize,
    /// Which cube face, for a point light. `None` for a spot light's single view.
    pub face: Option<u8>,
    /// What the depth pass renders with and the lighting pass samples with.
    pub view_projection: Mat4,
    /// The world-space sphere this view can possibly contain, for culling its casters.
    pub bounds: Sphere,
    /// The view's far plane, which is the light's range.
    pub far: f32,
}

impl LocalShadowView {
    /// The frustum to cull this view's casters against.
    #[must_use]
    pub fn frustum(&self) -> Frustum {
        Frustum::from_view_projection(&self.view_projection)
    }
}

/// How much one step of [`LocalLight::priority`] is worth, as a multiplier on importance.
///
/// Four. A boss's brazier at priority 1 outranks a decorative one that is four times as
/// important by every physical measure — which, since importance falls with the square of
/// distance, is a decorative light standing twice as close. Two steps of priority is four
/// times as close. That is the intended shape: priority is a thumb on the scale and not an
/// override, so a designer marking something important cannot accidentally spend the whole
/// budget on a light that is behind the player and a hundred metres away.
const PRIORITY_WEIGHT: f32 = 4.0;

/// How much a shadow map for this light is worth, larger being more.
///
/// § "do not arbitrarily choose the first N lights". The formula is one number out of five
/// inputs, and each of them earns its place:
///
/// ```text
/// intensity          a dim light's shadow is a faint shadow
/// range / distance   the light's angular size on screen: this is the screen-space
///                    influence term, and it is squared because area is
/// range fade         a light at the edge of local.max_distance is worth nothing, and
///                    fading rather than cutting stops a map appearing and vanishing as
///                    the player walks a step back and forth
/// priority           the gameplay term, PRIORITY_WEIGHT per step
/// ```
///
/// Distance is measured to the light's *reach* rather than to the light, for the reason the
/// range test uses the same measure: a big brazier lighting the room the player stands in is
/// not far away because its centre is.
#[must_use]
pub fn shadow_importance(light: &LocalLight, camera_position: Vec3, max_distance: f32) -> f32 {
    let distance = (light.position - camera_position).length();
    // Never divide by less than a tenth of the range: inside the light, the ratio would run
    // away and one torch would outrank everything in the level.
    let apparent = light.range / distance.max(light.range * 0.1);
    let reach = (distance - light.range).max(0.0);
    let fade = if max_distance > 0.0 {
        (1.0 - reach / max_distance).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let gameplay = PRIORITY_WEIGHT.powi(i32::from(light.priority));
    light.intensity.max(0.0) * apparent * apparent * fade * gameplay
}

/// A light that won a shadow map, and the views it spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedLight {
    /// Index into the frame's light list.
    pub light: usize,
    /// Index of this light's first view in the frame's view list.
    pub first_view: usize,
    /// How many views it uses: one for a spot, six for a point.
    pub view_count: usize,
}

/// The projection every local shadow view shares, for a light of the given cone and range.
///
/// A spot light's field of view is *twice* its outer half-angle, plus a margin: a projection
/// that exactly matched the cone would leave the cone's rim on the very edge texel, where
/// the PCF kernel reads outside the map.
#[must_use]
fn local_projection(fov_y: f32, far: f32) -> Mat4 {
    Mat4::perspective(
        fov_y.clamp(0.02, std::f32::consts::PI - 0.02),
        1.0,
        LOCAL_SHADOW_NEAR,
        far.max(LOCAL_SHADOW_NEAR + 0.01),
    )
}

/// The single view a spot light renders with.
#[must_use]
pub fn spot_view(light: &LocalLight) -> LocalShadowView {
    let direction = if light.direction.length_squared() > 1e-6 {
        light.direction.normalize()
    } else {
        Vec3::new(0.0, -1.0, 0.0)
    };
    let up = if direction.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let view = Mat4::look_at(light.position, light.position + direction, up);
    // A margin of ten per cent on the cone angle, so the outermost lit texel still has a
    // full PCF footprint inside the map.
    let fov = (light.outer_angle() * 2.2).max(0.05);
    LocalShadowView {
        light: usize::MAX,
        face: None,
        view_projection: local_projection(fov, light.range) * view,
        bounds: Sphere::new(light.position, light.range),
        far: light.range,
    }
}

/// One of the six views a point light renders with.
#[must_use]
pub fn point_face_view(light: &LocalLight, face: u8) -> LocalShadowView {
    let index = (face as usize).min(5);
    let direction = CUBE_FACE_DIRECTIONS[index];
    let view = Mat4::look_at(
        light.position,
        light.position + direction,
        CUBE_FACE_UPS[index],
    );
    // Ninety degrees exactly: the six faces have to meet, and a margin here would overlap
    // them instead — the face is chosen by the major axis, so a sample never lands outside
    // the face it selected.
    LocalShadowView {
        light: usize::MAX,
        face: Some(index as u8),
        view_projection: local_projection(std::f32::consts::FRAC_PI_2, light.range) * view,
        bounds: Sphere::new(light.position, light.range),
        far: light.range,
    }
}

/// Which cube face a direction from the light falls on.
///
/// The CPU mirror of what the shader does, so a test can assert the two agree without a GPU.
#[must_use]
pub fn cube_face_of(direction: Vec3) -> u8 {
    let (x, y, z) = (direction.x.abs(), direction.y.abs(), direction.z.abs());
    if x >= y && x >= z {
        u8::from(direction.x < 0.0)
    } else if y >= z {
        2 + u8::from(direction.y < 0.0)
    } else {
        4 + u8::from(direction.z < 0.0)
    }
}

/// Chooses which shadow-casting lights actually get a map this frame.
///
/// ```text
/// every light that asked
///      | not significant, out of range, or not touching the view -> dropped
///      v
/// candidates
///      | sorted: higher priority first, then nearer first
///      v
/// greedily assigned views until the budget is spent
/// ```
///
/// Greedy rather than exact: a light that does not fit is skipped and the next one is
/// considered, so a point light that cannot afford its six views does not block four spot
/// lights that can. `selected` and `views` are the caller's, reused across frames.
pub fn select_shadow_lights(
    lights: &[LocalLight],
    camera_position: Vec3,
    camera_frustum: &Frustum,
    settings: &ShadowSettings,
    scratch: &mut Vec<(usize, f32)>,
    selected: &mut Vec<SelectedLight>,
    views: &mut Vec<LocalShadowView>,
) {
    selected.clear();
    views.clear();
    scratch.clear();

    let budget = settings.local_view_budget();
    if budget == 0 {
        return;
    }

    for (index, light) in lights.iter().enumerate() {
        if !light.casts_shadow || !light.is_significant() {
            continue;
        }
        if light.kind == LocalLightKind::Point && !settings.local.point_lights {
            continue;
        }
        let distance = (light.position - camera_position).length();
        // Distance to the light's *reach*, not to the light: a big brazier lighting a room
        // the player is standing in is not "far away" because its centre is.
        if distance - light.range > settings.local.max_distance {
            continue;
        }
        // A light that cannot touch the view cannot cast a shadow anybody sees.
        if !camera_frustum.intersects_sphere(&Sphere::new(light.position, light.range)) {
            continue;
        }
        let importance = shadow_importance(light, camera_position, settings.local.max_distance);
        // The floor. A budget with room left in it is otherwise always spent, and the last
        // views go to lights whose shadow is a few hundred texels of nothing.
        if importance < settings.local.min_importance {
            continue;
        }
        scratch.push((index, importance));
    }

    // Most important first. `sort_by` rather than `sort_unstable_by` so two lights that tie
    // keep their authored order — otherwise which of them gets the last shadow map could
    // change frame to frame, and a shadow map that appears and disappears is worse than one
    // that was never there.
    scratch.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for &(index, _) in scratch.iter() {
        let light = &lights[index];
        let needed = match light.kind {
            LocalLightKind::Spot => 1,
            LocalLightKind::Point => 6,
        };
        if views.len() + needed > budget {
            continue;
        }
        let first_view = views.len();
        match light.kind {
            LocalLightKind::Spot => {
                let mut view = spot_view(light);
                view.light = index;
                views.push(view);
            }
            LocalLightKind::Point => {
                for face in 0..6 {
                    let mut view = point_face_view(light, face);
                    view.light = index;
                    views.push(view);
                }
            }
        }
        selected.push(SelectedLight {
            light: index,
            first_view,
            view_count: needed,
        });
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::shadow::settings::{LocalShadowSettings, ShadowPreset};
    use engine_render_data::{Camera, Projection};

    fn camera() -> Camera {
        let mut camera = Camera::new(Projection::perspective(16.0 / 9.0)).expect("valid aspect");
        camera.set_position(Vec3::ZERO);
        camera
    }

    fn frustum(camera: &Camera) -> Frustum {
        Frustum::from_view_projection(&camera.view_projection())
    }

    fn spot_at(position: Vec3, priority: i16) -> LocalLight {
        LocalLight::spot(
            position,
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::ONE,
            2.0,
            10.0,
            0.3,
            0.5,
        )
        .casting_shadows(priority)
    }

    fn select(
        lights: &[LocalLight],
        settings: &ShadowSettings,
    ) -> (Vec<SelectedLight>, Vec<LocalShadowView>) {
        let camera = camera();
        let mut scratch = Vec::new();
        let mut selected = Vec::new();
        let mut views = Vec::new();
        select_shadow_lights(
            lights,
            camera.position(),
            &frustum(&camera),
            settings,
            &mut scratch,
            &mut selected,
            &mut views,
        );
        (selected, views)
    }

    #[test]
    fn a_light_that_did_not_ask_gets_no_shadow_map() {
        let settings = ShadowPreset::High.settings();
        let quiet = LocalLight::point(Vec3::new(0.0, 2.0, -5.0), Vec3::ONE, 1.0, 8.0);
        let (selected, views) = select(&[quiet], &settings);
        assert!(selected.is_empty());
        assert!(views.is_empty());
    }

    #[test]
    fn higher_priority_wins_the_last_map() {
        let settings = ShadowSettings {
            local: super::super::settings::LocalShadowSettings {
                max_views: 1,
                ..ShadowPreset::High.settings().local
            },
            ..ShadowPreset::High.settings()
        };
        // The important light is further away, and must still win.
        let near = spot_at(Vec3::new(0.0, 2.0, -3.0), 0);
        let important = spot_at(Vec3::new(0.0, 2.0, -12.0), 5);
        let (selected, views) = select(&[near, important], &settings);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].light, 1, "priority must outrank distance");
        assert_eq!(views.len(), 1);
    }

    #[test]
    fn distance_decides_between_equal_priorities() {
        let settings = ShadowSettings {
            local: super::super::settings::LocalShadowSettings {
                max_views: 1,
                ..ShadowPreset::High.settings().local
            },
            ..ShadowPreset::High.settings()
        };
        let far = spot_at(Vec3::new(0.0, 2.0, -20.0), 1);
        let near = spot_at(Vec3::new(0.0, 2.0, -4.0), 1);
        let (selected, _) = select(&[far, near], &settings);
        assert_eq!(selected[0].light, 1);
    }

    #[test]
    fn the_budget_is_counted_in_views_not_lights() {
        // A point light spends six. With a budget of four it cannot be afforded at all, and
        // the spot lights behind it must still get theirs — which is what "skip, do not
        // stop" buys. Ordering matters here: the point light asks *first*, with the highest
        // priority, and being unaffordable must not stop the queue.
        let settings = ShadowSettings {
            local: super::super::settings::LocalShadowSettings {
                max_views: 4,
                ..ShadowPreset::High.settings().local
            },
            ..ShadowPreset::High.settings()
        };
        let point =
            LocalLight::point(Vec3::new(0.0, 2.0, -5.0), Vec3::ONE, 2.0, 8.0).casting_shadows(9);
        let lights = vec![
            point,
            spot_at(Vec3::new(1.0, 2.0, -5.0), 1),
            spot_at(Vec3::new(2.0, 2.0, -5.0), 1),
        ];
        let (selected, views) = select(&lights, &settings);
        assert_eq!(views.len(), 2, "two spots, and the point light skipped");
        assert!(selected.iter().all(|light| light.light != 0));
    }

    #[test]
    fn a_point_light_spends_six_views_in_cube_order() {
        let settings = ShadowSettings {
            local: super::super::settings::LocalShadowSettings {
                max_views: 6,
                ..ShadowPreset::Ultra.settings().local
            },
            ..ShadowPreset::Ultra.settings()
        };
        let point =
            LocalLight::point(Vec3::new(0.0, 2.0, -5.0), Vec3::ONE, 2.0, 8.0).casting_shadows(1);
        let (selected, views) = select(&[point], &settings);
        assert_eq!(selected[0].view_count, 6);
        assert_eq!(views.len(), 6);
        for (index, view) in views.iter().enumerate() {
            assert_eq!(view.face, Some(index as u8));
        }
    }

    #[test]
    fn point_lights_can_be_switched_off_without_touching_spot_lights() {
        let settings = ShadowPreset::Medium.settings();
        assert!(!settings.local.point_lights);
        let point =
            LocalLight::point(Vec3::new(0.0, 2.0, -5.0), Vec3::ONE, 2.0, 8.0).casting_shadows(9);
        let (selected, _) = select(&[point, spot_at(Vec3::new(0.0, 2.0, -5.0), 0)], &settings);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].light, 1);
    }

    #[test]
    fn a_light_behind_the_camera_is_not_given_a_map() {
        let settings = ShadowPreset::High.settings();
        // The camera looks down -Z, so +Z is behind it, well outside the light's range.
        let behind = spot_at(Vec3::new(0.0, 2.0, 60.0), 5);
        let (selected, _) = select(&[behind], &settings);
        assert!(selected.is_empty());
    }

    #[test]
    fn a_light_further_than_the_cutoff_is_not_given_a_map() {
        let settings = ShadowPreset::High.settings();
        let distant = spot_at(Vec3::new(0.0, 2.0, -settings.local.max_distance - 40.0), 5);
        let (selected, _) = select(&[distant], &settings);
        assert!(selected.is_empty());
    }

    #[test]
    fn a_spot_view_contains_its_own_cone() {
        let light = spot_at(Vec3::new(0.0, 5.0, 0.0), 0);
        let view = spot_view(&light);
        let frustum = view.frustum();
        // Straight down the axis, and out at the cone's rim.
        assert!(frustum.contains_point(Vec3::new(0.0, 1.0, 0.0)));
        let rim = light.outer_angle().tan() * 4.0;
        assert!(
            frustum.contains_point(Vec3::new(rim, 1.0, 0.0)),
            "the cone's rim fell outside its own shadow projection"
        );
    }

    #[test]
    fn the_six_faces_cover_every_direction() {
        // Every face's own axis has to select that face, and every view has to contain a
        // point along its axis — otherwise a point light has a blind spot, which renders as
        // a wedge of the room that is never shadowed.
        let light = LocalLight::point(Vec3::new(0.0, 3.0, 0.0), Vec3::ONE, 1.0, 10.0);
        for (index, direction) in CUBE_FACE_DIRECTIONS.iter().enumerate() {
            assert_eq!(cube_face_of(*direction), index as u8);
            let view = point_face_view(&light, index as u8);
            assert!(
                view.frustum()
                    .contains_point(light.position + *direction * 2.0),
                "face {index} does not contain the direction it looks along"
            );
        }
    }

    #[test]
    fn selection_reuses_its_buffers() {
        let settings = ShadowPreset::Ultra.settings();
        let lights: Vec<LocalLight> = (0..settings.local_view_budget())
            .map(|index| spot_at(Vec3::new(index as f32, 2.0, -5.0), 0))
            .collect();
        let camera = camera();
        let (mut scratch, mut selected, mut views) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..4 {
            select_shadow_lights(
                &lights,
                camera.position(),
                &frustum(&camera),
                &settings,
                &mut scratch,
                &mut selected,
                &mut views,
            );
        }
        let capacity = views.capacity();
        select_shadow_lights(
            &lights,
            camera.position(),
            &frustum(&camera),
            &settings,
            &mut scratch,
            &mut selected,
            &mut views,
        );
        assert_eq!(views.capacity(), capacity);
        assert_eq!(views.len(), settings.local_view_budget());
    }

    #[test]
    fn importance_falls_with_the_square_of_distance() {
        // The screen-space influence term. A light twice as far away covers a quarter of the
        // screen and is worth a quarter as much.
        let near = spot_at(Vec3::new(0.0, 0.0, -10.0), 0);
        let far = spot_at(Vec3::new(0.0, 0.0, -20.0), 0);
        let near_score = shadow_importance(&near, Vec3::ZERO, 40.0);
        let far_score = shadow_importance(&far, Vec3::ZERO, 40.0);
        let ratio = near_score / far_score;
        assert!(
            (2.0..8.0).contains(&ratio),
            "a light at half the distance scored {ratio}x, which is not an inverse square              plus a range fade"
        );
    }

    #[test]
    fn a_brighter_light_outranks_a_dimmer_one_at_the_same_place() {
        let place = Vec3::new(3.0, 0.0, -12.0);
        let dim = LocalLight {
            intensity: 1.0,
            ..spot_at(place, 0)
        };
        let bright = LocalLight {
            intensity: 6.0,
            ..spot_at(place, 0)
        };
        assert!(
            shadow_importance(&bright, Vec3::ZERO, 40.0)
                > shadow_importance(&dim, Vec3::ZERO, 40.0)
        );
    }

    #[test]
    fn a_wider_light_outranks_a_narrower_one_at_the_same_place() {
        let place = Vec3::new(0.0, 0.0, -15.0);
        let small = LocalLight {
            range: 4.0,
            ..spot_at(place, 0)
        };
        let large = LocalLight {
            range: 12.0,
            ..spot_at(place, 0)
        };
        assert!(
            shadow_importance(&large, Vec3::ZERO, 40.0)
                > shadow_importance(&small, Vec3::ZERO, 40.0)
        );
    }

    #[test]
    fn a_boss_light_outranks_a_nearer_decorative_one() {
        // The case §21 names. The boss's light is three times as far away — a ninth of the
        // screen influence — and its priority has to carry it anyway.
        let settings = ShadowSettings {
            local: LocalShadowSettings {
                max_views: 1,
                ..ShadowPreset::Ultra.settings().local
            },
            ..ShadowPreset::Ultra.settings()
        }
        .validated();
        let decorative = spot_at(Vec3::new(0.0, 0.0, -8.0), 0);
        let boss = spot_at(Vec3::new(0.0, 0.0, -24.0), 2);
        let (selected, _) = select(&[decorative, boss], &settings);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].light, 1, "the boss light did not win the map");
    }

    #[test]
    fn priority_is_a_thumb_on_the_scale_and_not_an_override() {
        // The other half of the same decision. A priority-1 light on the far side of the
        // world must not beat the light burning next to the player, or a designer marking
        // something important spends the whole budget on something nobody can see.
        let settings = ShadowSettings {
            local: LocalShadowSettings {
                max_views: 1,
                min_importance: 0.0,
                ..ShadowPreset::Ultra.settings().local
            },
            ..ShadowPreset::Ultra.settings()
        }
        .validated();
        let close = spot_at(Vec3::new(0.0, 0.0, -3.0), 0);
        let distant = spot_at(Vec3::new(0.0, 0.0, -38.0), 1);
        let (selected, _) = select(&[close, distant], &settings);
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].light, 0,
            "a distant priority light took the map"
        );
    }

    #[test]
    fn a_light_below_the_importance_floor_gets_no_map_even_with_the_budget_free() {
        // The floor, which is not the ceiling. There are fifteen views going spare here and
        // the light still must not get one: its shadow would be a few hundred texels of
        // nothing.
        let base = ShadowPreset::Ultra.settings();
        let settings = ShadowSettings {
            local: LocalShadowSettings {
                min_importance: 1.0e6,
                ..base.local
            },
            ..base
        }
        .validated();
        let (selected, views) = select(&[spot_at(Vec3::new(0.0, 0.0, -12.0), 0)], &settings);
        assert!(selected.is_empty(), "the floor did not reject the light");
        assert!(views.is_empty());

        // And with the floor at zero the same light is selected, so the test above is about
        // the floor and not about some other rejection.
        let permissive = ShadowSettings {
            local: LocalShadowSettings {
                min_importance: 0.0,
                ..base.local
            },
            ..base
        }
        .validated();
        let (selected, _) = select(&[spot_at(Vec3::new(0.0, 0.0, -12.0), 0)], &permissive);
        assert_eq!(selected.len(), 1);
    }
}
