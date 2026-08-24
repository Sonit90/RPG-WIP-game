//! Texture deduplication and mip chains, against a real device.
//!
//! `mipmap.rs` unit-tests the filters and `texture_id.rs` unit-tests the hash; what needs a
//! driver is that the renderer really uploads one resource for two identical images, really
//! keeps two different ones apart, and really allocates and fills a chain that the SRV can
//! see. Those are all statements about GPU objects, and none of them is visible in a pixel.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use engine_platform::{Window, WindowConfig};
use engine_renderer::device::RenderDevice;
use engine_renderer::mipmap::{Compression, TextureKind, level_count};
use engine_renderer::{Renderer, RendererConfig, RendererError, TextureHandle, TextureImage};

static GPU: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(PoisonError::into_inner)
}

fn gpu_available() -> bool {
    match RenderDevice::new(false, false) {
        Ok(_) => true,
        Err(RendererError::NoSuitableAdapter { examined }) => {
            eprintln!("SKIP: no D3D12 adapter on this machine ({examined} examined)");
            false
        }
        Err(error) => panic!("unexpected D3D12 initialization failure: {error}"),
    }
}

fn renderer_for(title: &str) -> (Window, Renderer) {
    let window = Window::new(&WindowConfig::new(title, 320, 240).with_visible(false))
        .expect("hidden window creation must succeed");
    let config = RendererConfig::new(320, 240)
        .with_debug_layer(true)
        .with_break_on_validation_error(false);
    let renderer =
        Renderer::new(window.raw_handle(), &config).expect("renderer creation must succeed");
    (window, renderer)
}

/// An image whose texels are a function of the position, so two images built the same way
/// are byte-identical and two built differently are not.
fn image(width: u32, height: u32, seed: u8) -> Vec<u8> {
    let mut texels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            texels.extend_from_slice(&[
                (x as u8).wrapping_mul(7).wrapping_add(seed),
                (y as u8).wrapping_mul(13).wrapping_add(seed),
                seed,
                255,
            ]);
        }
    }
    texels
}

fn upload(
    renderer: &mut Renderer,
    texels: &[u8],
    width: u32,
    height: u32,
    kind: TextureKind,
) -> TextureHandle {
    renderer
        .create_texture(&TextureImage {
            width,
            height,
            kind,
            texels,
        })
        .expect("the upload must succeed")
}

#[test]
fn two_copies_of_one_image_become_one_gpu_texture() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("texture dedup");

    let before = renderer.texture_count();
    // Two separate allocations holding the same bytes — which is what two `.glb` files
    // exported from the same source produce, and the case a path comparison cannot see.
    let first = image(32, 32, 3);
    let second = first.clone();
    assert_ne!(
        first.as_ptr(),
        second.as_ptr(),
        "the test needs two distinct buffers to prove anything"
    );

    let a = upload(&mut renderer, &first, 32, 32, TextureKind::Color);
    let b = upload(&mut renderer, &second, 32, 32, TextureKind::Color);

    assert_eq!(a, b, "identical content must resolve to one texture");
    assert_eq!(
        renderer.texture_count() - before,
        1,
        "and must cost exactly one upload"
    );
    assert_eq!(
        renderer.texture_requests(),
        // Four fallback requests at startup plus these two.
        6,
        "both requests are counted even though one was answered from the table"
    );
}

#[test]
fn images_that_differ_in_any_way_stay_separate() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("texture identity");

    let before = renderer.texture_count();
    let base = image(16, 16, 1);

    // One texel different: the hash has to see it, or two maps that differ in a small
    // detail — a decal, a logo — would collapse into one.
    let mut altered = base.clone();
    altered[4 * (8 * 16 + 8)] ^= 0xFF;

    // The same bytes at a different shape, and the same bytes in the other colour space.
    // Both are different GPU textures however the texels compare.
    let strip = image(16, 16, 1);

    let a = upload(&mut renderer, &base, 16, 16, TextureKind::Color);
    let b = upload(&mut renderer, &altered, 16, 16, TextureKind::Color);
    let c = upload(&mut renderer, &strip, 8, 32, TextureKind::Color);
    let d = upload(&mut renderer, &base, 16, 16, TextureKind::Data);

    assert_ne!(a, b, "one changed texel is a different texture");
    assert_ne!(
        a, c,
        "the same bytes at another shape are a different picture"
    );
    assert_ne!(a, d, "an sRGB view and a linear one are different textures");
    assert_eq!(
        renderer.texture_count() - before,
        4,
        "four genuinely different images must cost four uploads"
    );
}

#[test]
fn a_normal_map_and_a_data_map_of_the_same_bytes_are_filtered_apart() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("texture kinds");

    // Same bytes, same colour space, different *meaning*: one is filtered as a direction and
    // renormalized, the other averaged where it lies. Sharing one resource between them
    // would give whichever asked second the other's mips.
    let texels = image(8, 8, 5);
    let normal = upload(&mut renderer, &texels, 8, 8, TextureKind::Normal);
    let data = upload(&mut renderer, &texels, 8, 8, TextureKind::Data);
    assert_ne!(
        normal, data,
        "a normal map's chain is not a data map's chain, so they cannot share a resource"
    );
}

