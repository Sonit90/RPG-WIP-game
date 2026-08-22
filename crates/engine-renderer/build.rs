//! Compiles every HLSL entry point to DXIL, and rasterizes every icon, at build time.
//!
//! Nothing is compiled or decoded at runtime. A shader error is a *build* error, carrying
//! `dxc`'s own diagnostic with file and line, and a malformed icon is a build error too;
//! startup does no compilation work at all.
//!
//! The generated `shaders.rs` in `OUT_DIR` declares one `&'static [u8]` per entry point;
//! `src/shader.rs` includes it. The generated `icons.rs` declares, per icon, one
//! `&'static [u8]` of straight-alpha RGBA texels for each size in `ICON_SIZES`;
//! `src/ui_atlas.rs` includes it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One compiled entry point.
struct ShaderEntry {
    /// File under `shaders/`.
    file: &'static str,
    /// HLSL entry-point function.
    entry: &'static str,
    /// `dxc` target profile.
    profile: &'static str,
    /// Name of the generated Rust constant.
    constant: &'static str,
}

/// Every shader the renderer contains.
///
/// Adding an entry point means adding a line here. There is no directory scan: a shader
/// that nothing references should fail to build as an unused file, not be compiled silently.
const SHADERS: &[ShaderEntry] = &[
    ShaderEntry {
        file: "triangle.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "TRIANGLE_VS",
    },
    ShaderEntry {
        file: "triangle.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "TRIANGLE_PS",
    },
    ShaderEntry {
        file: "mesh.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "MESH_VS",
    },
    ShaderEntry {
        file: "mesh.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "MESH_PS",
    },
    ShaderEntry {
        file: "mesh_instanced.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "MESH_INSTANCED_VS",
    },
    ShaderEntry {
        file: "mesh_instanced.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "MESH_INSTANCED_PS",
    },
    ShaderEntry {
        file: "particle.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "PARTICLE_VS",
    },
    ShaderEntry {
        file: "particle.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "PARTICLE_PS",
    },
    ShaderEntry {
        file: "mesh_skinned.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "MESH_SKINNED_VS",
    },
    ShaderEntry {
        file: "mesh_skinned.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "MESH_SKINNED_PS",
    },
    ShaderEntry {
        file: "ui.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "UI_VS",
    },
    ShaderEntry {
        file: "ui.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "UI_PS",
    },
    ShaderEntry {
        file: "shadow_depth.hlsl",
        entry: "VSInstanced",
        profile: "vs_6_0",
        constant: "SHADOW_DEPTH_INSTANCED_VS",
    },
    ShaderEntry {
        file: "shadow_depth.hlsl",
        entry: "VSSkinned",
        profile: "vs_6_0",
        constant: "SHADOW_DEPTH_SKINNED_VS",
    },
    ShaderEntry {
        file: "shadow_depth.hlsl",
        entry: "PSAlpha",
        profile: "ps_6_0",
        constant: "SHADOW_DEPTH_ALPHA_PS",
    },
    ShaderEntry {
        file: "instanced.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "INSTANCED_VS",
    },
    ShaderEntry {
        file: "instanced.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "INSTANCED_PS",
    },
];

/// The headers the shaders above include.
///
/// Not entry points, so they have no place in [`SHADERS`] — but an edit to one changes the
/// code of every shader that includes it, and Cargo only knows that if it is told.
const HEADERS: &[&str] = &["scene_frame.hlsli", "material.hlsli"];

