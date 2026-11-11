//! The particle pass, drawn on a real GPU, asserted in pixels.
//!
//! Every other test of this system runs without a device: the pool integrates, the director
//! spawns, the draw list fills. None of that proves a particle reaches the screen — a
//! premultiplied blend with the wrong factors, a billboard basis taken from the wrong axis,
//! or an input layout that steps per vertex instead of per instance all produce a perfectly
//! healthy CPU state and a black frame.
//!
//! These tests are that proof:
//!
//! * a particle covers pixels at all, and where the camera is looking,
//! * additive blending *adds*, so two overlapping particles are brighter than one,
//! * a particle's colour reaches the screen rather than a white splat,
//! * a zero-size or fully faded particle draws nothing,
//! * and twenty thousand particles submit in one frame without failing.
//!
//! Every test skips itself when no D3D12 adapter is present.

#![cfg(windows)]

use engine_core::math::Vec3;
use engine_render_data::{Camera, ParticleBlend, ParticleDrawList, ParticleInstance, Projection};
use engine_renderer::offscreen::{CapturedImage, render_particles};

const WIDTH: u32 = 240;
const HEIGHT: u32 = 135;

/// A black background, so anything drawn is what the particle put there.
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Whether a D3D12 adapter exists. Without one there is nothing to test.
fn adapter_available() -> bool {
    engine_renderer::offscreen::render_triangle(8, 8, CLEAR).is_ok()
}

/// A camera at the origin looking down −Z, which is where the tests put their particles.
fn camera() -> Camera {
    let Ok(mut camera) = Camera::new(Projection::perspective(WIDTH as f32 / HEIGHT as f32)) else {
        panic!("a 16:9 perspective is valid");
    };
    let Ok(()) = camera.look_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0), Vec3::Y) else {
        panic!("the camera must look down -Z");
    };
    camera
}

/// One particle ten metres ahead of the camera.
fn particle(size: f32, color: [f32; 4]) -> ParticleInstance {
    ParticleInstance {
        position: [0.0, 0.0, -10.0],
        size,
        color,
        rotation: 0.0,
    }
}

/// The brightest pixel in the image, as linear-ish 0..1 per channel.
fn brightest(image: &CapturedImage) -> [f32; 3] {
    let mut best = [0.0_f32; 3];
    let mut brightest = 0.0_f32;
    for pixel in image.pixels.chunks_exact(4) {
        let value = f32::from(pixel[0]) + f32::from(pixel[1]) + f32::from(pixel[2]);
        if value > brightest {
            brightest = value;
            best = [
                f32::from(pixel[0]) / 255.0,
                f32::from(pixel[1]) / 255.0,
                f32::from(pixel[2]) / 255.0,
            ];
        }
    }
    best
}

/// How many pixels are not the clear colour.
fn covered(image: &CapturedImage) -> usize {
    image
        .pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[0] > 2 || pixel[1] > 2 || pixel[2] > 2)
        .count()
}

#[test]
fn a_particle_reaches_the_screen() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let mut list = ParticleDrawList::with_capacity(16);
    list.push(ParticleBlend::Additive, particle(1.5, [1.0, 0.6, 0.2, 1.0]));

    let Ok(image) = render_particles(WIDTH, HEIGHT, CLEAR, &list, &camera()) else {
        panic!("the particle pass must record and submit");
    };
    let lit = covered(&image);
    assert!(lit > 50, "a 1.5 m particle covered {lit} pixels");

    // And it is in front of the camera, not smeared across the whole frame: a soft round
    // falloff over a metre and a half at ten metres is a small bright blob.
    assert!(
        lit < (WIDTH * HEIGHT) as usize / 2,
        "one particle covered half the screen"
    );
}

#[test]
fn a_particle_carries_its_own_colour() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let mut list = ParticleDrawList::with_capacity(16);
    list.push(ParticleBlend::Additive, particle(2.0, [0.0, 0.0, 1.0, 1.0]));
    let Ok(image) = render_particles(WIDTH, HEIGHT, CLEAR, &list, &camera()) else {
        panic!("the particle pass must record and submit");
    };

    let colour = brightest(&image);
    assert!(colour[2] > 0.3, "a blue particle came out {colour:?}");
    assert!(
        colour[2] > colour[0] * 3.0,
        "the colour did not survive to the screen: {colour:?}"
    );
}

