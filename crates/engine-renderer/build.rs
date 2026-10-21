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

include!("build_fonts.rs");

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
    /// Preprocessor symbols, passed as `-D NAME=1`.
    ///
    /// Two entries that differ only here are two different shaders from one source file, so
    /// the defines are part of the output filename as well — without that the second
    /// permutation would overwrite the first's DXIL and every variant would be the last one
    /// compiled.
    defines: &'static [&'static str],
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
        defines: &[],
    },
    ShaderEntry {
        file: "triangle.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "TRIANGLE_PS",
        defines: &[],
    },
    ShaderEntry {
        file: "mesh.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "MESH_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "mesh_instanced.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "MESH_INSTANCED_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "particle.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "PARTICLE_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "particle.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "PARTICLE_PS",
        defines: &[],
    },
    ShaderEntry {
        file: "mesh_skinned.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "MESH_SKINNED_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "ui.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "UI_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "ui.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "UI_PS",
        defines: &[],
    },
    ShaderEntry {
        file: "shadow_depth.hlsl",
        entry: "VSInstanced",
        profile: "vs_6_0",
        constant: "SHADOW_DEPTH_INSTANCED_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "shadow_depth.hlsl",
        entry: "VSSkinned",
        profile: "vs_6_0",
        constant: "SHADOW_DEPTH_SKINNED_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "shadow_depth.hlsl",
        entry: "PSAlpha",
        profile: "ps_6_0",
        constant: "SHADOW_DEPTH_ALPHA_PS",
        defines: &[],
    },
    ShaderEntry {
        file: "contact.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "CONTACT_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "contact.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "CONTACT_PS",
        defines: &[],
    },
    ShaderEntry {
        file: "instanced.hlsl",
        entry: "VSMain",
        profile: "vs_6_0",
        constant: "INSTANCED_VS",
        defines: &[],
    },
    ShaderEntry {
        file: "instanced.hlsl",
        entry: "PSMain",
        profile: "ps_6_0",
        constant: "INSTANCED_PS",
        defines: &[],
    },
];

/// The mesh pixel shader's feature permutations.
///
/// Three independent compile-time features — `MATERIAL_PBR`, `MATERIAL_NORMAL_MAP` and
/// `ENABLE_SHADOWS` — across the three mesh pipelines, so the renderer can bind a pipeline
/// state that contains only the work a given material and the current shadow setting
/// actually need. A feature that is off is not branched around at runtime: it is not in the
/// bytecode.
///
/// Eight permutations per pipeline, twenty-four blobs. They are cheap to compile and cheap
/// to embed (the largest is 45 KB), and enumerating them here rather than deriving the set
/// from what the game happens to use keeps "which variants exist" answerable by reading one
/// table. Which of them become *pipeline states* is a runtime decision: `renderer.rs`
/// creates them on first use.
const MESH_PERMUTATIONS: &[MeshPermutation] = &[
    MeshPermutation {
        file: "mesh.hlsl",
        prefix: "MESH_PS",
    },
    MeshPermutation {
        file: "mesh_instanced.hlsl",
        prefix: "MESH_INSTANCED_PS",
    },
    MeshPermutation {
        file: "mesh_skinned.hlsl",
        prefix: "MESH_SKINNED_PS",
    },
];

/// One mesh pixel shader, compiled once per feature combination.
struct MeshPermutation {
    /// File under `shaders/`.
    file: &'static str,
    /// Constant-name prefix. The suffix names the features that are on.
    prefix: &'static str,
}

/// The three feature axes, in the order their suffixes are appended.
///
/// `(define, suffix)`. A permutation's constant is the prefix plus one `_SUFFIX` per feature
/// that is on, so the all-off variant keeps the bare prefix — `MESH_PS` still means what it
/// has always meant, the shader with nothing switched on.
const MESH_FEATURES: [(&str, &str); 3] = [
    ("MATERIAL_PBR", "PBR"),
    ("MATERIAL_NORMAL_MAP", "NORMAL_MAP"),
    ("ENABLE_SHADOWS", "SHADOWS"),
];

/// The headers the shaders above include.
///
/// Not entry points, so they have no place in [`SHADERS`] — but an edit to one changes the
/// code of every shader that includes it, and Cargo only knows that if it is told.
const HEADERS: &[&str] = &["scene_frame.hlsli", "material.hlsli", "pbr.hlsli"];