/// One icon rasterized into the UI atlas.
///
/// Same rule as [`SHADERS`]: adding an icon means adding a line here. A file nobody named
/// is not silently compiled in, and a name in this table with no file is a build error.
const ICONS: &[IconEntry] = &[
    IconEntry {
        file: "firebolt.svg",
        constant: "FIREBOLT",
    },
    IconEntry {
        file: "meteor.svg",
        constant: "METEOR",
    },
    IconEntry {
        file: "flame_cone.svg",
        constant: "FLAME_CONE",
    },
    IconEntry {
        file: "flame_lance.svg",
        constant: "FLAME_LANCE",
    },
    IconEntry {
        file: "pyroblast.svg",
        constant: "PYROBLAST",
    },
    IconEntry {
        file: "firestorm.svg",
        constant: "FIRESTORM",
    },
    IconEntry {
        file: "detonate.svg",
        constant: "DETONATE",
    },
    IconEntry {
        file: "fire_mastery.svg",
        constant: "FIRE_MASTERY",
    },
    IconEntry {
        file: "flame_shield.svg",
        constant: "FLAME_SHIELD",
    },
    IconEntry {
        file: "blink.svg",
        constant: "BLINK",
    },
    IconEntry {
        file: "helm.svg",
        constant: "HELM",
    },
    IconEntry {
        file: "shoulders.svg",
        constant: "SHOULDERS",
    },
    IconEntry {
        file: "chestpiece.svg",
        constant: "CHESTPIECE",
    },
    IconEntry {
        file: "cloak.svg",
        constant: "CLOAK",
    },
    IconEntry {
        file: "gloves.svg",
        constant: "GLOVES",
    },
    IconEntry {
        file: "leggings.svg",
        constant: "LEGGINGS",
    },
    IconEntry {
        file: "boots.svg",
        constant: "BOOTS",
    },
    IconEntry {
        file: "bracers.svg",
        constant: "BRACERS",
    },
    IconEntry {
        file: "amulet.svg",
        constant: "AMULET",
    },
    IconEntry {
        file: "ring.svg",
        constant: "RING",
    },
    IconEntry {
        file: "trinket.svg",
        constant: "TRINKET",
    },
];

/// One icon: a file under `assets/icons/` and the constant its texels are bound to.
struct IconEntry {
    /// File under `assets/icons/`.
    file: &'static str,
    /// Name of the generated Rust constant.
    constant: &'static str,
}

/// The sizes every icon is rasterized at, largest first, in texels.
///
/// **Size variants, which are mipmaps done where they can be done correctly.** An icon is
/// drawn anywhere from 33 physical pixels (a bar slot at the smallest UI scale) to 132 (the
/// Ultimate at the largest), a four-fold range. Sampling a single 132-texel image down to 33
/// with a linear filter reads four texels out of every sixteen and aliases the thin detail
/// in a flame.
///
/// Hardware mipmaps are the usual answer and are *not* usable here: the icons share one
/// texture with the font, and a mip chain of an atlas averages across cell boundaries, so
/// level 1 would blend glyphs into icons. Instead each icon is rasterized from the vector
/// source at each size — sharper than downsampling would be — and the renderer picks the
/// cell nearest the size it is about to draw. That keeps every draw within 2x minification,
/// which is what a mip chain would have delivered, with no mip chain and no second texture.
///
/// The first entry must equal `MAX_ICON_PIXELS` in `engine-render-data`, and the second must
/// be half of it. `src/ui_atlas.rs` asserts both, since a build script cannot read a
/// constant out of the crate it is building.
const ICON_SIZES: [u32; 2] = [132, 66];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=shaders");
    println!("cargo:rerun-if-changed=assets/icons");
    println!("cargo:rerun-if-env-changed=DXC_PATH");

    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env("OUT_DIR"));
    let shader_dir = manifest.join("shaders");
    let dxc = find_dxc();

    // Included headers are not entry points and so are not in the table above; without
    // these lines an edit to the shared lighting or material header would not rebuild any of
    // the shaders that include it, and the binary would keep the old code.
    for header in HEADERS {
        println!(
            "cargo:rerun-if-changed={}",
            shader_dir.join(header).display()
        );
    }

    let mut generated = String::from(
        "// Generated by build.rs. Do not edit.\n\
         // One DXIL blob per entry point, compiled by dxc and embedded in the binary.\n",
    );

    for shader in SHADERS {
        let source = shader_dir.join(shader.file);
        println!("cargo:rerun-if-changed={}", source.display());

        let stem = shader.file.trim_end_matches(".hlsl");
        let output = out_dir.join(format!("{stem}.{}.dxil", shader.entry));
        compile(&dxc, &source, shader, &output);

        generated.push_str(&format!(
            "\n/// `{}` `{}` from `shaders/{}`, compiled to DXIL at build time.\n\
             pub const {}: &[u8] = include_bytes!(r\"{}\");\n",
            shader.entry,
            shader.profile,
            shader.file,
            shader.constant,
            output.display(),
        ));
    }

    std::fs::write(out_dir.join("shaders.rs"), generated)
        .unwrap_or_else(|error| panic!("failed to write the generated shader module: {error}"));

    rasterize_icons(&manifest.join("assets").join("icons"), &out_dir);
}