#[test]
fn additive_particles_add() {
    // The blend factor test that only a GPU can answer: `ONE, ONE` on a premultiplied
    // source. If the source factor ever became SRC_ALPHA the coverage would be applied
    // twice and a stack of particles would be dimmer than it should be, not brighter.
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let dim = [0.30, 0.30, 0.30, 1.0];

    let mut one = ParticleDrawList::with_capacity(16);
    one.push(ParticleBlend::Additive, particle(2.0, dim));
    let Ok(single) = render_particles(WIDTH, HEIGHT, CLEAR, &one, &camera()) else {
        panic!("the particle pass must record and submit");
    };

    let mut three = ParticleDrawList::with_capacity(16);
    for _ in 0..3 {
        three.push(ParticleBlend::Additive, particle(2.0, dim));
    }
    let Ok(stacked) = render_particles(WIDTH, HEIGHT, CLEAR, &three, &camera()) else {
        panic!("the particle pass must record and submit");
    };

    let single_peak = brightest(&single)[0];
    let stacked_peak = brightest(&stacked)[0];
    assert!(
        stacked_peak > single_peak * 1.5,
        "three overlapping particles ({stacked_peak}) were not much brighter than one \
         ({single_peak})"
    );
}

#[test]
fn an_invisible_particle_draws_nothing() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    // The draw list refuses both of these before they reach the GPU, and the frame that
    // results has to be the clear colour rather than a stray quad at the origin.
    let mut list = ParticleDrawList::with_capacity(16);
    list.push(ParticleBlend::Additive, particle(0.0, [1.0, 1.0, 1.0, 1.0]));
    list.push(ParticleBlend::Additive, particle(2.0, [1.0, 1.0, 1.0, 0.0]));
    assert!(list.is_empty());

    let Ok(image) = render_particles(WIDTH, HEIGHT, CLEAR, &list, &camera()) else {
        panic!("an empty pass must still produce a frame");
    };
    assert_eq!(covered(&image), 0, "something was drawn for no particles");
}

#[test]
fn a_particle_behind_the_camera_is_not_drawn() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let mut list = ParticleDrawList::with_capacity(16);
    list.push(
        ParticleBlend::Additive,
        ParticleInstance {
            position: [0.0, 0.0, 10.0],
            size: 2.0,
            color: [1.0, 1.0, 1.0, 1.0],
            rotation: 0.0,
        },
    );
    let Ok(image) = render_particles(WIDTH, HEIGHT, CLEAR, &list, &camera()) else {
        panic!("the particle pass must record and submit");
    };
    assert_eq!(
        covered(&image),
        0,
        "a particle behind the eye was drawn in front of it"
    );
}

#[test]
fn both_blend_modes_draw_in_one_frame() {
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let mut list = ParticleDrawList::with_capacity(16);
    list.push(
        ParticleBlend::Additive,
        ParticleInstance {
            position: [-2.0, 0.0, -10.0],
            size: 1.5,
            color: [1.0, 0.5, 0.1, 1.0],
            rotation: 0.0,
        },
    );
    list.push(
        ParticleBlend::Alpha,
        ParticleInstance {
            position: [2.0, 0.0, -10.0],
            size: 1.5,
            color: [0.3, 0.3, 0.35, 1.0],
            rotation: 0.0,
        },
    );

    let Ok(image) = render_particles(WIDTH, HEIGHT, CLEAR, &list, &camera()) else {
        panic!("the particle pass must record and submit");
    };

    // One blob left of centre and one right of it: both pipeline states ran in one
    // submission, which is what "two draws whatever the count" means.
    let mut left = 0;
    let mut right = 0;
    for (index, pixel) in image.pixels.chunks_exact(4).enumerate() {
        if pixel[0] <= 2 && pixel[1] <= 2 && pixel[2] <= 2 {
            continue;
        }
        if index % WIDTH as usize > WIDTH as usize / 2 {
            right += 1;
        } else {
            left += 1;
        }
    }
    assert!(
        left > 20,
        "the additive particle is missing ({left} pixels)"
    );
    assert!(right > 20, "the alpha particle is missing ({right} pixels)");
}

#[test]
fn a_full_frame_of_particles_still_submits() {
    // The stress case at the level this test can state it: the whole default budget in one
    // frame, through one upload allocation per blend mode and one draw each.
    if !adapter_available() {
        eprintln!("skipped: no D3D12 adapter");
        return;
    }
    let mut list = ParticleDrawList::default();
    let capacity = list.capacity();
    for index in 0..capacity {
        let offset = (index % 64) as f32 * 0.05 - 1.6;
        list.push(
            ParticleBlend::Additive,
            ParticleInstance {
                position: [offset, 0.0, -12.0],
                size: 0.05,
                color: [1.0, 0.5, 0.2, 0.5],
                rotation: index as f32 * 0.01,
            },
        );
    }
    assert_eq!(list.len(), capacity);
    assert_eq!(list.dropped(), 0);

    let Ok(image) = render_particles(WIDTH, HEIGHT, CLEAR, &list, &camera()) else {
        panic!("a full budget of particles must still submit");
    };
    assert!(covered(&image) > 0, "16,384 particles drew nothing");
}
