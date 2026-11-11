//! The shadow test course: six bays, each built to make one shadow failure visible.
//!
//! ```text
//!                 x = -60      x = -20       x = +20       x = +60
//!
//!   z = -70    A  one pillar                             B  a hundred crates
//!              on empty ground                              in a grid
//!
//!   z = -20    C  an arch and a rock spire, with rocks receding to the horizon
//!              (the cascade transition lives here)
//!
//!   z = +20    D  leaf screens          E  spot lights on   F  braziers ringing
//!                 (alpha-tested)           a colonnade         a crate stack
//!
//!   z = +55                        (P)  player spawn, facing -Z
//! ```
//!
//! # Why this is a scene and not a test
//!
//! Everything about shadows that can be decided by arithmetic is decided by arithmetic, in
//! `engine_renderer::shadow`'s unit tests and in `tests/shadow_render.rs`, which renders to
//! an offscreen target and reads pixels back. What none of that can answer is whether the
//! result *looks* right: whether peter-panning is visible at the scale a character is drawn
//! at, whether a cascade edge shows up as a line across the ground when you walk towards it,
//! whether the shadow of a leaf reads as a leaf. Those are judgements made with eyes, and a
//! judgement made with eyes needs somewhere to stand.
//!
//! So each bay isolates one question, and they are far enough apart that walking from one to
//! the next is a deliberate act rather than something that happens by accident:
//!
//! | Bay | What stands there | What it is for |
//! | --- | --- | --- |
//! | A | one pillar, nothing else within twenty metres | acne, peter-panning, bias. The control. |
//! | B | a hundred crates in a grid | caster culling and the shadow LOD, and what a hundred casters cost |
//! | C | an arch, a spire, and rocks receding past the far cascade | cascade splits, blending, and stability while walking |
//! | D | leaf screens | alpha-tested casters: the silhouette comes from the texture |
//! | E | a colonnade under two spot lights | local shadow views, and the budget refusing the third |
//! | F | braziers around a stack | point lights: six faces each, and what the budget does about it |
//!
//! # Only the layout lives here
//!
//! The same rule the rest of this crate follows. This module places objects and lights; it
//! spawns nothing and knows nothing about a renderer, a shadow map or a cascade. What makes
//! it a *shadow* course is only which objects it chose and where it put them.

use engine_core::math::Vec3;
use engine_core::transform::Transform;

use crate::asset::{
    ARCH, BRAZIER, GROUND_PLAIN, LEAF_SCREEN, PILLAR, ROCK_MEDIUM, ROCK_SMALL, ROCK_SPIRE,
    SKY_DOME, SUPPLY_CRATE,
};
use crate::lighting::{EnvironmentLighting, SceneLight};
use crate::object::{EnvironmentCollision, EnvironmentObject};
use crate::scene::{EnvironmentScene, SpawnPoints};
use crate::{hash_range, hash01};

/// Half-width of the square the player is confined to, in metres.
///
/// Wider than the arena's forty, because bay C exists to be walked *towards*: a cascade
/// transition is a thing that happens at a distance, and a course whose far wall is inside
/// the second cascade cannot show one.
pub const COURSE_HALF_EXTENT: f32 = 90.0;

/// Where the player starts. South of every bay, looking down the course.
pub const PLAYER_SPAWN: Vec3 = Vec3::new(0.0, 0.0, 55.0);

/// Where to stand to watch bay C's cascade transitions, looking down -Z.
///
/// Not the spawn. Cascade distances are measured from the camera, so "does the near split
/// show a seam" is a question about a particular place to stand — the near end of the
/// receding line — and a course that only names its spawn point makes that place something
/// each person has to find again.
pub const CASCADE_WALK: Vec3 = Vec3::new(-4.0, 0.0, -18.0);

/// How many crates stand in bay B.
///
/// A hundred, because the questions that bay asks are "does culling drop the ones behind
/// the camera" and "does the LOD swap the far ones down", and both are invisible with ten.
/// It is also, deliberately, about the number of casters a real fight has in it.
pub const CRATE_GRID: usize = 10;