/// Rasterizes every icon to straight-alpha RGBA texels and generates `icons.rs`.
///
/// The texels are written to `OUT_DIR` as raw files and `include_bytes!`d, rather than
/// spelled out as a Rust array literal: the largest variant alone is 132x132x4, or 68 KiB
/// per icon, and that much source text is something `rustc` would have to parse every build.
fn rasterize_icons(icon_dir: &Path, out_dir: &Path) {
    let mut generated = String::from(
        "// Generated by build.rs. Do not edit.\n\
         // Every icon rasterized from assets/icons at build time, one blob per size.\n\
         // Straight-alpha RGBA8, size x size texels, row-major.\n",
    );
    generated.push_str(&format!(
        "\n/// The sizes every icon is rasterized at, largest first, in texels.\n\
         pub const SIZES: [usize; {}] = {:?};\n",
        ICON_SIZES.len(),
        ICON_SIZES.map(|size| size as usize),
    ));

    for icon in ICONS {
        let source = icon_dir.join(icon.file);
        println!("cargo:rerun-if-changed={}", source.display());

        let mut variants = Vec::new();
        for size in ICON_SIZES {
            let texels = rasterize(&source, size);
            let output = out_dir.join(format!("{}_{size}.rgba", icon.constant));
            std::fs::write(&output, &texels)
                .unwrap_or_else(|error| panic!("failed to write {}: {error}", output.display()));
            variants.push(format!("include_bytes!(r\"{}\")", output.display()));
        }

        // One slice per icon, indexed the same way as `SIZES`, so the atlas walks the two
        // in step rather than naming a size twice.
        generated.push_str(&format!(
            "\n/// `assets/icons/{}` at each of `SIZES`, rasterized at build time.\n\
             pub const {}: [&[u8]; {}] = [{}];\n",
            icon.file,
            icon.constant,
            ICON_SIZES.len(),
            variants.join(", "),
        ));
    }

    std::fs::write(out_dir.join("icons.rs"), generated)
        .unwrap_or_else(|error| panic!("failed to write the generated icon module: {error}"));
}

/// Rasterizes one SVG into a `size` square of straight-alpha RGBA texels.
///
/// The icon is scaled to fit by its own view box, so an icon authored at any size lands
/// correctly without the table having to record its dimensions. Rendering the vector at each
/// size, rather than downsampling the largest, is what makes the small variant sharp.
fn rasterize(source: &Path, size: u32) -> Vec<u8> {
    use resvg::tiny_skia;
    use resvg::usvg;

    let data = std::fs::read(source)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source.display()));

    // The default options carry no font database: none of the icons contain a <text>
    // element, and `usvg` drops any it cannot shape. Checking here means an icon that grows
    // one fails the build instead of rendering with a hole in it.
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default())
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", source.display()));

    let view_box = tree.size();
    assert!(
        view_box.width() > 0.0 && view_box.height() > 0.0,
        "{} has an empty view box",
        source.display()
    );

    // Uniform scale with the icon centred: a non-square icon keeps its proportions rather
    // than being stretched to the cell, which is what a stretch would do to a flame.
    let scale = (size as f32 / view_box.width()).min(size as f32 / view_box.height());
    let transform = tiny_skia::Transform::from_translate(
        (size as f32 - view_box.width() * scale) * 0.5,
        (size as f32 - view_box.height() * scale) * 0.5,
    )
    .pre_scale(scale, scale);

    let mut pixmap = tiny_skia::Pixmap::new(size, size)
        .unwrap_or_else(|| panic!("failed to allocate a {size}x{size} pixmap"));
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // tiny-skia works premultiplied; the UI pass blends straight alpha. Undoing the
    // multiply here rather than in the shader keeps the one blend state the whole UI shares.
    let mut texels = Vec::with_capacity((size * size) as usize * 4);
    for texel in pixmap.pixels() {
        let color = texel.demultiply();
        texels.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }

    assert!(
        texels.iter().skip(3).step_by(4).any(|&alpha| alpha != 0),
        "{} rasterized to nothing but transparent texels at {size}",
        source.display()
    );
    texels
}