/// One icon rasterized into the UI atlas.
///
/// Same rule as [`SHADERS`]: adding an icon means adding a line here. A file nobody named
/// is not silently compiled in, and a name in this table with no file is a build error.
const ICONS: &[IconEntry] = &[
    IconEntry {
        file: "firemage/firebolt.svg",
        constant: "FIREBOLT",
    },
    IconEntry {
        file: "firemage/meteor.svg",
        constant: "METEOR",
    },
    IconEntry {
        file: "firemage/flame_cone.svg",
        constant: "FLAME_CONE",
    },
    IconEntry {
        file: "firemage/flame_lance.svg",
        constant: "FLAME_LANCE",
    },
    IconEntry {
        file: "firemage/pyroblast.svg",
        constant: "PYROBLAST",
    },
    IconEntry {
        file: "firemage/firestorm.svg",
        constant: "FIRESTORM",
    },
    IconEntry {
        file: "firemage/detonate.svg",
        constant: "DETONATE",
    },
    IconEntry {
        file: "firemage/fire_mastery.svg",
        constant: "FIRE_MASTERY",
    },
    IconEntry {
        file: "firemage/flame_shield.svg",
        constant: "FLAME_SHIELD",
    },
    IconEntry {
        file: "firemage/blink.svg",
        constant: "BLINK",
    },
    IconEntry {
        file: "firemage/fire_beam.svg",
        constant: "FIRE_BEAM",
    },
    IconEntry {
        file: "warrior/recuperate.svg",
        constant: "RECUPERATE",
    },
    IconEntry {
        file: "warrior/taunt.svg",
        constant: "TAUNT",
    },
    IconEntry {
        file: "warrior/war_stomp.svg",
        constant: "WAR_STOMP",
    },
    IconEntry {
        file: "warrior/colossus_strike.svg",
        constant: "COLOSSUS_STRIKE",
    },
    IconEntry {
        file: "warrior/shield_charge.svg",
        constant: "SHIELD_CHARGE",
    },
    IconEntry {
        file: "warrior/shield_bash.svg",
        constant: "SHIELD_BASH",
    },
    IconEntry {
        file: "warrior/shield_block.svg",
        constant: "SHIELD_BLOCK",
    },
    IconEntry {
        file: "warrior/puncture_blood.svg",
        constant: "PUNCTURE_BLOOD",
    },
    IconEntry {
        file: "warrior/adrenaline.svg",
        constant: "ADRENALINE",
    },
    IconEntry {
        file: "warrior/battle_trance.svg",
        constant: "BATTLE_TRANCE",
    },
    IconEntry {
        file: "warrior/spell_reflect.svg",
        constant: "SPELL_REFLECT",
    },
    IconEntry {
        file: "warrior/last_stand.svg",
        constant: "LAST_STAND",
    },
    IconEntry {
        file: "warrior/valor.svg",
        constant: "VALOR",
    },
    IconEntry {
        file: "warrior/battle_fervor.svg",
        constant: "BATTLE_FERVOR",
    },
    // The Druid's thirteen.
    IconEntry {
        file: "druid/moonfire.svg",
        constant: "MOONFIRE",
    },
    IconEntry {
        file: "druid/sunfire.svg",
        constant: "SUNFIRE",
    },
    IconEntry {
        file: "druid/wrath.svg",
        constant: "WRATH",
    },
    IconEntry {
        file: "druid/rejuvenation.svg",
        constant: "REJUVENATION",
    },
    IconEntry {
        file: "druid/regrowth.svg",
        constant: "REGROWTH",
    },
    IconEntry {
        file: "druid/lifebloom.svg",
        constant: "LIFEBLOOM",
    },
    IconEntry {
        file: "druid/wild_growth.svg",
        constant: "WILD_GROWTH",
    },
    IconEntry {
        file: "druid/swiftmend.svg",
        constant: "SWIFTMEND",
    },
    IconEntry {
        file: "druid/nourish.svg",
        constant: "NOURISH",
    },
    IconEntry {
        file: "druid/natures_cleanse.svg",
        constant: "NATURES_CLEANSE",
    },
    IconEntry {
        file: "druid/wild_rush.svg",
        constant: "WILD_RUSH",
    },
    IconEntry {
        file: "druid/astral_power.svg",
        constant: "ASTRAL_POWER",
    },
    IconEntry {
        file: "druid/tranquility.svg",
        constant: "TRANQUILITY",
    },
    IconEntry {
        file: "equipment/helm.svg",
        constant: "HELM",
    },
    IconEntry {
        file: "equipment/shoulders.svg",
        constant: "SHOULDERS",
    },
    IconEntry {
        file: "equipment/chestpiece.svg",
        constant: "CHESTPIECE",
    },
    IconEntry {
        file: "equipment/cloak.svg",
        constant: "CLOAK",
    },
    IconEntry {
        file: "equipment/gloves.svg",
        constant: "GLOVES",
    },
    IconEntry {
        file: "equipment/leggings.svg",
        constant: "LEGGINGS",
    },
    IconEntry {
        file: "equipment/boots.svg",
        constant: "BOOTS",
    },
    IconEntry {
        file: "equipment/bracers.svg",
        constant: "BRACERS",
    },
    IconEntry {
        file: "equipment/amulet.svg",
        constant: "AMULET",
    },
    IconEntry {
        file: "equipment/ring.svg",
        constant: "RING",
    },
    IconEntry {
        file: "equipment/trinket.svg",
        constant: "TRINKET",
    },
    IconEntry {
        file: "equipment/trinket_ward.svg",
        constant: "TRINKET_WARD",
    },
    IconEntry {
        file: "equipment/trinket_dispel.svg",
        constant: "TRINKET_DISPEL",
    },
    IconEntry {
        file: "equipment/trinket_haste.svg",
        constant: "TRINKET_HASTE",
    },
    IconEntry {
        file: "equipment/trinket_absorb.svg",
        constant: "TRINKET_ABSORB",
    },
    IconEntry {
        file: "map/map_pack.svg",
        constant: "MAP_PACK",
    },
    IconEntry {
        file: "map/map_boss.svg",
        constant: "MAP_BOSS",
    },
    IconEntry {
        file: "map/map_player.svg",
        constant: "MAP_PLAYER",
    },
    IconEntry {
        file: "map/map_compass.svg",
        constant: "MAP_COMPASS",
    },
    IconEntry {
        file: "map/monster.svg",
        constant: "MONSTER",
    },
    IconEntry {
        file: "map/ability_physical.svg",
        constant: "ABILITY_PHYSICAL",
    },
    IconEntry {
        file: "map/ability_fire.svg",
        constant: "ABILITY_FIRE",
    },
    IconEntry {
        file: "map/ability_arcane.svg",
        constant: "ABILITY_ARCANE",
    },
    IconEntry {
        file: "map/ability_nature.svg",
        constant: "ABILITY_NATURE",
    },
    IconEntry {
        file: "map/ability_heal.svg",
        constant: "ABILITY_HEAL",
    },
    IconEntry {
        file: "map/crest_elite.svg",
        constant: "CREST_ELITE",
    },
    IconEntry {
        file: "map/emblem_crypt.svg",
        constant: "EMBLEM_CRYPT",
    },
    IconEntry {
        file: "map/control_zoom_in.svg",
        constant: "CONTROL_ZOOM_IN",
    },
    IconEntry {
        file: "map/control_zoom_out.svg",
        constant: "CONTROL_ZOOM_OUT",
    },
    IconEntry {
        file: "map/control_fit.svg",
        constant: "CONTROL_FIT",
    },
    IconEntry {
        file: "map/control_recenter.svg",
        constant: "CONTROL_RECENTER",
    },
    IconEntry {
        file: "map/control_close.svg",
        constant: "CONTROL_CLOSE",
    },
    IconEntry {
        file: "map/mouse_drag.svg",
        constant: "MOUSE_DRAG",
    },
    IconEntry {
        file: "map/mouse_wheel.svg",
        constant: "MOUSE_WHEEL",
    },
    IconEntry {
        file: "map/mouse_click.svg",
        constant: "MOUSE_CLICK",
    },
    IconEntry {
        file: "map/loot_chest.svg",
        constant: "LOOT_CHEST",
    },
    IconEntry {
        file: "map/icon_lock.svg",
        constant: "ICON_LOCK",
    },
    IconEntry {
        file: "map/icon_check.svg",
        constant: "ICON_CHECK",
    },
    IconEntry {
        file: "map/tag_control.svg",
        constant: "TAG_CONTROL",
    },
    IconEntry {
        file: "map/tag_interrupt.svg",
        constant: "TAG_INTERRUPT",
    },
    IconEntry {
        file: "map/tag_area.svg",
        constant: "TAG_AREA",
    },
    IconEntry {
        file: "map/control_chevron.svg",
        constant: "CONTROL_CHEVRON",
    },
    // Edit HUD's controls.
    IconEntry {
        file: "ui/arrow.svg",
        constant: "UI_ARROW",
    },
    IconEntry {
        file: "ui/plus.svg",
        constant: "UI_PLUS",
    },
    IconEntry {
        file: "ui/minus.svg",
        constant: "UI_MINUS",
    },
    IconEntry {
        file: "ui/eye.svg",
        constant: "UI_EYE",
    },
    IconEntry {
        file: "ui/eye_off.svg",
        constant: "UI_EYE_OFF",
    },
    IconEntry {
        file: "ui/grid.svg",
        constant: "UI_GRID",
    },
    IconEntry {
        file: "ui/snap.svg",
        constant: "UI_SNAP",
    },
    IconEntry {
        file: "ui/reset.svg",
        constant: "UI_RESET",
    },
    IconEntry {
        file: "ui/move.svg",
        constant: "UI_MOVE",
    },
    IconEntry {
        file: "ui/key.svg",
        constant: "UI_KEY",
    },
    IconEntry {
        file: "ui/dice.svg",
        constant: "UI_DICE",
    },
    // The buffs and debuffs that have no ability icon of their own to borrow: shown under
    // nameplates, beside the player's frame and on the party's.
    IconEntry {
        file: "aura/burning.svg",
        constant: "AURA_BURNING",
    },
    IconEntry {
        file: "aura/bleeding.svg",
        constant: "AURA_BLEEDING",
    },
    IconEntry {
        file: "aura/recovering.svg",
        constant: "AURA_RECOVERING",
    },
    IconEntry {
        file: "aura/sundered.svg",
        constant: "AURA_SUNDERED",
    },
    IconEntry {
        file: "aura/astral_sunfire.svg",
        constant: "AURA_ASTRAL_SUNFIRE",
    },
    IconEntry {
        file: "aura/natures_exposure.svg",
        constant: "AURA_NATURES_EXPOSURE",
    },
    IconEntry {
        file: "aura/astral_wrath.svg",
        constant: "AURA_ASTRAL_WRATH",
    },
    IconEntry {
        file: "aura/poisoned.svg",
        constant: "AURA_POISONED",
    },
    IconEntry {
        file: "aura/respawn_protection.svg",
        constant: "AURA_RESPAWN_PROTECTION",
    },
    IconEntry {
        file: "aura/rended.svg",
        constant: "AURA_RENDED",
    },
    IconEntry {
        file: "aura/crippled.svg",
        constant: "AURA_CRIPPLED",
    },
    IconEntry {
        file: "aura/frenzy.svg",
        constant: "AURA_FRENZY",
    },
    IconEntry {
        file: "aura/stunned.svg",
        constant: "AURA_STUNNED",
    },
    IconEntry {
        file: "aura/silenced.svg",
        constant: "AURA_SILENCED",
    },
    IconEntry {
        file: "aura/inferno.svg",
        constant: "AURA_INFERNO",
    },
    IconEntry {
        file: "aura/blood_ward.svg",
        constant: "AURA_BLOOD_WARD",
    },
    IconEntry {
        file: "aura/scorched.svg",
        constant: "AURA_SCORCHED",
    },
    IconEntry {
        file: "aura/fortitude.svg",
        constant: "AURA_FORTITUDE",
    },
    IconEntry {
        file: "aura/battle_shout.svg",
        constant: "AURA_BATTLE_SHOUT",
    },
    IconEntry {
        file: "aura/arcane_intellect.svg",
        constant: "AURA_ARCANE_INTELLECT",
    },
    IconEntry {
        file: "aura/bloodlust.svg",
        constant: "AURA_BLOODLUST",
    },
    IconEntry {
        file: "aura/regeneration.svg",
        constant: "AURA_REGENERATION",
    },
    IconEntry {
        file: "aura/curse_of_weakness.svg",
        constant: "AURA_CURSE_OF_WEAKNESS",
    },
    IconEntry {
        file: "aura/withering_plague.svg",
        constant: "AURA_WITHERING_PLAGUE",
    },
    IconEntry {
        file: "aura/magic_vulnerability.svg",
        constant: "AURA_MAGIC_VULNERABILITY",
    },
    IconEntry {
        file: "aura/plagued.svg",
        constant: "AURA_PLAGUED",
    },
    IconEntry {
        file: "modifier/rotating_beams.svg",
        constant: "MODIFIER_ROTATING_BEAMS",
    },
    IconEntry {
        file: "modifier/plague_explosion.svg",
        constant: "MODIFIER_PLAGUE_EXPLOSION",
    },
    IconEntry {
        file: "gem/gem.svg",
        constant: "GEM",
    },
    IconEntry {
        file: "gem/gem_ruby.svg",
        constant: "GEM_RUBY",
    },
    IconEntry {
        file: "gem/gem_amber.svg",
        constant: "GEM_AMBER",
    },
    IconEntry {
        file: "gem/gem_topaz.svg",
        constant: "GEM_TOPAZ",
    },
    IconEntry {
        file: "gem/gem_emerald.svg",
        constant: "GEM_EMERALD",
    },
    IconEntry {
        file: "gem/gem_amethyst.svg",
        constant: "GEM_AMETHYST",
    },
    IconEntry {
        file: "gem/gem_onyx.svg",
        constant: "GEM_ONYX",
    },
    IconEntry {
        file: "gem/gem_opal.svg",
        constant: "GEM_OPAL",
    },
    IconEntry {
        file: "gem/book_talent.svg",
        constant: "BOOK_TALENT",
    },
    IconEntry {
        file: "gem/book_enhancement.svg",
        constant: "BOOK_ENHANCEMENT",
    },
    IconEntry {
        file: "aura/shielded.svg",
        constant: "AURA_SHIELDED",
    },
];