/// The shadow test course.
///
/// Deterministic, like every scene in this crate: two calls produce byte-identical scenes.
#[must_use]
pub fn shadow_course() -> EnvironmentScene {
    let mut scene = EnvironmentScene::with_capacity("shadow_course", 256);
    scene.half_extent = COURSE_HALF_EXTENT;
    // The same afternoon sun the arena uses. A shadow course lit by a bespoke light would
    // be answering a question about a light nothing else in the game has.
    scene.lighting = EnvironmentLighting::arena_afternoon();
    scene.ground_height = 0.0;
    scene.spawns = SpawnPoints {
        player: PLAYER_SPAWN,
        featured_dummy: Vec3::new(0.0, 0.0, 40.0),
        group_center: Vec3::new(0.0, 0.0, 44.0),
        group_spacing: 3.0,
    };

    push_ground(&mut scene);
    push_bay_a_isolated_caster(&mut scene);
    push_bay_b_many_casters(&mut scene);
    push_bay_c_distance(&mut scene);
    push_bay_d_foliage(&mut scene);
    push_bay_e_spot_lights(&mut scene);
    push_bay_f_point_lights(&mut scene);
    scene
}

/// The sky and one flat plain, and nothing else underfoot.
///
/// Flat on purpose. Every other test bed here has terrain in it, and terrain hides shadow
/// bugs: acne on a slope reads as shading, and a shadow that starts a few centimetres away
/// from its caster is invisible unless the ground under the caster is level.
fn push_ground(scene: &mut EnvironmentScene) {
    scene.push(EnvironmentObject::decoration(SKY_DOME, Vec3::ZERO));
    scene.push(EnvironmentObject {
        asset: GROUND_PLAIN,
        transform: Transform::from_translation(Vec3::ZERO).with_scale(Vec3::new(640.0, 1.0, 640.0)),
        collision: EnvironmentCollision::None,
    });
}

/// Bay A: one pillar on empty ground.
///
/// The control, and the only bay whose answer is a single number: the depth and slope bias.
/// Too little and the pillar's own face is striped with acne; too much and its shadow starts
/// half a metre out from its base and the pillar appears to float. Both are obvious here and
/// only here, because there is nothing else within twenty metres to confuse them with.
fn push_bay_a_isolated_caster(scene: &mut EnvironmentScene) {
    scene.push(
        EnvironmentObject::decoration(PILLAR, Vec3::new(-60.0, 0.0, -70.0))
            .with_collision(EnvironmentCollision::standing(0.6, 4.2, 0.6)),
    );
    // One small rock beside it, at the scale of a character's foot. A metre-wide caster and
    // a five-metre one need different amounts of bias, and the pair is what shows it.
    scene.push(EnvironmentObject::decoration(
        ROCK_SMALL,
        Vec3::new(-56.0, 0.0, -66.0),
    ));
}

/// Bay B: a hundred crates in a grid.
///
/// Regular spacing, because irregular spacing hides a missing shadow. When the caster cull
/// or the shadow LOD drops something it should not have, a grid shows a hole in a pattern
/// and a scatter shows nothing at all.
fn push_bay_b_many_casters(scene: &mut EnvironmentScene) {
    const SPACING: f32 = 2.6;
    let origin = Vec3::new(46.0, 0.0, -84.0);
    for row in 0..CRATE_GRID {
        for column in 0..CRATE_GRID {
            let index = (row * CRATE_GRID + column) as u32;
            let position = Vec3::new(
                origin.x + column as f32 * SPACING,
                // Every fifth crate is stacked on another one. A shadow cast onto a surface
                // that is not the ground is the case a cascade's near plane gets wrong, and
                // a field of objects all standing on the same plane never exercises it.
                f32::from(index % 5 == 0) * 0.9,
                origin.z + row as f32 * SPACING,
            );
            scene.push(
                EnvironmentObject::decoration(SUPPLY_CRATE, position).with_yaw(hash_range(
                    index,
                    3,
                    0.0,
                    std::f32::consts::TAU,
                )),
            );
        }
    }
}