/// Reads a build-script environment variable that Cargo always sets.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("cargo did not set {name}"))
}

/// Runs `dxc` for one entry point, failing the build with its diagnostic.
fn compile(dxc: &Path, source: &Path, shader: &ShaderEntry, output: &Path) {
    // One set of flags for every build profile. Optimized-only means a shader bug
    // reproduces identically in debug and release, and the bytecode is the same bytes the
    // shipped binary runs. Warnings are errors because an HLSL warning is nearly always a
    // type or semantic mistake that renders as garbage rather than as a failure.
    let result = Command::new(dxc)
        .arg("-T")
        .arg(shader.profile)
        .arg("-E")
        .arg(shader.entry)
        .arg("-O3")
        .arg("-WX")
        .arg("-Fo")
        .arg(output)
        .arg(source)
        .output();

    let output_result = match result {
        Ok(output) => output,
        Err(error) => panic!("failed to run {}: {error}", dxc.display()),
    };

    if !output_result.status.success() {
        let messages = String::from_utf8_lossy(&output_result.stderr);
        let messages = if messages.trim().is_empty() {
            String::from_utf8_lossy(&output_result.stdout).into_owned()
        } else {
            messages.into_owned()
        };
        panic!(
            "shader compilation failed: {} {} in {}\n{}",
            shader.entry,
            shader.profile,
            source.display(),
            messages.trim()
        );
    }
}

/// Locates `dxc.exe`.
///
/// Order: an explicit `DXC_PATH`, then `PATH`, then the Windows SDK. The SDK ships it, so
/// the third case is the one that works on a machine nobody has configured.
fn find_dxc() -> PathBuf {
    if let Ok(explicit) = std::env::var("DXC_PATH") {
        let path = PathBuf::from(explicit);
        assert!(
            path.is_file(),
            "DXC_PATH does not name a file: {}",
            path.display()
        );
        return path;
    }

    if runs("dxc") {
        return PathBuf::from("dxc");
    }

    if let Some(path) = newest_sdk_dxc() {
        return path;
    }

    panic!(
        "dxc.exe was not found.\n\
         It ships with the Windows 10/11 SDK at \
         bin\\<version>\\x64\\dxc.exe (SDK 10.0.17763 or newer).\n\
         Install the SDK, put dxc.exe on PATH, or set DXC_PATH to its full path."
    );
}

/// Whether a command exists and runs.
fn runs(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Searches the Windows SDK for the newest `dxc.exe` matching the host architecture.
fn newest_sdk_dxc() -> Option<PathBuf> {
    let arch = match std::env::var("HOST").unwrap_or_default() {
        host if host.starts_with("aarch64") => "arm64",
        host if host.starts_with("i686") => "x86",
        _ => "x64",
    };

    let roots = [
        std::env::var("ProgramFiles(x86)").ok(),
        std::env::var("ProgramFiles").ok(),
    ];

    let mut best: Option<(Vec<u32>, PathBuf)> = None;

    for root in roots.into_iter().flatten() {
        let bin = Path::new(&root).join("Windows Kits").join("10").join("bin");
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };

        for entry in entries.flatten() {
            let candidate = entry.path().join(arch).join("dxc.exe");
            if !candidate.is_file() {
                continue;
            }
            let version: Vec<u32> = entry
                .file_name()
                .to_string_lossy()
                .split('.')
                .filter_map(|part| part.parse().ok())
                .collect();
            // A directory whose name is not a version sorts below every real one rather
            // than being discarded: a working dxc is better than none.
            if best.as_ref().is_none_or(|(seen, _)| *seen < version) {
                best = Some((version, candidate));
            }
        }
    }

    best.map(|(_, path)| path)
}