/// One icon: a file under `assets/icons/` and the constant its texels are bound to.
///
/// The path carries the folder, and the folders are who the icon belongs to: `firemage/`,
/// `warrior/`, `druid/`, `equipment/`, `map/` (the dungeon map's markers and sidebar art), `modifier/` (dungeon key
/// modifiers) and `aura/` (buffs and debuffs that are not simply an ability's own effect). A class's art is then one directory to look in
/// rather than eleven names to pick out of forty-five.
struct IconEntry {
    /// File under `assets/icons/`, including its folder.
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
        compile_entry(&dxc, &shader_dir, &out_dir, shader, &mut generated);
    }

    // The mesh pixel shaders, once per combination of the three feature defines. Same
    // compile and the same generated constant as any other entry point; only the defines
    // and the name differ.
    for permutation in MESH_PERMUTATIONS {
        for mask in 0..(1u32 << MESH_FEATURES.len()) {
            let mut defines: Vec<&'static str> = Vec::new();
            let mut constant = String::from(permutation.prefix);
            for (bit, (define, suffix)) in MESH_FEATURES.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    defines.push(define);
                    constant.push('_');
                    constant.push_str(suffix);
                }
            }
            let entry = ShaderEntry {
                file: permutation.file,
                entry: "PSMain",
                profile: "ps_6_0",
                constant: Box::leak(constant.into_boxed_str()),
                defines: Box::leak(defines.into_boxed_slice()),
            };
            compile_entry(&dxc, &shader_dir, &out_dir, &entry, &mut generated);
        }
    }

    std::fs::write(out_dir.join("shaders.rs"), generated)
        .unwrap_or_else(|error| panic!("failed to write the generated shader module: {error}"));

    rasterize_icons(&manifest.join("assets").join("icons"), &out_dir);

    // The interface fonts, as one signed-distance-field atlas.
    println!("cargo:rerun-if-changed=build_fonts.rs");
    build_font_atlas(
        &manifest.join("..").join("engine-render-data").join("fonts"),
        &out_dir,
    );
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
/// Compiles one entry point and appends its generated constant.
///
/// The output filename carries the defines as well as the entry point: two permutations of
/// one `PSMain` are two files, and without the suffix the second would silently overwrite
/// the first and every variant would be whichever was compiled last.
fn compile_entry(
    dxc: &Path,
    shader_dir: &Path,
    out_dir: &Path,
    shader: &ShaderEntry,
    generated: &mut String,
) {
    let source = shader_dir.join(shader.file);
    println!("cargo:rerun-if-changed={}", source.display());

    let stem = shader.file.trim_end_matches(".hlsl");
    let mut name = format!("{stem}.{}", shader.entry);
    for define in shader.defines {
        name.push('.');
        name.push_str(&define.to_lowercase());
    }
    name.push_str(".dxil");
    let output = out_dir.join(name);
    compile(dxc, &source, shader, &output);

    let features = if shader.defines.is_empty() {
        String::from("no features")
    } else {
        shader.defines.join(", ")
    };
    generated.push_str(&format!(
        "
/// `{}` `{}` from `shaders/{}` ({}), compiled to DXIL at build time.
         pub const {}: &[u8] = include_bytes!(r\"{}\");
",
        shader.entry,
        shader.profile,
        shader.file,
        features,
        shader.constant,
        output.display(),
    ));
}

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
        .args(
            shader
                .defines
                .iter()
                .flat_map(|define| ["-D".to_string(), format!("{define}=1")]),
        )
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