/// Bay C: geometry receding to the horizon.
///
/// The cascade bay. An arch and a spire near the walkway, then rocks stepping away in a line
/// that crosses every cascade boundary the default split gives — 15 m, 50 m, 150 m — so that
/// walking down it moves each rock from one cascade to the next while you watch its shadow.
///
/// What to look for: the shadow's resolution stepping down is expected and fine; the shadow
/// *moving*, or a visible line across the ground where one cascade meets the next, is not.
/// The first is what cascades are, the second is what stabilisation and blending are for.
fn push_bay_c_distance(scene: &mut EnvironmentScene) {
    scene.push(
        EnvironmentObject::decoration(ARCH, Vec3::new(-14.0, 0.0, -20.0))
            .with_collision(EnvironmentCollision::standing(2.4, 6.0, 0.8)),
    );
    scene.push(
        EnvironmentObject::decoration(ROCK_SPIRE, Vec3::new(6.0, 0.0, -22.0))
            .with_collision(EnvironmentCollision::standing(1.1, 3.6, 1.1)),
    );

    // The receding line. Spaced so the gaps grow with distance, which keeps the apparent
    // spacing roughly even from the walkway and puts a rock either side of every split.
    let mut distance = 6.0_f32;
    let mut index = 0_u32;
    while distance < 260.0 {
        let sway = hash_range(index, 17, -3.0, 3.0);
        let asset = if hash01(index, 29) > 0.5 {
            ROCK_MEDIUM
        } else {
            ROCK_SPIRE
        };
        scene.push(
            EnvironmentObject::decoration(asset, Vec3::new(-4.0 + sway, 0.0, -24.0 - distance))
                .with_yaw(hash_range(index, 31, 0.0, std::f32::consts::TAU)),
        );
        distance *= 1.32;
        index += 1;
    }
}

/// Bay D: leaf screens.
///
/// The only bay whose casters are not solid. A leaf screen is six quads and a texture with
/// holes in it, so its shadow is correct only if the depth pass runs a pixel shader and
/// discards the transparent texels — get it wrong and the shadow is six flat rectangles,
/// which is not a subtle failure once you are looking for it.
///
/// Two clusters at different distances, because the alpha test also has to survive the
/// shadow LOD: a caster far enough away to drop a level is still an alpha-tested caster.
fn push_bay_d_foliage(scene: &mut EnvironmentScene) {
    for cluster in 0..2_u32 {
        let center = Vec3::new(-52.0, 0.0, 22.0 + cluster as f32 * 26.0);
        for index in 0..9_u32 {
            let seed = cluster * 32 + index;
            let position = center
                + Vec3::new(
                    hash_range(seed, 41, -6.0, 6.0),
                    0.0,
                    hash_range(seed, 43, -6.0, 6.0),
                );
            scene.push(
                EnvironmentObject::decoration(LEAF_SCREEN, position)
                    .with_yaw(hash_range(seed, 47, 0.0, std::f32::consts::TAU))
                    .with_scale(hash_range(seed, 53, 0.8, 1.5)),
            );
        }
    }
}

/// Bay E: a colonnade under spot lights.
///
/// Three pillars in a row and three spot lights aimed along it, of which the local shadow
/// budget will grant two at Medium and all three at High. That is the point: the third
/// pillar keeping its shadow when the quality goes up, and losing it when the quality comes
/// down, is what makes "the budget is real" something you can see rather than read.
///
/// The lights are tilted rather than vertical. A spot light straight down puts its shadow
/// directly under the caster, where it is hidden by the caster.
fn push_bay_e_spot_lights(scene: &mut EnvironmentScene) {
    const OUTER: f32 = 0.55;
    for index in 0..3_u32 {
        let x = 6.0 + index as f32 * 9.0;
        scene.push(
            EnvironmentObject::decoration(PILLAR, Vec3::new(x, 0.0, 26.0))
                .with_collision(EnvironmentCollision::standing(0.6, 4.2, 0.6)),
        );
        // A crate beside each pillar: something small for the pillar's shadow to fall across,
        // so the shadow is read on a surface rather than only on the floor.
        scene.push(EnvironmentObject::decoration(
            SUPPLY_CRATE,
            Vec3::new(x + 2.4, 0.0, 29.0),
        ));
        // Priority descending, so which light loses its shadow map when the budget tightens
        // is decided rather than incidental — and so it is always the same one.
        scene.push_light(
            SceneLight::spot(
                Vec3::new(x - 1.5, 7.0, 21.0),
                Vec3::new(0.25, -1.0, 0.5),
                26.0,
                OUTER,
            )
            .with_color(Vec3::new(0.85, 0.9, 1.0), 6.0)
            .casting_shadows(10 - index as i16),
        );
    }
}