#[test]
fn an_uploaded_texture_carries_the_whole_mip_chain() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("mip chains");

    // A power of two, a non-power-of-two, a long strip and a single texel: the four shapes
    // the level count is easy to get wrong on.
    for (width, height) in [(64_u32, 64_u32), (100, 60), (128, 1), (1, 1)] {
        let texels = image(width, height, (width % 251) as u8);
        let handle = upload(&mut renderer, &texels, width, height, TextureKind::Color);
        let levels = renderer
            .texture_mip_levels(handle)
            .expect("the handle names a texture");
        assert_eq!(
            levels,
            level_count(width, height),
            "{width}x{height} must allocate the whole chain"
        );
    }

    // Every level of every chain was filled by a copy the driver accepted: an unwritten
    // subresource is a debug-layer error at upload, and the renderer is running with the
    // debug layer on.
}

#[test]
fn a_model_uploads_one_texture_per_distinct_image_not_per_reference() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("model dedup");

    // The shipped content, not a constructed case: the environment library's materials share
    // images with each other by design, and that sharing is what deduplication is for.
    let before = renderer.texture_count();
    let mut references = 0;
    for (_asset, model) in engine_assets::procedural::environment::environment_library() {
        references += model.textures.len();
        renderer.create_model(&model).expect("the model uploads");
    }
    let uploaded = renderer.texture_count() - before;

    assert!(uploaded > 0, "the library has textures");
    assert!(
        uploaded < references,
        "{references} reference(s) must resolve to fewer uploads, got {uploaded}"
    );
}

#[test]
fn compression_is_chosen_by_what_a_texture_is_and_how_big_it_is() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("texture compression");

    // Big enough to be worth compressing. Opaque colour goes to BC1 at half a byte a texel,
    // a normal map to BC5 at one, and packed ORM data to neither — its three channels are
    // independent numbers and BC1's shared endpoints would tie them together.
    let opaque = image(64, 64, 11);
    let normal = image(64, 64, 12);
    let data = image(64, 64, 13);

    let colour_handle = upload(&mut renderer, &opaque, 64, 64, TextureKind::Color);
    let normal_handle = upload(&mut renderer, &normal, 64, 64, TextureKind::Normal);
    let data_handle = upload(&mut renderer, &data, 64, 64, TextureKind::Data);

    assert_eq!(
        renderer.texture_compression(colour_handle),
        Some(Compression::Bc1)
    );
    assert_eq!(
        renderer.texture_compression(normal_handle),
        Some(Compression::Bc5)
    );
    assert_eq!(
        renderer.texture_compression(data_handle),
        Some(Compression::None),
        "an ORM map must stay uncompressed: BC1 couples channels that mean different things"
    );

    // Colour with meaningful alpha stays uncompressed too. BC1's alpha is one bit per block,
    // and a cutout material's alpha decides whether a fragment exists at all.
    let mut cutout = image(64, 64, 14);
    cutout[3] = 0;
    let cutout_handle = upload(&mut renderer, &cutout, 64, 64, TextureKind::Color);
    assert_eq!(
        renderer.texture_compression(cutout_handle),
        Some(Compression::None)
    );

    // And a small texture is left alone whatever it is: one bad 4x4 block is a sixteenth of a
    // 16x16 image, and the 1x1 fallbacks must stay exact because the whole material ABI rests
    // on them.
    let small = image(32, 32, 15);
    let small_handle = upload(&mut renderer, &small, 32, 32, TextureKind::Color);
    assert_eq!(
        renderer.texture_compression(small_handle),
        Some(Compression::None)
    );
    for fallback in renderer.fallback_textures().in_descriptor_order() {
        assert_eq!(
            renderer.texture_compression(fallback),
            Some(Compression::None),
            "the shared fallbacks must stay exact"
        );
    }
}

#[test]
fn a_compressed_texture_still_carries_its_whole_mip_chain() {
    let _gpu = gpu_lock();
    if !gpu_available() {
        return;
    }
    let (_window, mut renderer) = renderer_for("compressed mips");

    // The chain runs past the block size — 4x4, 2x2, 1x1 are all one block with padding — and
    // getting that wrong is a debug-layer error at upload rather than a wrong picture. The
    // renderer runs with the debug layer on here, so reaching the assertion is half the test.
    for kind in [TextureKind::Color, TextureKind::Normal] {
        let texels = image(128, 64, 21);
        let handle = upload(&mut renderer, &texels, 128, 64, kind);
        assert_ne!(
            renderer.texture_compression(handle),
            Some(Compression::None),
            "{kind:?} at 128x64 should be compressed"
        );
        assert_eq!(
            renderer.texture_mip_levels(handle),
            Some(level_count(128, 64)),
            "{kind:?} must keep the whole chain after compression"
        );
    }
}