/// Bay F: braziers around a stack.
///
/// Point lights, which cost six shadow views each — the whole High budget for one of them.
/// Four braziers stand here and all four ask; what the renderer does about that is the thing
/// being looked at. Expect one of them to cast, the rest to light without casting, and the
/// difference to be visible on the crates in the middle.
///
/// **Walk into the bay.** Local shadows have a range — 40 m at every preset that has them —
/// so from the spawn, sixty metres away, none of these are granted anything and the bay
/// looks like four unremarkable braziers. That is the range doing its job, not a bug, and it
/// is the reason this bay cannot be judged from where the course starts.
fn push_bay_f_point_lights(scene: &mut EnvironmentScene) {
    let center = Vec3::new(60.0, 0.0, 30.0);
    // Something in the middle for the light to be blocked by, at several heights: a point
    // light's shadow is six faces, and a stack that only reaches a metre never exercises the
    // top face at all.
    for (index, (x, y, z)) in [
        (0.0, 0.0, 0.0),
        (1.1, 0.0, 0.4),
        (0.5, 0.95, 0.2),
        (-0.9, 0.0, 0.9),
    ]
    .into_iter()
    .enumerate()
    {
        scene.push(
            EnvironmentObject::decoration(SUPPLY_CRATE, center + Vec3::new(x, y, z))
                .with_yaw(hash_range(index as u32, 59, 0.0, std::f32::consts::TAU)),
        );
    }
    for index in 0..4_u32 {
        let angle = index as f32 / 4.0 * std::f32::consts::TAU + 0.4;
        let (sin, cos) = angle.sin_cos();
        let position = center + Vec3::new(cos * 6.5, 0.0, sin * 6.5);
        scene.push(
            EnvironmentObject::decoration(BRAZIER, position)
                .with_collision(EnvironmentCollision::standing(0.45, 1.35, 0.45)),
        );
        scene.push_light(
            SceneLight::brazier(position + Vec3::new(0.0, 1.45, 0.0), 16.0)
                .casting_shadows(4 - index as i16),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_course_is_the_same_course_every_time_it_is_built() {
        assert_eq!(shadow_course().signature(), shadow_course().signature());
    }

    #[test]
    fn every_bay_is_present_and_carries_what_it_was_built_to_carry() {
        let scene = shadow_course();
        // Bay B is the count that matters most: it is the one a stray `break` would silently
        // shrink into a scene that measures nothing.
        assert_eq!(
            scene.count_of(SUPPLY_CRATE),
            CRATE_GRID * CRATE_GRID + 3 + 4,
            "bay B's grid, bay E's crates and bay F's stack"
        );
        assert_eq!(scene.count_of(LEAF_SCREEN), 18, "two clusters of nine");
        assert!(scene.count_of(PILLAR) >= 4, "bay A's and bay E's");
        assert_eq!(scene.count_of(ARCH), 1);
        assert_eq!(scene.count_of(GROUND_PLAIN), 1);
        assert_eq!(scene.count_of(SKY_DOME), 1);
    }

    #[test]
    fn the_lights_ask_for_more_shadow_maps_than_any_preset_can_grant() {
        // The course is only a test of the budget if it overdraws it. Three spots (one view
        // each) and four point lights (six each) is 27 views against Ultra's 16.
        let scene = shadow_course();
        let asking: usize = scene
            .lights
            .iter()
            .filter(|light| light.casts_shadow)
            .map(|light| if light.cone_radians.is_some() { 1 } else { 6 })
            .sum();
        assert_eq!(asking, 27);
        assert!(
            scene.lights.iter().all(|light| light.intensity > 0.0),
            "a light with no intensity is a light that proves nothing"
        );
    }

    #[test]
    fn the_receding_line_crosses_every_cascade_boundary() {
        // The default splits are 15, 50 and 150 metres from the camera, measured from
        // [`CASCADE_WALK`]. A line that stops short of the last split cannot show a
        // transition at it, and one that starts past the first cannot show that one.
        let scene = shadow_course();
        let depths: Vec<f32> = scene
            .objects
            .iter()
            .filter(|object| object.asset == ROCK_MEDIUM || object.asset == ROCK_SPIRE)
            .map(|object| CASCADE_WALK.z - object.position().z)
            .collect();
        for boundary in [15.0, 50.0, 150.0] {
            assert!(
                depths.iter().any(|depth| *depth < boundary)
                    && depths.iter().any(|depth| *depth > boundary),
                "nothing straddles the {boundary} m split"
            );
        }
    }

    #[test]
    fn nothing_stands_where_the_player_spawns() {
        let scene = shadow_course();
        for object in &scene.objects {
            if object.asset == GROUND_PLAIN || object.asset == SKY_DOME {
                continue;
            }
            let offset = object.position() - PLAYER_SPAWN;
            let distance = (offset.x * offset.x + offset.z * offset.z).sqrt();
            assert!(distance > 6.0, "{} stands on the spawn", object.asset);
        }
    }
}
